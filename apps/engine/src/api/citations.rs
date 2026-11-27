//! The citation network around one paper, from the reference lists of the papers Constellate has read
//! ([`crate::analysis::citations`]).
//!
//! ```text
//! GET  /api/papers/{id}/citations[?run=R]    the network around the paper, from the lists read so far; with R,
//!                                             how many of the papers citing it are of run R
//!                                             ([`CitationNetwork`]); seed_state "not_read" until its own list is
//! POST /api/papers/{id}/references[?run=R]   read the paper's reference list from its PDF (fetched first when it
//!                                             is not kept), look the works it cites that are not in the library
//!                                             up on arXiv, and answer the network
//! ```
//!
//! The GET reads nothing and asks no one: it links the kept lists to the library as it is now. The POST reads the
//! list once per PDF and version of the reader ([`crate::library::references`]: a second POST reads nothing), and
//! asks arXiv about at most [`MOST_LOOKUPS`] entries, one request every three seconds, never about an entry it
//! asked about before. Nothing else is asked of any outside service.
//!
//! A paper whose PDF cannot be had is answered with why, as the reader shows it: 404 when it is published nowhere
//! a program may fetch it from, 502 when its source refused or could not be reached, 422 when the PDF cannot be
//! read.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use serde::Deserialize;
use url::Url;

use super::reviews::papers_of;
use super::routes::{ApiError, Shared, on_the_side};
use crate::analysis::citations::{CitationNetwork, Linker, network};
use crate::library::references::{Arxiv, KeptReferences, Looked, ReferencesReading, Stopped, look_up, to_look_up};
use crate::library::{Library, LibraryError};
use crate::papers::pdf::PdfOutcome;
use crate::sources::citations::{self, Clock, Pacer};
use crate::sources::{Fetch, Limits, any_https, get_following};

/// Entries looked up on arXiv per request at most: at its pace, a minute. The rest are looked up when the
/// references are read again.
pub const MOST_LOOKUPS: usize = 20;
/// A paper's id is short ("2601.00001", "icml-2026-17", "hep-th/9901001"): anything longer is not one.
const LONGEST_ID: usize = 200;
/// What one answer of arXiv's API may be: a feed of at most five short entries.
const LOOKUP_LIMITS: Limits = Limits { max_bytes: 2 * 1024 * 1024, time: Duration::from_secs(30) };

/// The pace of every lookup the engine makes, whoever asks: two requests at once keep arXiv's pace together.
static PACE: LazyLock<Mutex<Pacer>> = LazyLock::new(|| Mutex::new(Pacer::default()));

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![("/api/papers/{id}/citations", get(citations_of)), ("/api/papers/{id}/references", post(read_references))]
}

#[derive(Deserialize)]
struct InRun {
    run: Option<String>,
}

/// Why a request was not answered with a network: as [`ApiError`] says, or why the paper's PDF cannot be had
/// (in the words of [`PdfOutcome::why_not`]), with its own status.
#[derive(Debug)]
enum Refusal {
    Api(ApiError),
    NoPdf { status: StatusCode, why: String },
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<LibraryError> for Refusal {
    fn from(error: LibraryError) -> Self {
        Self::Api(error.into())
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::NoPdf { status, why } => (status, Json(serde_json::json!({ "error": why }))).into_response(),
        }
    }
}

/// The paper's id as asked, if it can be one.
fn paper_id(id: &str) -> Result<&str, ApiError> {
    if id.is_empty() || id.len() > LONGEST_ID || id.chars().any(char::is_control) {
        return Err(ApiError::NotFound);
    }
    Ok(id)
}

/// The ids of run `run`'s papers, when one is asked about (its name checked by [`papers_of`]).
fn run_papers(engine: &Shared, run: Option<&str>) -> Result<Option<HashSet<String>>, ApiError> {
    Ok(run.map(|run| papers_of(engine, run, None)).transpose()?.map(|ids| ids.into_iter().collect()))
}

