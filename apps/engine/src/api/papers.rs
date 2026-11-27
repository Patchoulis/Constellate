//! What the reader asks about one paper: its record, its PDF, what its authors declare; the same declarations
//! for every paper of a run, as the job; and the reviewers' scores of every paper whose reviews page was opened.
//!
//! ```text
//! GET  /api/papers/{id}               the paper's record as the reader shows it (`Paper`)
//! GET  /api/papers/{id}/pdf           its PDF: kept, or fetched now and kept; a byte range when asked for one
//! POST /api/papers/{id}/declarations  what its authors declare, read from the PDF once (`Declarations`)
//! POST /api/runs/{run}/declarations   the same for every paper of the run, as the job: {ok, id}
//! GET  /api/reviews                   {paper id: `Ratings`} for every paper whose reviews page was opened
//! ```
//!
//! These replace the Python backend's `GET /api/papers/<id>`, `GET /api/pdf/<id>`, `POST /api/declarations/<id>`,
//! `POST /api/runs/<name>/declarations` and `GET /api/reviews`; its `POST /api/reviews/<id>` is the engine's
//! `POST /api/papers/{id}/reviews` ([`super::reviews`]). A paper id comes percent-encoded in one path segment
//! (an old arXiv id has a `/`: `hep-th%2F9901001`) and is decoded once, by the router. It never becomes a path
//! here: the PDF's file is the library's own naming of it ([`crate::library::Library::pdf_file`]).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use serde::Serialize;
use serde_json::Value;

use super::citations::{Refusal, no_pdf, paper_id};
use super::reviews::papers_of;
use super::routes::{ApiError, Shared, on_the_side, one_name};
use crate::jobs::Finished;
use crate::library::{Declarations, Ratings, Reading};
use crate::papers::declarations::{Declared, declarations_of, read_and_count};
use crate::papers::pdf::{PdfOutcome, as_source};
use crate::sources::acceptance::{Acceptance, Term, accepted};
use crate::text::declarations::PARSER;

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}", get(record)),
        ("/api/papers/{id}/pdf", get(pdf)),
        ("/api/papers/{id}/declarations", post(declarations)),
        ("/api/runs/{run}/declarations", post(run_declarations)),
        ("/api/reviews", get(all_ratings)),
    ]
}

/// The PDF's own page may frame it (the reader's frame), nothing may load from it, the browser's viewer may show
/// it.
const PDF_POLICY: &str = "default-src 'none'; object-src 'self'; frame-ancestors 'self'";

/// `Paper` in the contracts, field for field.
#[derive(Debug, Serialize)]
struct Record {
    id: String,
    title: String,
    r#abstract: String,
    authors: Vec<String>,
    published: String,
    comment: String,
    primary_category: String,
    categories: Vec<String>,
    venues: Vec<String>,
    url: Option<String>,
    pdf_url: Option<String>,
    /// The AI-text detector's verdict: given by the detector, which is not part of this module; null meanwhile,
    /// as the contract says for a paper never scored.
    ai: Option<Value>,
    acceptance: Acceptance,
    acceptance_term: Option<Term>,
    readable: bool,
    declarations: Option<Declarations>,
    reviews: Option<Ratings>,
}

fn record_of(engine: &Shared, id: &str) -> Result<Record, ApiError> {
    let library = engine.library()?;
    let paper = library.paper(paper_id(id)?)?.ok_or(ApiError::NotFound)?;
    let readable = engine.pdfs.readable(&library, &paper)?;
    let declarations = match library.kept_declarations(&[id], PARSER)?.remove(id).map(|kept| kept.reading) {
        Some(Reading::Read(declarations)) => Some(declarations),
        Some(Reading::NoPdf | Reading::Unreadable) | None => None,
    };
    let reviews = library.ratings(id)?;
    let told = accepted(&as_source(&paper));
    Ok(Record {
        id: paper.id,
        title: paper.title,
        r#abstract: paper.r#abstract,
        authors: paper.authors,
        published: paper.published,
        comment: paper.comment,
        primary_category: paper.primary_category,
        categories: paper.categories,
        venues: paper.venues,
        url: paper.url,
        pdf_url: paper.pdf_url,
        ai: None,
        acceptance: told.tier,
        acceptance_term: told.term,
        readable,
        declarations,
        reviews,
    })
}

