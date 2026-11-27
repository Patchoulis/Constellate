//! A paper's text from its PDF, and where a quote is in it: what the reader's Text tab shows and what a click on
//! a piece of evidence lands on (docs/evidence.md, "The routes").
//!
//! ```text
//! GET  /api/papers/{id}/passages   the paper's passages (`PaperPassages`); 404 when its PDF cannot be had or read
//! POST /api/papers/{id}/locate     {quote}: where the quote is, in the paper's own words (`LocateAnswer`)
//! ```
//!
//! Both read the paper through the library ([`crate::library::Library::paper_texts`]): its PDF is read once per
//! version of the reader and kept, so the second request for a paper costs a database read.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get, post};
use serde::Deserialize;

use super::routes::{ApiError, Shared, on_the_side};
use crate::library::PaperReading;
use crate::text::passages::{LocateAnswer, Passages};

/// The longest quote looked for, in characters: a paragraph or two. A quote is the paper's words, not the paper.
pub const LONGEST_QUOTE: usize = 5_000;

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![("/api/papers/{id}/passages", get(passages)), ("/api/papers/{id}/locate", post(locate))]
}

/// `LocateRequest` in the contracts.
#[derive(Deserialize)]
struct LocateRequest {
    quote: String,
}

/// The paper's passages, or not found: not in the library, no PDF kept, or a PDF that cannot be read (the reason
/// goes to the engine's error output; the reader then shows the PDF itself, which the browser may still manage).
fn read(engine: &Shared, paper_id: &str) -> Result<Passages, ApiError> {
    let library = engine.library()?;
    match library.paper_texts(&[paper_id])?.pop() {
        Some(PaperReading::Read(passages)) => Ok(passages),
        Some(PaperReading::Unreadable { why, .. }) => {
            eprintln!("passages: the PDF of {paper_id} cannot be read: {why}");
            Err(ApiError::NotFound)
        }
        Some(PaperReading::NoPdf { .. } | PaperReading::NotInLibrary) | None => Err(ApiError::NotFound),
    }
}

async fn passages(State(engine): State<Shared>, Path(paper_id): Path<String>) -> Result<Json<Passages>, ApiError> {
    Ok(Json(on_the_side(move || read(&engine, &paper_id)).await?))
}