/// The network around `seed`, from every reference list kept, linked to the library's `linker` as it is now.
fn network_now(
    library: &Library,
    linker: &Linker,
    seed: &str,
    run: Option<&HashSet<String>>,
) -> Result<CitationNetwork, ApiError> {
    let kept = library.every_kept_reference_list()?;
    let read = linker.read_all(kept.into_iter().map(|(id, list)| (id, list.references())).collect());
    Ok(network(
        seed,
        linker,
        &read,
        run,
        chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).date_naive(),
    ))
}

fn network_of(engine: &Shared, seed: &str, run: Option<&str>) -> Result<CitationNetwork, ApiError> {
    let library = engine.library()?;
    library.paper(paper_id(seed)?)?.ok_or(ApiError::NotFound)?;
    let run = run_papers(engine, run)?;
    let papers = library.link_papers()?;
    network_now(&library, &Linker::new(&papers), seed, run.as_ref())
}

async fn citations_of(
    State(engine): State<Shared>,
    Path(seed): Path<String>,
    query: Result<Query<InRun>, QueryRejection>,
) -> Result<Json<CitationNetwork>, ApiError> {
    let Query(InRun { run }) = query?;
    Ok(Json(on_the_side(move || network_of(&engine, &seed, run.as_deref())).await?))
}

/// Time as the lookups' pace needs it: the wall clock.
struct Wall;

