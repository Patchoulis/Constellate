//! A paper's claims as a tree with its reviews mapped onto them ([`crate::analysis::claim_tree`]), and how many
//! of a community's papers' claims reviewers contested.
//!
//! ```text
//! GET    /api/papers/{id}/claims    what is kept, checked again, and what asking the rest would cost (`ClaimsView`)
//! POST   /api/papers/{id}/claims    {budget_usd}: ask what is not kept, as the job; answers {job}
//! DELETE /api/papers/{id}/claims    forget the paper's tree and mapping (a redo); answers {forgotten}
//! GET    /api/runs/{run}/communities/{community}/claims/contested   per paper, its contested claims (`Contested`)
//! ```
//!
//! The GET asks nothing: the interface shows its estimate, and the user starts the work with a click (the POST),
//! capped at the budget the user agreed to. The model is the one chosen in the settings.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get};
use serde::{Deserialize, Serialize};

use super::reviews::papers_of;
use super::routes::{ApiError, Shared, on_the_side};
use crate::analysis::claim_tree::asking::{Asked, Askers, AskingError, ask};
use crate::analysis::claim_tree::{self, ClaimsView, Contested};
use crate::library::{Feature, PaperReading, Subject};
use crate::model::language::{Budget, DEFAULT_MODEL, Https, Limit};
use crate::settings;
use crate::usage::Usd;

/// The most one paper's two calls may be allowed to spend, in US dollars: far above what they take (a few cents
/// to a dollar), so that a mistyped budget is refused rather than spent.
pub const MOST_BUDGET_USD: f64 = 10.0;
/// A paper id longer than this is no paper's.
const LONGEST_ID: usize = 200;

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/claims", get(read).post(start).delete(forget)),
        ("/api/runs/{run}/communities/{community}/claims/contested", get(community_contested)),
    ]
}

/// `ClaimsStart` in the contracts.
#[derive(Deserialize)]
pub struct Start {
    /// The most the calls may spend, in US dollars: what the user agreed to after seeing the estimate.
    budget_usd: f64,
}

#[derive(Serialize)]
pub struct Started {
    /// The job's number, as `/api/status` gives it.
    job: u64,
}

#[derive(Serialize)]
pub struct Forgotten {
    forgotten: usize,
}

fn paper_id(id: &str) -> Result<&str, ApiError> {
    if id.is_empty() || id.len() > LONGEST_ID { Err(ApiError::NotFound) } else { Ok(id) }
}

/// The model chosen in the settings, the default when none is.
fn chosen_model(engine: &Shared) -> Result<String, ApiError> {
    let chosen = settings::load(&engine.settings_file(), &engine.choices)?.describe_model;
    Ok(if chosen.trim().is_empty() { DEFAULT_MODEL.to_owned() } else { chosen })
}