async fn record(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Record>, ApiError> {
    Ok(Json(on_the_side(move || record_of(&engine, &id)).await?))
}

/// A byte range of a file of `size` bytes, as `Range: bytes=...` asks for one: `None` for the whole file (no
/// range, several ranges, or a unit other than bytes, all of which are answered with the whole file, as HTTP
/// allows), `Err` for a range outside the file.
fn range_of(asked: Option<&HeaderValue>, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = asked.and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("bytes=")) else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((first, last)) = spec.trim().split_once('-') else { return Ok(None) };
    let number = |text: &str| text.trim().parse::<u64>().ok();
    let (start, end) = match (first.trim().is_empty(), number(first), number(last)) {
        // the last n bytes
        (true, _, Some(suffix)) if suffix > 0 => (size.saturating_sub(suffix), size.saturating_sub(1)),
        (false, Some(start), None) if last.trim().is_empty() => (start, size.saturating_sub(1)),
        (false, Some(start), Some(end)) if start <= end => (start, end.min(size.saturating_sub(1))),
        _ => return Ok(None),
    };
    if size == 0 || start >= size { Err(()) } else { Ok(Some((start, end))) }
}

/// The PDF's bytes `from..=to` as an answer, with the headers every answer of a PDF carries.
fn pdf_answer(path: &std::path::Path, id: &str, asked: Option<HeaderValue>) -> Result<Response, Refusal> {
    let files = |error: std::io::Error| {
        eprintln!("papers: the PDF of {id} could not be read: {error}");
        Refusal::from(ApiError::Files)
    };
    let mut file = std::fs::File::open(path).map_err(files)?;
    let size = file.metadata().map_err(files)?.len();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(PDF_POLICY));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    // the file's own name, the library's naming of the id: letters, digits, `_`, `.` and `-` only
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("paper.pdf");
    let disposition = format!("inline; filename=\"{name}\"");
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).unwrap_or(HeaderValue::from_static("inline")),
    );
    let (status, start, end) = match range_of(asked.as_ref(), size) {
        Ok(None) => (StatusCode::OK, 0, size.saturating_sub(1)),
        Ok(Some((start, end))) => {
            let range = format!("bytes {start}-{end}/{size}");
            headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&range).map_err(|_| ApiError::Internal)?);
            (StatusCode::PARTIAL_CONTENT, start, end)
        }
        Err(()) => {
            let range = format!("bytes */{size}");
            headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&range).map_err(|_| ApiError::Internal)?);
            return Ok((StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response());
        }
    };
    let mut bytes = Vec::new();
    if size > 0 {
        file.seek(SeekFrom::Start(start)).map_err(files)?;
        file.take(end - start + 1).read_to_end(&mut bytes).map_err(files)?;
    }
    Ok((status, headers, Body::from(bytes)).into_response())
}

async fn pdf(State(engine): State<Shared>, Path(id): Path<String>, asked: HeaderMap) -> Result<Response, Refusal> {
    let range = asked.get(header::RANGE).cloned();
    let served = tokio::task::spawn_blocking(move || {
        let library = engine.library()?;
        let outcome = engine.pdfs.pdf(&library, paper_id(&id)?)?;
        match &outcome {
            PdfOutcome::Kept(path) | PdfOutcome::FetchedNow(path) => pdf_answer(path, &id, range),
            _ => Err(no_pdf(&outcome)),
        }
    });
    served.await.map_err(|_| Refusal::from(ApiError::Internal))?
}

/// Why the authors' declarations cannot be told, as the reader shows it.
fn refusal(declared: Declared) -> Result<Declarations, Refusal> {
    let unreadable = |why: String| Refusal::NoPdf { status: StatusCode::UNPROCESSABLE_ENTITY, why };
    match declared {
        Declared::Read(declarations) => Ok(declarations),
        Declared::NoPdf(outcome) => Err(no_pdf(&outcome)),
        Declared::NoText => {
            Err(unreadable("This paper's PDF has no text that can be read (it may be a scan)".to_owned()))
        }
        Declared::Unreadable(why) => Err(unreadable(format!("This paper's PDF could not be read: {why}"))),
    }
}