impl Clock for Wall {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Only arXiv's API is asked, over https; a redirect anywhere else is not followed.
fn on_arxiv_api(address: &Url) -> bool {
    any_https(address) && address.host_str() == Some("export.arxiv.org")
}

/// What the lookups reach: the network in the program, nothing at all in a test.
#[cfg(not(test))]
fn lookups_fetch() -> Box<dyn Fetch> {
    Box::new(crate::sources::Http::new())
}

#[cfg(test)]
fn lookups_fetch() -> Box<dyn Fetch> {
    struct NoNetwork;
    impl Fetch for NoNetwork {
        fn get(&self, url: &str, _: Limits) -> Result<crate::sources::Response, crate::sources::FetchError> {
            Err(crate::sources::FetchError::Unreachable(url.to_owned(), "a test reaches no network".to_owned()))
        }
    }
    Box::new(NoNetwork)
}

/// Look up on arXiv what `list` (of `seed`) cites that is not in the library, through `fetch`, at the pace every
/// lookup of the engine keeps. Blocks for up to [`MOST_LOOKUPS`] times arXiv's pause, and while another request's
/// lookups go on; a list with nothing to ask about waits for nobody.
fn look_up_on_arxiv(list: &mut KeptReferences, seed: &str, linker: &Linker, fetch: &dyn Fetch) -> Looked {
    if to_look_up(list, seed, linker).is_empty() {
        return Looked { asked: 0, found: 0, stopped: Stopped::AllAsked };
    }
    let mut get = |url: &str| -> Result<citations::Response, String> {
        let (_, response) = get_following(fetch, url, LOOKUP_LIMITS, &on_arxiv_api).map_err(|why| why.to_string())?;
        Ok(citations::Response { status: response.status, retry_after: response.retry_after, body: response.text() })
    };
    let mut pacer = PACE.lock().unwrap_or_else(PoisonError::into_inner);
    let arxiv = Arxiv { base: crate::sources::arxiv::API, pacer: &mut pacer, get: &mut get, clock: &mut Wall };
    look_up(list, seed, linker, MOST_LOOKUPS, arxiv)
}

/// The refusal for a paper whose PDF cannot be had: not found when it is published nowhere a program may fetch
/// it from, a bad gateway when its source failed.
fn no_pdf(outcome: &PdfOutcome) -> Refusal {
    let status = match outcome {
        PdfOutcome::NoAddress => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_GATEWAY,
    };
    let why = outcome.why_not().unwrap_or_else(|| "This paper's PDF could not be had".to_owned());
    Refusal::NoPdf { status, why }
}

fn read_then_network(engine: &Shared, seed: &str, run: Option<&str>) -> Result<CitationNetwork, Refusal> {
    let library = engine.library()?;
    library.paper(paper_id(seed)?)?.ok_or(ApiError::NotFound)?;
    // a run that is not there is not found before anything is fetched
    let run = run_papers(engine, run)?;
    let outcome = engine.pdfs.pdf(&library, seed)?;
    if outcome.file().is_none() {
        return Err(no_pdf(&outcome));
    }
    let (pdf_hash, mut list) = match library.references_of(seed)? {
        ReferencesReading::Kept { pdf_hash, list } | ReferencesReading::ReadNow { pdf_hash, list } => (pdf_hash, list),
        // taken away between the fetch and the reading
        ReferencesReading::NoPdf => return Err(no_pdf(&PdfOutcome::NoAddress)),
        ReferencesReading::Unreadable(why) => {
            let why = format!("This paper's PDF cannot be read: {why}");
            return Err(Refusal::NoPdf { status: StatusCode::UNPROCESSABLE_ENTITY, why });
        }
    };
    let papers = library.link_papers()?;
    let linker = Linker::new(&papers);
    let looked = look_up_on_arxiv(&mut list, seed, &linker, lookups_fetch().as_ref());
    if looked.asked > 0 {
        library.keep_lookups(seed, &pdf_hash, &list)?;
    }
    if let Stopped::Failed(why) = &looked.stopped {
        eprintln!("citations: arXiv lookups for {seed} stopped after {} requests: {why}", looked.asked);
    }
    Ok(network_now(&library, &linker, seed, run.as_ref())?)
}

async fn read_references(
    State(engine): State<Shared>,
    Path(seed): Path<String>,
    query: Result<Query<InRun>, QueryRejection>,
) -> Result<Json<CitationNetwork>, Refusal> {
    let Query(InRun { run }) = query.map_err(ApiError::from)?;
    let answer = tokio::task::spawn_blocking(move || read_then_network(&engine, &seed, run.as_deref()))
        .await
        .map_err(|_| ApiError::Internal)??;
    Ok(Json(answer))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use serde::Serialize;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::config::tests::STRONG;
    use super::super::config::{Environment, Listen};
    use super::super::routes::Engine;
    use super::*;
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::references::Lookup;
    use crate::library::{Known, Paper};
    use crate::settings::Choices;
    use crate::sources::fake::{Fake, status};

    const AT: &str = "127.0.0.1:8765";
    const SEED: &str = "hep-th/9901001";
    const SEED_PATH: &str = "hep-th%2F9901001";
    const CITER_PDF: &str = "https://arxiv.org/pdf/2601.00009";
    const GONE_PDF: &str = "https://arxiv.org/pdf/2601.00008";

    fn choices() -> Choices {
        let one = |name: &str| vec![name.to_owned()];
        Choices {
            embedding_models: one("Qwen/Qwen3-Embedding-0.6B"),
            clustering: one("spectral"),
            acceptance: one("all"),
            flags: one("llm_declared"),
            describe_models: one("claude-opus-5-5"),
        }
    }

    /// The seed's PDF: a paper of the library (by title), a paper on arXiv that is not, and one found nowhere.
    fn seed_pdf() -> Vec<u8> {
        pdf_with(&[
            "1 Introduction",
            "",
            "We build on [1], extend [2] and compare with [3].",
            "",
            "References",
            "",
            "[1] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.",
            "",
            "[2] A. Turing. Merging without interference. arXiv preprint arXiv:2301.01234, 2023.",
            "",
            "[3] G. Hopper. A work that is not in the library. In ICML, 2022.",
        ])
    }

    /// A paper of the library on arXiv that cites the seed by its old-style arXiv id; its PDF is not kept.
    fn citer_pdf() -> Vec<u8> {
        pdf_with(&[
            "1 Introduction",
            "",
            "Strings are studied in [1].",
            "",
            "References",
            "",
            "[1] E. Witten. String theory dynamics in various dimensions. arXiv:hep-th/9901001, 1999.",
            "",
            "[2] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.",
            "",
            "[3] G. Hopper. Another work that is not in the library. In ICML, 2022.",
        ])
    }

    fn on_arxiv(id: &str) -> Paper {
        Paper { pdf_url: Some(format!("https://arxiv.org/pdf/{id}")), ..paper(id) }
    }

    /// A library of the seed (its PDF kept), "arith" (cited by it), a citing paper whose PDF is fetched on demand,
    /// a conference paper published nowhere a program may fetch from, and an arXiv paper whose PDF is gone; runs
    /// "r1" (the seed and its citer) and "r2" (the seed alone).
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let mut seed = paper(SEED);
        seed.title = "String theory dynamics in various dimensions".to_owned();
        seed.published = "1999-01-04T00:00:00Z".to_owned();
        let mut arith = paper("arith");
        arith.title = "Editing models with task arithmetic".to_owned();
        arith.published = "2023-05-01T00:00:00Z".to_owned();
        let papers = [seed, arith, on_arxiv("2601.00009"), paper("iclr-2026-1"), on_arxiv("2601.00008")];
        library.add_papers(&papers, None, Known::Keep).unwrap();
        library.keep_pdf(SEED, &seed_pdf()).unwrap();
        for (run, ids) in [("r1", json!([{"id": SEED}, {"id": "2601.00009"}])), ("r2", json!([{"id": SEED}]))] {
            let folder = data.path().join("runs").join(run);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("graph.json"), json!({"nodes": ids, "edges": []}).to_string()).unwrap();
        }
        data
    }

    fn serve(data: &tempfile::TempDir, fake: Arc<Fake>) -> axum::Router {
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        engine.pdfs = crate::papers::pdf::Pdfs::new(fake);
        let provider = Arc::new(super::super::login::oidc::tests::Fake::new(&[]));
        let (router, _, _) = super::super::application(
            &listen,
            AT.parse().unwrap(),
            engine,
            super::super::routes::routes(),
            provider,
            || Ok(super::super::login::password::Hashing::light()),
        )
        .unwrap();
        router
    }

    fn fake() -> Arc<Fake> {
        let pdf = crate::sources::Response {
            status: 200,
            content_type: Some("application/pdf".to_owned()),
            location: None,
            retry_after: None,
            body: citer_pdf(),
        };
        Arc::new(Fake::new().with(CITER_PDF, pdf).with(GONE_PDF, status(404)))
    }

    async fn ask(router: &axum::Router, method: &str, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    // `CitationNetwork` and the types it uses, field for field as packages/contracts/src/index.ts writes them: an
    // answer that reads as these and writes back the same has every field of the contract and no other.

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractNetwork {
        seed: String,
        seed_state: ContractSeedState,
        references: Vec<ContractNeighbour>,
        citations: Vec<ContractNeighbour>,
        edges: Vec<ContractEdge>,
        similar: Vec<ContractSimilar>,
        totals: ContractTotals,
        papers_read: u64,
        papers_in_library: u64,
        built: String,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ContractSeedState {
        Read,
        NotRead,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractNeighbour {
        key: String,
        title: String,
        authors: Vec<String>,
        authors_total: u64,
        year: Option<u64>,
        venue: Option<String>,
        link: ContractLink,
        url: Option<String>,
        confidence: ContractConfidence,
        cited_by: u64,
        entry: Option<String>,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum ContractLink {
        InLibrary { paper: String },
        OnArxiv { arxiv: String },
        Unresolved,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ContractConfidence {
        Identified,
        TitleParsed,
        RawOnly,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractEdge {
        from: String,
        to: String,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractSimilar {
        a: String,
        b: String,
        shared_references: u64,
        co_cited: u64,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractTotals {
        references: u64,
        references_in_library: u64,
        references_on_arxiv: u64,
        references_unresolved: u64,
        citations: u64,
        citations_shown: u64,
        citations_in_run: Option<u64>,
    }

    /// `answer` is a `CitationNetwork` of the contracts, field for field.
    fn as_contract(answer: &Value) {
        let typed: ContractNetwork = serde_json::from_value(answer.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), *answer, "every field of the contract, and only those");
        assert!(chrono::NaiveDate::parse_from_str(&typed.built, "%Y-%m-%d").is_ok(), "built is YYYY-MM-DD");
    }

    fn keys(works: &Value) -> Vec<String> {
        works.as_array().unwrap().iter().map(|work| work["key"].as_str().unwrap().to_owned()).collect()
    }

    #[tokio::test]
    async fn a_paper_whose_references_were_not_read_says_so_and_nothing_is_read_or_fetched() {
        let data = data();
        let fake = fake();
        let router = serve(&data, fake.clone());
        let (status, network) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations")).await;
        assert_eq!(status, StatusCode::OK, "{network}");
        as_contract(&network);
        assert_eq!(network["seed"], SEED);
        assert_eq!(network["seed_state"], "not_read");
        assert_eq!((network["references"].clone(), network["citations"].clone()), (json!([]), json!([])));
        assert_eq!((network["papers_read"].clone(), network["papers_in_library"].clone()), (json!(0), json!(5)));
        assert_eq!(network["totals"]["citations_in_run"], Value::Null);
        assert!(fake.asked().is_empty());
        let library = Library::open(&data.path().join("library")).unwrap();
        assert!(library.every_kept_reference_list().unwrap().is_empty(), "a GET reads nothing");
        assert_eq!(ask(&router, "GET", "/api/papers/nowhere/citations").await.0, StatusCode::NOT_FOUND);
        let long = "x".repeat(LONGEST_ID + 1);
        assert_eq!(ask(&router, "GET", &format!("/api/papers/{long}/citations")).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reading_a_papers_references_keeps_them_and_a_second_read_reads_nothing() {
        let data = data();
        let fake = fake();
        let router = serve(&data, fake.clone());
        let (status, network) = ask(&router, "POST", &format!("/api/papers/{SEED_PATH}/references")).await;
        assert_eq!(status, StatusCode::OK, "{network}");
        as_contract(&network);
        assert_eq!(network["seed_state"], "read");
        assert_eq!(
            keys(&network["references"]),
            ["paper:arith", "arxiv:2301.01234", "title:aworkthatisnotinthelibrary"]
        );
        let references = &network["references"];
        assert_eq!(references[0]["link"], json!({"kind": "in_library", "paper": "arith"}));
        assert_eq!(references[0]["title"], "Editing models with task arithmetic");
        assert_eq!(references[1]["link"], json!({"kind": "on_arxiv", "arxiv": "2301.01234"}));
        assert_eq!(references[1]["url"], "https://arxiv.org/abs/2301.01234");
        assert_eq!(references[2]["link"], json!({"kind": "unresolved"}));
        assert_eq!(references[2]["confidence"], "title_parsed");
        assert!(references[2]["entry"].as_str().unwrap().starts_with("G. Hopper."));
        let totals = &network["totals"];
        assert_eq!(
            [&totals["references"], &totals["references_in_library"], &totals["references_on_arxiv"]],
            [&json!(3), &json!(1), &json!(1)]
        );
        assert_eq!(network["papers_read"], 1);
        assert!(fake.asked().is_empty(), "a kept PDF is not fetched");

        // mark the kept list: read again, it would lose the mark
        let library = Library::open(&data.path().join("library")).unwrap();
        let ReferencesReading::Kept { pdf_hash, mut list } = library.references_of(SEED).unwrap() else { panic!() };
        list.entries[2].lookup = Lookup::NotFound;
        assert!(library.keep_lookups(SEED, &pdf_hash, &list).unwrap());
        let (status, again) = ask(&router, "POST", &format!("/api/papers/{SEED_PATH}/references")).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["references"], network["references"]);
        let ReferencesReading::Kept { list, .. } = library.references_of(SEED).unwrap() else { panic!() };
        assert_eq!(list.entries[2].lookup, Lookup::NotFound, "kept: not read again");

        // and the GET serves what was read
        let (_, kept) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations")).await;
        assert_eq!(
            (kept["seed_state"].clone(), kept["references"].clone()),
            (json!("read"), network["references"].clone())
        );
    }

    #[tokio::test]
    async fn a_citing_papers_pdf_is_fetched_once_and_the_run_counts_the_papers_citing_the_seed() {
        let data = data();
        let fake = fake();
        let router = serve(&data, fake.clone());
        let (status, citer) = ask(&router, "POST", "/api/papers/2601.00009/references").await;
        assert_eq!(status, StatusCode::OK, "{citer}");
        assert_eq!(fake.asked(), [CITER_PDF]);
        assert_eq!(citer["references"][0]["link"], json!({"kind": "in_library", "paper": SEED}));
        ask(&router, "POST", "/api/papers/2601.00009/references").await;
        assert_eq!(fake.asked(), [CITER_PDF], "fetched once, then kept");

        let (_, around) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations?run=r1")).await;
        as_contract(&around);
        assert_eq!(around["seed_state"], "not_read");
        assert_eq!(keys(&around["citations"]), ["paper:2601.00009"]);
        assert_eq!(around["citations"][0]["entry"], Value::Null);
        assert_eq!(
            (around["totals"]["citations"].clone(), around["totals"]["citations_in_run"].clone()),
            (json!(1), json!(1))
        );
        let (_, other) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations?run=r2")).await;
        assert_eq!(other["totals"]["citations_in_run"], 0, "the citing paper is not of run r2");
        let (_, any) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations")).await;
        assert_eq!(any["totals"]["citations_in_run"], Value::Null);

        // both read: the seed's own list adds the edge from its citer to what both cite
        let (_, both) = ask(&router, "POST", &format!("/api/papers/{SEED_PATH}/references?run=r1")).await;
        as_contract(&both);
        assert_eq!(both["edges"], json!([{"from": "paper:2601.00009", "to": "paper:arith"}]));
        assert_eq!(both["totals"]["citations_in_run"], 1);

        for run in ["none", "..", "a%2Fb"] {
            let path = format!("/api/papers/{SEED_PATH}/citations?run={run}");
            assert_eq!(ask(&router, "GET", &path).await.0, StatusCode::NOT_FOUND, "{run}");
            let path = format!("/api/papers/{SEED_PATH}/references?run={run}");
            assert_eq!(ask(&router, "POST", &path).await.0, StatusCode::NOT_FOUND, "{run}");
        }
    }

    #[tokio::test]
    async fn a_paper_whose_pdf_cannot_be_had_is_answered_with_why() {
        let data = data();
        let router = serve(&data, fake());
        let (status, told) = ask(&router, "POST", "/api/papers/iclr-2026-1/references").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{told}");
        assert_eq!(told["error"], PdfOutcome::NoAddress.why_not().unwrap());
        let (status, told) = ask(&router, "POST", "/api/papers/2601.00008/references").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{told}");
        assert!(told["error"].as_str().unwrap().contains("404"), "{told}");
        let (status, told) = ask(&router, "POST", "/api/papers/nowhere/references").await;
        assert_eq!((status, told["error"].clone()), (StatusCode::NOT_FOUND, json!("not found")));
        assert_eq!(ask(&router, "GET", "/api/papers/2601.00008/citations").await.1["seed_state"], "not_read");
    }
}