pub async fn read(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<ClaimsView>, ApiError> {
    paper_id(&id)?;
    let view = on_the_side(move || {
        let model = chosen_model(&engine)?;
        claim_tree::view(&*engine.library()?, &id, &model)?.ok_or(ApiError::NotFound)
    })
    .await?;
    Ok(Json(view))
}

fn told(asked: &Asked) -> String {
    match asked {
        Asked::Kept => "kept from before".to_owned(),
        Asked::Answered => "answered".to_owned(),
        Asked::Failed(why) => format!("no usable answer ({why})"),
        Asked::NotStarted(why) => format!("not asked ({why})"),
        Asked::Skipped(why) => format!("not asked: {why}"),
    }
}

pub async fn start(
    State(engine): State<Shared>,
    Path(id): Path<String>,
    body: Result<Json<Start>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    paper_id(&id)?;
    let Json(Start { budget_usd }) = body?;
    if !budget_usd.is_finite() || budget_usd <= 0.0 || budget_usd > MOST_BUDGET_USD {
        return Err(ApiError::Invalid(format!(
            "the budget must be more than 0 and at most {MOST_BUDGET_USD} US dollars"
        )));
    }
    let endpoint = engine.language.clone().map_err(|why| ApiError::Invalid(why.to_string()))?;
    let job = on_the_side(move || {
        let library = engine.library()?;
        let model = chosen_model(&engine)?;
        match library.paper_texts(&[id.as_str()])?.pop() {
            Some(PaperReading::Read(_)) => {}
            Some(PaperReading::NotInLibrary) | None => return Err(ApiError::NotFound),
            Some(PaperReading::NoPdf { .. } | PaperReading::Unreadable { .. }) => {
                return Err(ApiError::Invalid(AskingError::NoPdf.to_string()));
            }
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above: 0 < budget <= 10
        let budget = Budget::new(Limit::Dollars(Usd::from_picodollars((budget_usd * 1e12).round() as u64)));
        let job = engine.job.start_tracked("reading a paper's claims", move |tracker| {
            let transport = Https::default();
            let askers = Askers { library: &library, transport: &transport, endpoint, recorder: &*library };
            let done = ask(&askers, &id, &model, &budget, tracker).map_err(|error| error.to_string())?;
            tracker.log(&format!("The claim tree: {}", told(&done.tree)));
            tracker.log(&format!("The reviews mapped onto it: {}", told(&done.mapping)));
            // no run's name: the interface opens nothing when this is done
            Ok(String::new())
        })?;
        Ok(job)
    })
    .await?;
    Ok(Json(Started { job }))
}

pub async fn forget(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Forgotten>, ApiError> {
    paper_id(&id)?;
    let forgotten = on_the_side(move || {
        let library = engine.library()?;
        library.paper(&id)?.ok_or(ApiError::NotFound)?;
        Ok(library.forget(Feature::ClaimTree, &[Subject::Paper(id)])?)
    })
    .await?;
    Ok(Json(Forgotten { forgotten }))
}

pub async fn community_contested(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
) -> Result<Json<Contested>, ApiError> {
    let contested = on_the_side(move || {
        let papers = papers_of(&engine, &run, Some(&community))?;
        let model = chosen_model(&engine)?;
        Ok(claim_tree::contested(&*engine.library()?, &papers, &model)?)
    })
    .await?;
    Ok(Json(contested))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, mpsc};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::config::tests::STRONG;
    use super::super::config::{Environment, Listen};
    use super::super::routes::Engine;
    use super::super::tests::served;
    use super::*;
    use crate::analysis::claim_tree::asking::tests::{FORUM, LINES, PAPER, keep_reviews};
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Known, Library};
    use crate::model::language::{BASE_VARIABLE, Endpoint, KEY_VARIABLE};
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

    /// An endpoint on this computer that nothing listens on: no request can reach any provider from a test.
    fn nowhere() -> Endpoint {
        Endpoint::from_lookup(|name| match name {
            KEY_VARIABLE => Some("test-key-not-real".to_owned()),
            BASE_VARIABLE => Some("http://127.0.0.1:9".to_owned()),
            _ => None,
        })
        .unwrap()
    }

    /// The engine with a library of a paper with a PDF and reviews, one without a PDF, and a run of both.
    fn engine(language: bool) -> (Engine, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        {
            let library = Library::open(&data.path().join("library")).unwrap();
            let mut with_pdf = paper(PAPER);
            with_pdf.url = Some(FORUM.to_owned());
            library.add_papers(&[with_pdf, paper("2601.00002")], None, Known::Keep).unwrap();
            library.keep_pdf(PAPER, &pdf_with(&LINES)).unwrap();
            keep_reviews(&library);
        }
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("graph.json"),
            json!({"nodes": [{"id": PAPER}, {"id": "2601.00002"}], "edges": []}).to_string(),
        )
        .unwrap();
        std::fs::write(
            run.join("communities.json"),
            json!({"communities": [{"tree_path": "c0", "members": [0, 1], "children": []}]}).to_string(),
        )
        .unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        if language {
            engine.language = Ok(nowhere());
        }
        (engine, data)
    }

    fn router(engine: Engine) -> axum::Router {
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        served(&listen, AT, engine).0
    }

    async fn ask(router: &axum::Router, method: &str, path: &str, body: Option<String>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .header(header::CONTENT_TYPE, "application/json");
        let response =
            router.clone().oneshot(request.body(body.map_or(Body::empty(), Body::from)).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_view_of_a_paper_says_what_is_kept_and_what_the_rest_would_cost() {
        let (engine, _data) = engine(false);
        let router = router(engine);
        let (status, view) = ask(&router, "GET", &format!("/api/papers/{PAPER}/claims"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(view["model"], "claude-opus-5-5", "the model chosen in the settings");
        assert_eq!(view["tree"]["state"], "not_asked");
        assert_eq!(view["mapping"]["state"], "no_tree");
        assert_eq!(view["estimate"]["to_ask"], 2);
        assert_eq!(view["estimate"]["by_model"][0]["counting"]["kind"], "assumed");
        assert_eq!(view["reviews"]["points"].as_array().unwrap().len(), 5);
        let (_, no_pdf) = ask(&router, "GET", "/api/papers/2601.00002/claims", None).await;
        assert_eq!(
            (no_pdf["paper"]["state"].clone(), no_pdf["tree"]["state"].clone()),
            (json!("no_pdf"), json!("no_text"))
        );
        assert_eq!(no_pdf["estimate"], Value::Null);
        assert_eq!(ask(&router, "GET", "/api/papers/nowhere/claims", None).await.0, StatusCode::NOT_FOUND);
        let long = "x".repeat(LONGEST_ID + 1);
        assert_eq!(ask(&router, "GET", &format!("/api/papers/{long}/claims"), None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn starting_is_refused_without_a_key_a_budget_a_pdf_or_a_paper() {
        let (keyless, _keyless_data) = engine(false);
        let keyless = router(keyless);
        let path = format!("/api/papers/{PAPER}/claims");
        let budget = |usd: f64| Some(json!({"budget_usd": usd}).to_string());
        assert_eq!(
            ask(&keyless, "POST", &path, budget(0.5)).await.0,
            StatusCode::BAD_REQUEST,
            "no key: nothing can be asked"
        );
        let (keyed, _data) = engine(true);
        let router = router(keyed);
        for refused in [0.0, -1.0, MOST_BUDGET_USD + 1.0] {
            assert_eq!(ask(&router, "POST", &path, budget(refused)).await.0, StatusCode::BAD_REQUEST, "{refused}");
        }
        assert_eq!(ask(&router, "POST", &path, Some("{\"budget\": 1}".to_owned())).await.0, StatusCode::BAD_REQUEST);
        let (status, said) = ask(&router, "POST", "/api/papers/2601.00002/claims", budget(0.5)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(said["error"].as_str().unwrap().contains("PDF"));
        assert_eq!(ask(&router, "POST", "/api/papers/nowhere/claims", budget(0.5)).await.0, StatusCode::NOT_FOUND);
        let huge = Some(format!("{{\"budget_usd\": 1, \"pad\": \"{}\"}}", "x".repeat(5 * 1024 * 1024)));
        assert_eq!(ask(&router, "POST", &path, huge).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_second_job_while_one_runs_is_busy() {
        let (engine, _data) = engine(true);
        let engine: Shared = Arc::new(engine);
        let (release, wait) = mpsc::channel::<()>();
        engine
            .job
            .start("a run", move |_| wait.recv().map(|()| String::new()).map_err(|error| error.to_string()))
            .unwrap();
        let refused =
            start(State(Arc::clone(&engine)), Path(PAPER.to_owned()), Ok(Json(Start { budget_usd: 0.5 }))).await;
        assert!(matches!(refused, Err(ApiError::Busy(_))), "{:?}", refused.err());
        release.send(()).unwrap();
    }

    #[tokio::test]
    async fn a_started_job_is_tracked_and_a_redo_forgets_what_was_kept() {
        let (engine, _data) = engine(true);
        let router = router(engine);
        let path = format!("/api/papers/{PAPER}/claims");
        let (status, started) = ask(&router, "POST", &path, Some(json!({"budget_usd": 0.5}).to_string())).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert!(started["job"].as_u64().unwrap() >= 1);
        let (_, status) = ask(&router, "GET", "/api/status", None).await;
        assert_eq!(status["kind"], "reading a paper's claims");
        let (status, forgotten) = ask(&router, "DELETE", &path, None).await;
        assert_eq!((status, forgotten["forgotten"].clone()), (StatusCode::OK, json!(0)));
        assert_eq!(ask(&router, "DELETE", "/api/papers/nowhere/claims", None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_community_says_how_many_claims_of_each_mapped_paper_were_contested() {
        let (engine, _data) = engine(false);
        let router = router(engine);
        let (status, contested) = ask(&router, "GET", "/api/runs/r1/communities/c0/claims/contested", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (contested["papers"].clone(), contested["papers_with_reviews"].clone(), contested["papers_mapped"].clone()),
            (json!(2), json!(1), json!(0))
        );
        assert_eq!(
            ask(&router, "GET", "/api/runs/r1/communities/c9/claims/contested", None).await.0,
            StatusCode::NOT_FOUND
        );
    }
}
