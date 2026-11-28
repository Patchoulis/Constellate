//! The citation network around one paper, and what a run and its communities cite and are cited, from the
//! reference lists of the papers of the library Constellate has read ([`crate::analysis::citations`]).
//!
//! ```text
//! GET  /api/papers/{id}/citations[?run=R]    the network around the paper, from the lists read so far, its
//!                                             citing papers from the whole library; with R, which of its
//!                                             neighbours are papers of run R and how many
//!                                             ([`CitationNetwork`]); seed_state "not_read" until its own list is
//! POST /api/papers/{id}/references[?run=R]   read the paper's reference list from its PDF (fetched first when it
//!                                             is not kept), look the works it cites that are not in the library
//!                                             up on arXiv, and answer the network
//! GET  /api/runs/{run}/citations             what the run and each of its communities cite and are cited
//!                                             ([`RunCitations`]): citations from inside a community, from the rest
//!                                             of the run and from the library outside it, counted apart
//! ```
//!
//! The GETs read nothing and ask no one: they count from the kept lists linked to the library as it is now, linked
//! once and kept ([`Linking`]). The POST reads the
//! list once per PDF and version of the reader ([`crate::library::papers::references`]: a second POST reads nothing), and
//! asks arXiv about at most [`MOST_LOOKUPS`] entries, at arXiv's one pace (first come, first served with the PDFs
//! and listings the engine fetches: [`crate::sources::polite::ARXIV_PACE`]), never about an entry it asked about
//! before; when it stops before every entry was asked about, the answer says so (`lookups_stopped`). Two POSTs for
//! one paper are made one after the other: the second finds what the first read and looked up. A PDF that cannot
//! be read is kept as such, and read again only once it is replaced. Nothing else is asked of any outside service.
//!
//! A paper whose PDF cannot be had is answered with why, as the reader shows it: 404 when it is published nowhere
//! a program may fetch it from, 502 when its source refused or could not be reached, 422 when the PDF cannot be
//! read.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Condvar, LazyLock, Mutex, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, SystemTime};

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use serde::Deserialize;
use url::Url;

use super::reviews::{papers_of, tree_of};
use crate::analysis::citations::{
    CitationNetwork, LinkedLibrary, Linker, LookupsStopped, RunCitations, community_citations, network,
};
use crate::api::routes::{ApiError, Shared, on_the_side, one_name};
use crate::library::papers::references::{
    Arxiv, KeptReferences, Looked, ReferencesReading, Revisions, Stopped, look_up, to_look_up,
};
use crate::library::{Library, LibraryError};
use crate::papers::pdf::PdfOutcome;
use crate::pipeline::results::files;
use crate::sources::arxiv::citations;
use crate::sources::{Fetch, Limits, any_https, get_following};

/// Entries looked up on arXiv per request at most: at its pace, a minute. The rest are looked up when the
/// references are read again.
pub const MOST_LOOKUPS: usize = 20;
/// A paper's id is short ("2601.00001", "icml-2026-17", "hep-th/9901001"): anything longer is not one.
const LONGEST_ID: usize = 200;
/// What one answer of arXiv's API may be: a feed of at most five short entries.
const LOOKUP_LIMITS: Limits = Limits { max_bytes: 2 * 1024 * 1024, time: Duration::from_secs(30) };

/// The papers whose references are being read, by data folder and paper: a second POST for one of them waits for
/// the first, then finds what it read and looked up, rather than asking arXiv the same again and overwriting it.
#[derive(Default)]
struct Reading {
    papers: Mutex<HashSet<(PathBuf, String)>>,
    done: Condvar,
}

static READING: LazyLock<Reading> = LazyLock::new(Reading::default);

/// A paper being read, from [`Reading::claim`] until dropped (on a panic too).
struct Claim<'a> {
    reading: &'a Reading,
    paper: (PathBuf, String),
}

impl Reading {
    /// Wait until nobody else reads `paper` of `data`, then claim it.
    fn claim(&self, data: PathBuf, paper: &str) -> Claim<'_> {
        let paper = (data, paper.to_owned());
        let mut papers = self.papers.lock().unwrap_or_else(PoisonError::into_inner);
        while papers.contains(&paper) {
            papers = self.done.wait(papers).unwrap_or_else(PoisonError::into_inner);
        }
        papers.insert(paper.clone());
        Claim { reading: self, paper }
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.reading.papers.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.paper);
        self.reading.done.notify_all();
    }
}

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/citations", get(citations_of)),
        ("/api/papers/{id}/references", post(read_references)),
        ("/api/runs/{run}/citations", get(run_citations)),
    ]
}