async fn declarations(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Declarations>, Refusal> {
    let read = tokio::task::spawn_blocking(move || {
        let library = engine.library()?;
        library.paper(paper_id(&id)?)?.ok_or(ApiError::NotFound)?;
        refusal(declarations_of(&library, &engine.pdfs, &id)?)
    });
    Ok(Json(read.await.map_err(|_| Refusal::from(ApiError::Internal))??))
}

/// Start the job that reads the declarations of every paper of the run. It counts its papers, can be stopped,
/// and keeps each batch as it goes, so starting it again carries on where it stopped.
async fn run_declarations(State(engine): State<Shared>, Path(run): Path<String>) -> Result<Json<Value>, ApiError> {
    one_name(&run)?;
    let shared = engine.clone();
    let id = on_the_side(move || {
        let ids = papers_of(&shared, &run, None)?;
        let library = shared.library()?;
        let reader = shared.clone();
        Ok(shared.job.start_counted("reading what the authors declare", move |log, counter| {
            log(&format!("reading what the authors of {} papers declare", ids.len()));
            let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
            let counts = read_and_count(&library, &reader.pdfs, &wanted, counter).map_err(|error| error.to_string())?;
            log(&format!("  {}", counts.summary()));
            if counter.is_cancelled() {
                log("  stopped: what was read is kept, and the next start carries on from there");
            }
            Ok(Finished::Run { run })
        })?)
    })
    .await?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id })))
}