async fn locate(
    State(engine): State<Shared>,
    Path(paper_id): Path<String>,
    body: Result<Json<LocateRequest>, JsonRejection>,
) -> Result<Json<LocateAnswer>, ApiError> {
    let Json(LocateRequest { quote }) = body?;
    if quote.chars().nth(LONGEST_QUOTE).is_some() {
        return Err(ApiError::Invalid(format!("the quote is longer than {LONGEST_QUOTE} characters")));
    }
    if quote.trim().is_empty() {
        return Err(ApiError::Invalid("the quote is empty".to_owned()));
    }
    let answer = on_the_side(move || Ok(LocateAnswer::from(read(&engine, &paper_id)?.locate(&quote)))).await?;
    Ok(Json(answer))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::config::tests::STRONG;
    use super::super::config::{Environment, Listen};
    use super::super::routes::Engine;
    use super::super::tests::served;
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Known, Library};
    use crate::settings::Choices;

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

    /// The engine as served on this computer, with three papers: one with a PDF, one without, one whose PDF is
    /// not one.
    fn engine() -> (axum::Router, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        {
            let library = Library::open(&data.path().join("library")).unwrap();
            library
                .add_papers(&[paper("2601.00001"), paper("iclr-2026-2"), paper("hep-th/9901001")], None, Known::Keep)
                .unwrap();
            let pdf = pdf_with(&["Introduction", "We show that kept passages are served to the reader of the paper."]);
            library.keep_pdf("2601.00001", &pdf).unwrap();
            library.keep_pdf("hep-th/9901001", b"%PDF-1.4 and then nothing a reader could use").unwrap();
        }
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let (router, _) = served(&listen, AT, Engine::new(data.path().to_path_buf(), choices()));
        (router, data)
    }

    async fn ask(router: &axum::Router, method: &str, path: &str, body: Option<String>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or(Body::empty(), Body::from))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_papers_passages_are_served_as_the_contracts_describe_them() {
        let (router, _data) = engine();
        let (status, told) = ask(&router, "GET", "/api/papers/2601.00001/passages", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(told["reader"], crate::text::passages::READER);
        assert_eq!(told["pages"][0]["number"], 1);
        let passage = &told["passages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|passage| passage["text"].as_str().unwrap().contains("kept passages"))
            .unwrap();
        assert!(passage["text"].as_str().unwrap().contains("kept passages are served"));
        assert_eq!(passage["address"]["page"], 1);
        assert!(passage["sentences"].is_array());
        // read once: the second answer is the kept one, the same
        assert_eq!(ask(&router, "GET", "/api/papers/2601.00001/passages", None).await.1, told);
    }

    #[tokio::test]
    async fn a_quote_is_located_in_the_papers_own_words_or_said_not_to_be_there() {
        let (router, _data) = engine();
        let found = json!({"quote": "kept  passages are SERVED to the reader"}).to_string();
        let (status, told) = ask(&router, "POST", "/api/papers/2601.00001/locate", Some(found)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!((told["found"].clone(), told["matched"]["kind"].clone()), (json!(true), json!("exact")));
        assert_eq!(told["wording"], "kept passages are served to the reader");
        assert_eq!(told["address"]["page"], 1);
        let missing = json!({"quote": "Nothing of the sort is written in this paper at all"}).to_string();
        assert_eq!(
            ask(&router, "POST", "/api/papers/2601.00001/locate", Some(missing)).await.1,
            json!({"found": false})
        );
    }

    #[tokio::test]
    async fn a_paper_without_a_readable_pdf_and_a_bad_request_are_told_apart() {
        let (router, _data) = engine();
        for paper in ["iclr-2026-2", "hep-th%2F9901001", "nowhere"] {
            let (status, _) = ask(&router, "GET", &format!("/api/papers/{paper}/passages"), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{paper}");
            let quote = json!({"quote": "kept passages are served to the reader"}).to_string();
            let (status, _) = ask(&router, "POST", &format!("/api/papers/{paper}/locate"), Some(quote)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{paper}");
        }
        let long = json!({"quote": "x".repeat(super::LONGEST_QUOTE + 1)}).to_string();
        for body in [long, json!({"quote": "  "}).to_string(), json!({"text": "x"}).to_string(), "[1]".to_owned()] {
            let (status, _) = ask(&router, "POST", "/api/papers/2601.00001/locate", Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn the_status_says_whether_a_key_is_set_and_never_the_key() {
        let data = tempfile::tempdir().unwrap();
        let listen = Listen::from_environment(&Environment::from_pairs([("CONSTELLATE_TOKEN", STRONG)])).unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        let (router, _) = served(&listen, AT, Engine::new(data.path().to_path_buf(), choices()));
        assert_eq!(ask(&router, "GET", "/api/status", None).await.1["api_key_set"], false);
        let key = "test-key-not-real-0123456789";
        engine.language = Environment::from_pairs([("ANTHROPIC_API_KEY", key)]).language_endpoint();
        let (router, _) = served(&listen, AT, engine);
        let (_, told) = ask(&router, "GET", "/api/status", None).await;
        assert_eq!(told["api_key_set"], true);
        assert!(!told.to_string().contains(key));
    }

    #[tokio::test]
    async fn the_running_job_can_be_asked_to_stop_and_no_other() {
        let data = tempfile::tempdir().unwrap();
        let listen = Listen::from_environment(&Environment::from_pairs([("CONSTELLATE_TOKEN", STRONG)])).unwrap();
        let engine = Engine::new(data.path().to_path_buf(), choices());
        let id = engine
            .job
            .start_tracked("asking about papers", |tracker| {
                while !tracker.cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(crate::jobs::Finished::Nothing)
            })
            .unwrap();
        let (router, _) = served(&listen, AT, engine);
        let other = json!({"id": id + 1}).to_string();
        assert_eq!(ask(&router, "POST", "/api/job/cancel", Some(other)).await.0, StatusCode::BAD_REQUEST);
        let (status, told) = ask(&router, "POST", "/api/job/cancel", Some(json!({"id": id}).to_string())).await;
        assert_eq!((status, told), (StatusCode::OK, json!({"ok": true})));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let (_, status) = ask(&router, "GET", "/api/status", None).await;
            if status["state"] == "done" {
                assert_eq!((status["outcome"].clone(), status["id"].clone()), (json!({"kind": "nothing"}), json!(id)));
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the job never stopped");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}