#[derive(Deserialize)]
struct InRun {
    run: Option<String>,
}

/// Why a request was not answered with a network: as [`ApiError`] says, or why the paper's PDF cannot be had
/// (in the words of [`PdfOutcome::why_not`]), with its own status.
#[derive(Debug)]
pub(super) enum Refusal {
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
pub(super) fn paper_id(id: &str) -> Result<&str, ApiError> {
    if id.is_empty() || id.len() > LONGEST_ID || id.chars().any(char::is_control) {
        return Err(ApiError::NotFound);
    }
    Ok(id)
}

/// The ids of run `run`'s papers, when one is asked about (its name checked by [`papers_of`]).
fn run_papers(engine: &Shared, run: Option<&str>) -> Result<Option<HashSet<String>>, ApiError> {
    Ok(run.map(|run| papers_of(engine, run, None)).transpose()?.map(|ids| ids.into_iter().collect()))
}

/// The library's reference lists linked ([`LinkedLibrary`]), kept for as long as the engine runs, and each run's
/// figures counted from them.
///
/// A request first asks the library how far its papers and lists have moved ([`Library::citation_revisions`]: two
/// numbers); when neither moved, nothing is read or linked. When one did, the stamps of the lists are read (no list),
/// the papers only when they moved, and only the lists new or changed since ([`LinkedLibrary::update`]). A run's figures are
/// counted again only when the linking moved or the run's files were written since.
#[derive(Default)]
pub struct Linking {
    kept: RwLock<Kept>,
    runs: Mutex<HashMap<String, Counted>>,
}

#[derive(Default)]
struct Kept {
    library: LinkedLibrary,
    /// The library's revisions it was brought up to, none before the first request.
    at: Option<Revisions>,
}

/// A run's figures, and what they were counted from.
struct Counted {
    from: RunFrom,
    figures: RunCitations,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunFrom {
    linked: u64,
    graph: Option<SystemTime>,
    communities: Option<SystemTime>,
}

impl Linking {
    /// The library linked as it is now: brought up to date first, with what changed alone.
    fn linked(&self, library: &Library) -> Result<RwLockReadGuard<'_, Kept>, ApiError> {
        let now = library.citation_revisions()?;
        if self.kept.read().unwrap_or_else(PoisonError::into_inner).at != Some(now) {
            let mut kept = self.kept.write().unwrap_or_else(PoisonError::into_inner);
            // another request may have brought it up to date meanwhile
            if kept.at != Some(now) {
                let at = kept.at;
                let papers = (at.map(|at| at.papers) != Some(now.papers)).then(|| library.link_papers()).transpose()?;
                // the stamps are read whenever anything moved (they are small, no list is read): a paper removed
                // takes its list with it
                let stamps = Some(library.reference_list_stamps()?);
                kept.library.update(papers, stamps, |wanted| library.reference_lists_of(wanted))?;
                kept.at = Some(now);
            }
        }
        Ok(self.kept.read().unwrap_or_else(PoisonError::into_inner))
    }
}

/// When a file was last written, if it is there.
fn written(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// The network around `seed`, from every reference list kept, linked to the library as it is now.
fn network_now(
    engine: &Shared,
    library: &Library,
    seed: &str,
    run: Option<&HashSet<String>>,
) -> Result<CitationNetwork, ApiError> {
    let kept = engine.citations.linked(library)?;
    Ok(network(
        seed,
        &kept.library,
        run,
        chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).date_naive(),
    ))
}

fn network_of(engine: &Shared, seed: &str, run: Option<&str>) -> Result<CitationNetwork, ApiError> {
    let library = engine.library()?;
    library.paper(paper_id(seed)?)?.ok_or(ApiError::NotFound)?;
    let run = run_papers(engine, run)?;
    network_now(engine, &library, seed, run.as_ref())
}

/// What run `run` and its communities cite and are cited, from every reference list kept: counted once per
/// change of the linking or of the run.
fn run_figures(engine: &Shared, run: &str) -> Result<RunCitations, ApiError> {
    let library = engine.library()?;
    let kept = engine.citations.linked(&library)?;
    let folder = engine.data.join("runs").join(one_name(run)?);
    let from = RunFrom {
        linked: kept.library.revision(),
        graph: written(&folder.join(files::GRAPH)),
        communities: written(&folder.join(files::COMMUNITIES)),
    };
    let mut runs = engine.citations.runs.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(counted) = runs.get(run).filter(|counted| counted.from == from) {
        return Ok(counted.figures.clone());
    }
    let (papers, tree) = tree_of(engine, run)?;
    let figures = community_citations(&kept.library, &papers, &tree);
    runs.insert(run.to_owned(), Counted { from, figures: figures.clone() });
    Ok(figures)
}

async fn run_citations(State(engine): State<Shared>, Path(run): Path<String>) -> Result<Json<RunCitations>, ApiError> {
    Ok(Json(on_the_side(move || run_figures(&engine, &run)).await?))
}

async fn citations_of(
    State(engine): State<Shared>,
    Path(seed): Path<String>,
    query: Result<Query<InRun>, QueryRejection>,
) -> Result<Json<CitationNetwork>, ApiError> {
    let Query(InRun { run }) = query?;
    Ok(Json(on_the_side(move || network_of(&engine, &seed, run.as_deref())).await?))
}

/// Only arXiv's API is asked, over https; a redirect anywhere else is not followed.
fn on_arxiv_api(address: &Url) -> bool {
    any_https(address) && address.host_str() == Some("export.arxiv.org")
}

/// Look up on arXiv what `list` (of `seed`) cites that is not in the library, through `fetch` (the engine's: the
/// network, which keeps arXiv's one pace). Blocks for up to [`MOST_LOOKUPS`] turns at arXiv; a list with nothing
/// to ask about waits for nobody.
fn look_up_on_arxiv(list: &mut KeptReferences, seed: &str, linker: &Linker, fetch: &dyn Fetch) -> Looked {
    if to_look_up(list, seed, linker).is_empty() {
        return Looked { asked: 0, found: 0, stopped: Stopped::AllAsked };
    }
    let mut get = |url: &str| -> Result<citations::Response, String> {
        let (_, response) = get_following(fetch, url, LOOKUP_LIMITS, &on_arxiv_api).map_err(|why| why.to_string())?;
        Ok(citations::Response { status: response.status, retry_after: response.retry_after, body: response.text() })
    };
    let arxiv = Arxiv { base: crate::sources::arxiv::API, get: &mut get };
    look_up(list, seed, linker, MOST_LOOKUPS, arxiv)
}

/// What the answer says of lookups that stopped before every entry worth asking about was asked about; `left` of
/// them are still to be asked.
fn stopped_early(stopped: &Stopped, left: usize) -> Option<LookupsStopped> {
    match stopped {
        Stopped::AllAsked => None,
        Stopped::AtMost { left } => Some(LookupsStopped::AtMost { left: *left }),
        Stopped::Failed(why) => Some(LookupsStopped::Failed { why: why.to_string(), left }),
    }
}

/// The refusal for a paper whose PDF cannot be had: not found when it is published nowhere a program may fetch
/// it from, a bad gateway when its source failed.
pub(super) fn no_pdf(outcome: &PdfOutcome) -> Refusal {
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
    // one at a time per paper: the fetch, the reading, the lookups and keeping them
    let _claim = READING.claim(engine.data.clone(), seed);
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
    let looked = look_up_on_arxiv(&mut list, seed, &linker, engine.pdfs.fetch());
    if looked.asked > 0 {
        library.keep_lookups(seed, &pdf_hash, &list)?;
    }
    if let Stopped::Failed(why) = &looked.stopped {
        eprintln!("citations: arXiv lookups for {seed} stopped after {} requests: {why}", looked.asked);
    }
    let left = to_look_up(&list, seed, &linker).len();
    let mut answer = network_now(engine, &library, seed, run.as_ref())?;
    answer.lookups_stopped = stopped_early(&looked.stopped, left);
    Ok(answer)
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

    use super::*;
    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::api::routes::Engine;
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::references::Lookup;
    use crate::library::papers::tests::paper;
    use crate::library::{Known, Paper};
    use crate::settings::Choices;
    use crate::sources::fake::{Fake, ok, status};

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
        let community = |path: &str, members: Value, children: Value| json!({"tree_path": path, "members": members, "children": children});
        let r1 = json!([community(
            "c0",
            json!([0, 1]),
            json!([community("c0.0", json!([0]), json!([])), community("c0.1", json!([1]), json!([]))])
        )]);
        let r2 = json!([community("c0", json!([0]), json!([]))]);
        for (run, ids, tree) in
            [("r1", json!([{"id": SEED}, {"id": "2601.00009"}]), r1), ("r2", json!([{"id": SEED}]), r2)]
        {
            let folder = data.path().join("runs").join(run);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("graph.json"), json!({"nodes": ids, "edges": []}).to_string()).unwrap();
            std::fs::write(folder.join("communities.json"), json!({"communities": tree}).to_string()).unwrap();
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
        let provider = Arc::new(crate::api::guard::login::oidc::tests::Fake::new(&[]));
        let (router, _, _) = crate::api::application(
            &listen,
            AT.parse().unwrap(),
            engine,
            crate::api::routes::routes(),
            provider,
            || Ok(crate::api::guard::login::password::Hashing::light()),
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
        lookups_stopped: Option<ContractLookupsStopped>,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum ContractLookupsStopped {
        AtMost { left: u64 },
        Failed { why: String, left: u64 },
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
        in_run: Option<bool>,
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
        references_in_run: Option<u64>,
        references_outside_run: Option<u64>,
        references_on_arxiv: u64,
        references_unresolved: u64,
        citations: u64,
        citations_shown: u64,
        citations_in_run: Option<u64>,
        citations_outside_run: Option<u64>,
    }

    // `RunCitations` of the contracts, field for field.

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractCounts {
        papers: u64,
        papers_read: u64,
        cited_by: u64,
        cited_by_inside: u64,
        cited_by_rest_of_run: u64,
        cited_by_outside_run: u64,
        cites: u64,
        cites_in_library: u64,
        cites_in_run: u64,
        cites_outside_run: u64,
        cites_on_arxiv: u64,
        cites_unresolved: u64,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContractCountedCommunity {
        tree_path: String,
        papers: u64,
        papers_read: u64,
        cited_by: u64,
        cited_by_inside: u64,
        cited_by_rest_of_run: u64,
        cited_by_outside_run: u64,
        cites: u64,
        cites_in_library: u64,
        cites_in_run: u64,
        cites_outside_run: u64,
        cites_on_arxiv: u64,
        cites_unresolved: u64,
        children: Vec<ContractCountedCommunity>,
    }

    #[derive(Serialize, Deserialize)]
    struct ContractRunCitations {
        #[serde(flatten)]
        counts: ContractCounts,
        library_papers: u64,
        library_papers_read: u64,
        communities: Vec<ContractCountedCommunity>,
    }

    /// `answer` is a `RunCitations` of the contracts, field for field.
    fn as_run_contract(answer: &Value) {
        let typed: ContractRunCitations = serde_json::from_value(answer.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), *answer, "every field of the contract, and only those");
    }

    /// `answer` is a `CitationNetwork` of the contracts, field for field.
    fn as_contract(answer: &Value) {
        let typed: ContractNetwork = serde_json::from_value(answer.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), *answer, "every field of the contract, and only those");
        assert!(chrono::NaiveDate::parse_from_str(&typed.built, "%Y-%m-%d").is_ok(), "built is YYYY-MM-DD");
    }

    /// What was asked of the fake besides arXiv's API (the lookups): the PDFs.
    fn pdfs_asked(fake: &Fake) -> Vec<String> {
        fake.asked().into_iter().filter(|address| !address.starts_with(crate::sources::arxiv::API)).collect()
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
        assert!(pdfs_asked(&fake).is_empty(), "a kept PDF is not fetched");
        // the fake has no answer for arXiv's API: the one lookup failed, and the answer says so
        assert_eq!(network["lookups_stopped"]["kind"], "failed", "{network}");
        assert_eq!(network["lookups_stopped"]["left"], 1);

        // mark the kept list: read again, it would lose the mark
        let library = Library::open(&data.path().join("library")).unwrap();
        let ReferencesReading::Kept { pdf_hash, mut list } = library.references_of(SEED).unwrap() else { panic!() };
        list.entries[2].lookup = Lookup::NotFound;
        assert!(library.keep_lookups(SEED, &pdf_hash, &list).unwrap());
        let (status, again) = ask(&router, "POST", &format!("/api/papers/{SEED_PATH}/references")).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["references"], network["references"]);
        assert_eq!(again["lookups_stopped"], Value::Null, "nothing left to ask about");
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
        assert_eq!(pdfs_asked(&fake), [CITER_PDF]);
        assert_eq!(citer["references"][0]["link"], json!({"kind": "in_library", "paper": SEED}));
        ask(&router, "POST", "/api/papers/2601.00009/references").await;
        assert_eq!(pdfs_asked(&fake), [CITER_PDF], "fetched once, then kept");

        let (_, around) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations?run=r1")).await;
        as_contract(&around);
        assert_eq!(around["seed_state"], "not_read");
        assert_eq!(keys(&around["citations"]), ["paper:2601.00009"]);
        assert_eq!(around["citations"][0]["entry"], Value::Null);
        assert_eq!(
            (around["totals"]["citations"].clone(), around["totals"]["citations_in_run"].clone()),
            (json!(1), json!(1))
        );
        assert_eq!(around["citations"][0]["in_run"], true);
        let (_, other) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations?run=r2")).await;
        assert_eq!(other["totals"]["citations_in_run"], 0, "the citing paper is not of run r2");
        assert_eq!(keys(&other["citations"]), ["paper:2601.00009"], "but it is of the library, and is counted");
        assert_eq!(
            (other["totals"]["citations"].clone(), other["totals"]["citations_outside_run"].clone()),
            (json!(1), json!(1))
        );
        assert_eq!(other["citations"][0]["in_run"], false, "marked outside the run");
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
    async fn a_runs_figures_count_citers_from_the_whole_library_and_follow_a_list_read_since() {
        let data = data();
        let router = serve(&data, fake());
        let (status, before) = ask(&router, "GET", "/api/runs/r2/citations").await;
        assert_eq!(status, StatusCode::OK, "{before}");
        as_run_contract(&before);
        assert_eq!((before["cited_by"].clone(), before["library_papers_read"].clone()), (json!(0), json!(0)));
        assert_eq!(before["library_papers"], 5);

        // the citer, a paper of the library outside run r2, is read: r2's seed is cited from outside the run
        assert_eq!(ask(&router, "POST", "/api/papers/2601.00009/references").await.0, StatusCode::OK);
        let (_, r2) = ask(&router, "GET", "/api/runs/r2/citations").await;
        as_run_contract(&r2);
        let split = |figures: &Value| {
            ["cited_by", "cited_by_inside", "cited_by_rest_of_run", "cited_by_outside_run"]
                .map(|field| figures[field].clone())
        };
        assert_eq!(split(&r2), [json!(1), json!(0), json!(0), json!(1)], "{r2}");
        assert_eq!(split(&r2["communities"][0]), [json!(1), json!(0), json!(0), json!(1)]);
        assert_eq!((r2["papers_read"].clone(), r2["library_papers_read"].clone()), (json!(0), json!(1)));

        // in run r1 the citer is of the run: of the rest of the run for the seed's community
        let (_, r1) = ask(&router, "GET", "/api/runs/r1/citations").await;
        as_run_contract(&r1);
        assert_eq!(split(&r1), [json!(1), json!(1), json!(0), json!(0)]);
        let seed = &r1["communities"][0]["children"][0];
        assert_eq!(seed["tree_path"], "c0.0");
        assert_eq!(split(seed), [json!(1), json!(0), json!(1), json!(0)]);
        // what the citer cites: the seed (of the run), arith (of the library, not of the run), one not found
        let citer = &r1["communities"][0]["children"][1];
        let cites = ["cites", "cites_in_run", "cites_outside_run", "cites_on_arxiv", "cites_unresolved"]
            .map(|field| citer[field].clone());
        assert_eq!(cites, [json!(3), json!(1), json!(1), json!(0), json!(1)], "{citer}");
        assert_eq!(ask(&router, "GET", "/api/runs/r1/citations").await.1, r1, "the same when nothing changed");

        assert_eq!(ask(&router, "GET", "/api/runs/none/citations").await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "GET", "/api/runs/..%2Fx/citations").await.0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_linking_is_kept_until_a_list_is_read_or_a_paper_added() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let mut seed = paper(SEED);
        seed.title = "String theory dynamics in various dimensions".to_owned();
        library.add_papers(&[seed, paper("arith")], None, Known::Keep).unwrap();
        let linking = Linking::default();
        let first = linking.linked(&library).unwrap().library.revision();
        assert_eq!(linking.linked(&library).unwrap().library.revision(), first, "nothing changed: kept as it was");

        library.add_papers(&[on_arxiv("2601.00009")], None, Known::Keep).unwrap();
        let added = linking.linked(&library).unwrap().library.revision();
        assert!(added > first, "a paper added");
        library.keep_pdf("2601.00009", &citer_pdf()).unwrap();
        assert!(matches!(library.references_of("2601.00009").unwrap(), ReferencesReading::ReadNow { .. }));
        let kept = linking.linked(&library).unwrap();
        assert!(kept.library.revision() > added, "a list read");
        assert!(kept.library.citers(&format!("paper:{SEED}")).unwrap().contains("2601.00009"));
        let read = kept.library.revision();
        drop(kept);
        assert_eq!(linking.linked(&library).unwrap().library.revision(), read);
    }

    #[tokio::test]
    async fn what_a_paper_cites_is_looked_up_on_arxiv_once_even_when_two_ask_at_once() {
        let data = data();
        let title = "A work that is not in the library";
        let search = citations::url_for(crate::sources::arxiv::API, &citations::Ask::Title(title.to_owned()));
        let feed = r#"<feed><entry><id>http://arxiv.org/abs/2205.00001v2</id><title>A Work That Is Not
            in the Library</title><published>2022-05-01T00:00:00Z</published></entry></feed>"#;
        let fake =
            Arc::new(Fake::new().with(&search, ok("application/atom+xml", feed)).slow(Duration::from_millis(50)));
        let router = serve(&data, fake.clone());
        let path = format!("/api/papers/{SEED_PATH}/references");
        let ((first_status, first), (second_status, second)) =
            tokio::join!(ask(&router, "POST", &path), ask(&router, "POST", &path));
        assert_eq!((first_status, second_status), (StatusCode::OK, StatusCode::OK), "{first} {second}");
        as_contract(&first);
        assert_eq!(
            fake.asked(),
            std::slice::from_ref(&search),
            "asked once: the second waited for the first and found it"
        );
        for answer in [&first, &second] {
            assert_eq!(answer["references"][2]["link"], json!({"kind": "on_arxiv", "arxiv": "2205.00001"}));
            assert_eq!(answer["references"][2]["confidence"], "identified");
            assert_eq!(answer["lookups_stopped"], Value::Null, "every entry worth asking about was asked");
        }
        assert_eq!(first["totals"]["references_on_arxiv"], 2);
        let (_, again) = ask(&router, "POST", &path).await;
        assert_eq!(again["references"], first["references"]);
        assert_eq!(fake.asked().len(), 1, "an entry is never asked about twice");
        let (_, kept) = ask(&router, "GET", &format!("/api/papers/{SEED_PATH}/citations")).await;
        assert_eq!(kept["references"][2]["link"], json!({"kind": "on_arxiv", "arxiv": "2205.00001"}));
    }

    #[tokio::test]
    async fn lookups_that_arxiv_refuses_stop_and_say_how_many_are_left() {
        let data = data();
        let title = "A work that is not in the library";
        let search = citations::url_for(crate::sources::arxiv::API, &citations::Ask::Title(title.to_owned()));
        let refused = crate::sources::Response { retry_after: Some(Duration::from_secs(40)), ..status(429) };
        let fake = Arc::new(Fake::new().with(&search, refused));
        let router = serve(&data, fake.clone());
        let (status, network) = ask(&router, "POST", &format!("/api/papers/{SEED_PATH}/references")).await;
        assert_eq!(status, StatusCode::OK, "the references read are answered all the same");
        as_contract(&network);
        let stopped = &network["lookups_stopped"];
        assert_eq!((stopped["kind"].clone(), stopped["left"].clone()), (json!("failed"), json!(1)));
        assert!(stopped["why"].as_str().unwrap().contains("40 seconds"), "{stopped}");
        assert_eq!(network["references"][2]["link"], json!({"kind": "unresolved"}));
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