async fn all_ratings(State(engine): State<Shared>) -> Result<Json<HashMap<String, Ratings>>, ApiError> {
    Ok(Json(on_the_side(move || Ok(engine.library()?.all_ratings()?)).await?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    use super::super::config::tests::STRONG;
    use super::super::config::{Environment, Listen};
    use super::super::routes::Engine;
    use super::super::tests::served;
    use super::*;
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Known, Library, Paper};
    use crate::papers::pdf::Pdfs;
    use crate::settings::Choices;
    use crate::sources::Response as Answer;
    use crate::sources::fake::{Fake, status};

    const AT: &str = "127.0.0.1:8765";

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

    fn checklist_pdf() -> Vec<u8> {
        let mut lines: Vec<String> =
            (0..40).map(|line| format!("Line {line} of the paper's own text, as long as a paper's is.")).collect();
        for line in [
            "NeurIPS Paper Checklist",
            "5. Open access to data and code",
            "Question: Does the paper provide open access to the data and code?",
            "Answer: [No]",
            "Justification: The code is proprietary.",
        ] {
            lines.push(line.to_owned());
        }
        pdf_with(&lines.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn pdf_answer(bytes: Vec<u8>) -> Answer {
        Answer {
            status: 200,
            content_type: Some("application/pdf".into()),
            location: None,
            retry_after: None,
            body: bytes,
        }
    }

    /// The engine on this computer with a library of: an old arXiv id with its PDF kept; an arXiv paper whose PDF
    /// is fetched (from the fake); an ICLR paper published nowhere a program may fetch from; an arXiv paper whose
    /// source answers 404; and a run of the first four.
    fn engine() -> (axum::Router, tempfile::TempDir, Arc<Fake>) {
        let data = tempfile::tempdir().unwrap();
        let on_arxiv = |id: &str| Paper { pdf_url: Some(format!("https://arxiv.org/pdf/{id}")), ..paper(id) };
        {
            let library = Library::open(&data.path().join("library")).unwrap();
            let iclr = Paper { comment: "ICLR 2026 Oral".into(), ..paper("iclr-2026-2") };
            library
                .add_papers(
                    &[on_arxiv("hep-th/9901001"), on_arxiv("2601.00001"), iclr, on_arxiv("2601.00404")],
                    None,
                    Known::Keep,
                )
                .unwrap();
            library.keep_pdf("hep-th/9901001", &checklist_pdf()).unwrap();
        }
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let nodes = json!({"nodes": [{"id": "hep-th/9901001"}, {"id": "2601.00001"}, {"id": "iclr-2026-2"}, {"id": "2601.00404"}, {"id": "gone-from-the-library"}], "edges": []});
        std::fs::write(run.join("graph.json"), nodes.to_string()).unwrap();
        let fake = Arc::new(
            Fake::new()
                .with("https://arxiv.org/pdf/2601.00001", pdf_answer(checklist_pdf()))
                .with("https://arxiv.org/pdf/2601.00404", status(404)),
        );
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        engine.pdfs = Pdfs::new(fake.clone());
        let (router, _) = served(&listen, AT, engine);
        (router, data, fake)
    }

    async fn ask(router: &axum::Router, method: &str, path: &str, range: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
    }

    async fn json_of(response: axum::response::Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The fields of an interface of the contracts, as written there.
    fn contract_fields(name: &str) -> BTreeSet<String> {
        let contracts = include_str!("../../../../packages/contracts/src/index.ts");
        let start = contracts.find(&format!("export interface {name} {{")).unwrap();
        let body = &contracts[start..];
        let body = &body[body.find('{').unwrap() + 1..body.find("\n}").unwrap()];
        body.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("/*") && !line.starts_with('*') && !line.starts_with("//"))
            .filter_map(|line| line.split_once(':').map(|(field, _)| field.trim_end_matches('?').to_owned()))
            .filter(|field| field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !field.is_empty())
            .collect()
    }

    #[tokio::test]
    async fn a_papers_record_has_the_fields_of_the_contracts_paper_and_an_old_arxiv_id_is_found() {
        let (router, _data, _) = engine();
        let (status, told) = json_of(ask(&router, "GET", "/api/papers/hep-th%2F9901001", None).await).await;
        assert_eq!(status, StatusCode::OK);
        let fields: BTreeSet<String> = told.as_object().unwrap().keys().cloned().collect();
        assert_eq!(fields, contract_fields("Paper"));
        assert_eq!((told["id"].clone(), told["readable"].clone()), (json!("hep-th/9901001"), json!(true)));
        assert_eq!((told["declarations"].clone(), told["reviews"].clone()), (Value::Null, Value::Null));
        assert_eq!(told["acceptance"], Value::Null, "an arXiv paper was accepted nowhere");

        let (_, iclr) = json_of(ask(&router, "GET", "/api/papers/iclr-2026-2", None).await).await;
        assert_eq!((iclr["readable"].clone(), iclr["acceptance"].clone()), (json!(false), json!("oral")));
        assert_eq!(iclr["acceptance_term"]["conference"], "ICLR 2026");
        for missing in ["/api/papers/nowhere", "/api/papers/hep-th/9901001"] {
            assert_eq!(ask(&router, "GET", missing, None).await.status(), StatusCode::NOT_FOUND, "{missing}");
        }
    }

    #[tokio::test]
    async fn a_kept_pdf_is_served_inline_as_a_pdf_and_by_ranges() {
        let (router, _data, fake) = engine();
        let whole = ask(&router, "GET", "/api/papers/hep-th%2F9901001/pdf", None).await;
        assert_eq!(whole.status(), StatusCode::OK);
        let headers = whole.headers().clone();
        assert_eq!(headers[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(headers[header::CONTENT_DISPOSITION], "inline; filename=\"hep-th_9901001.pdf\"");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
        let bytes = to_bytes(whole.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.as_ref(), checklist_pdf().as_slice());

        let part = ask(&router, "GET", "/api/papers/hep-th%2F9901001/pdf", Some("bytes=0-7")).await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_RANGE], format!("bytes 0-7/{}", bytes.len()));
        assert_eq!(to_bytes(part.into_body(), usize::MAX).await.unwrap().as_ref(), &bytes[..8]);
        let last = ask(&router, "GET", "/api/papers/hep-th%2F9901001/pdf", Some("bytes=-5")).await;
        assert_eq!(to_bytes(last.into_body(), usize::MAX).await.unwrap().as_ref(), &bytes[bytes.len() - 5..]);
        let outside = format!("bytes={}-", bytes.len() + 10);
        let refused = ask(&router, "GET", "/api/papers/hep-th%2F9901001/pdf", Some(&outside)).await;
        assert_eq!(refused.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(fake.asked().is_empty(), "a kept PDF asks nobody");
    }

    #[tokio::test]
    async fn a_pdf_not_kept_is_fetched_once_and_one_that_cannot_be_had_says_why() {
        let (router, data, fake) = engine();
        assert_eq!(ask(&router, "GET", "/api/papers/2601.00001/pdf", None).await.status(), StatusCode::OK);
        assert_eq!(ask(&router, "GET", "/api/papers/2601.00001/pdf", None).await.status(), StatusCode::OK);
        assert_eq!(fake.asked(), ["https://arxiv.org/pdf/2601.00001"]);
        assert!(data.path().join("library").join("pdfs").join("2601.00001.pdf").is_file());

        let (status, told) = json_of(ask(&router, "GET", "/api/papers/iclr-2026-2/pdf", None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(told["error"].as_str().unwrap().contains("not published anywhere"), "{told}");
        let (status, told) = json_of(ask(&router, "GET", "/api/papers/2601.00404/pdf", None).await).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(told["error"].as_str().unwrap().contains("404"), "{told}");
        assert_eq!(ask(&router, "GET", "/api/papers/nowhere/pdf", None).await.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_papers_declarations_are_read_once_and_then_part_of_its_record() {
        let (router, _data, _) = engine();
        let (status, told) =
            json_of(ask(&router, "POST", "/api/papers/hep-th%2F9901001/declarations", None).await).await;
        assert_eq!(status, StatusCode::OK);
        let fields: BTreeSet<String> = told.as_object().unwrap().keys().cloned().collect();
        assert_eq!(fields, contract_fields("Declarations"));
        assert_eq!(told["flags"], json!(["closed_code"]));
        assert_eq!(told["checklist"][0]["answer"], "No");
        let (_, record) = json_of(ask(&router, "GET", "/api/papers/hep-th%2F9901001", None).await).await;
        assert_eq!(record["declarations"], told);

        let (status, told) = json_of(ask(&router, "POST", "/api/papers/iclr-2026-2/declarations", None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(told["error"].is_string());
    }

    #[tokio::test]
    async fn a_runs_declarations_are_read_as_the_job_and_a_bad_run_is_not_found() {
        let (router, data, _) = engine();
        let (status, told) = json_of(ask(&router, "POST", "/api/runs/r1/declarations", None).await).await;
        assert_eq!((status, told["ok"].clone()), (StatusCode::OK, json!(true)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let status = loop {
            let (_, status) = json_of(ask(&router, "GET", "/api/status", None).await).await;
            if status["state"] != "running" {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "the job never ended");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(status["state"], "done", "{status}");
        assert_eq!(status["result"], "r1");
        assert_eq!((status["papers"]["done"].clone(), status["papers"]["of"].clone()), (json!(5), json!(5)));
        let log = status["log"].to_string();
        assert!(log.contains("2 read now") && log.contains("2 have no PDF"), "{log}");
        assert!(log.contains("1 could not be read"), "a paper the library no longer has stops nothing: {log}");
        let library = Library::open(&data.path().join("library")).unwrap();
        assert_eq!(library.kept_declarations(&["2601.00001", "hep-th/9901001"], PARSER).unwrap().len(), 2);
        for bad in ["/api/runs/nothing/declarations", "/api/runs/..%2Fx/declarations"] {
            assert_eq!(ask(&router, "POST", bad, None).await.status(), StatusCode::NOT_FOUND, "{bad}");
        }
    }

    #[tokio::test]
    async fn the_scores_of_every_paper_whose_reviews_were_read_are_told_by_paper() {
        let (router, data, _) = engine();
        let (status, told) = json_of(ask(&router, "GET", "/api/reviews", None).await).await;
        assert_eq!((status, told), (StatusCode::OK, json!({})));
        let ratings: Ratings =
            serde_json::from_value(json!({"reviews": 2, "exact": true, "ratings": {"Rating": {"values": [5, 7], "mean": 6.0, "meanings": ["", ""]}}}))
                .unwrap();
        Library::open(&data.path().join("library")).unwrap().keep_ratings("2601.00001", ratings).unwrap();
        let (_, told) = json_of(ask(&router, "GET", "/api/reviews", None).await).await;
        assert_eq!(told["2601.00001"]["ratings"]["Rating"]["values"], json!([5, 7]));
        let fields: BTreeSet<String> = told["2601.00001"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(fields, contract_fields("Ratings"));
    }

    #[test]
    fn a_range_is_read_as_http_says_and_anything_else_is_the_whole_file() {
        let range = |text: &str| range_of(Some(&HeaderValue::from_str(text).unwrap()), 100);
        assert_eq!(range("bytes=0-9"), Ok(Some((0, 9))));
        assert_eq!(range("bytes=90-"), Ok(Some((90, 99))));
        assert_eq!(range("bytes=-10"), Ok(Some((90, 99))));
        assert_eq!(range("bytes=50-500"), Ok(Some((50, 99))));
        assert_eq!(range("bytes=100-"), Err(()));
        for whole in ["bytes=0-1,5-6", "items=0-1", "bytes=9-3", "bytes=x-y", "bytes=-0"] {
            assert_eq!(range(whole), Ok(None), "{whole}");
        }
        assert_eq!(range_of(None, 100), Ok(None));
    }
}
