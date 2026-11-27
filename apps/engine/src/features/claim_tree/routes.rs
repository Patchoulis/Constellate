//! A paper's claims as a tree with its reviews mapped onto them ([`crate::features::claim_tree`]), and how many
//! of a community's papers' claims reviewers contested.
//!
//! ```text
//! GET    /api/papers/{id}/claims    what is kept, checked again, and what asking the rest would cost (`ClaimsView`)
//! POST   /api/papers/{id}/claims    {provider, model, budget_usd | budget_tokens}: ask what is not kept, as the
//!                                   job, within that budget (in tokens for a model with no price); answers {job}
//! DELETE /api/papers/{id}/claims    forget the tree and mapping the GET shows, of the chosen model (a redo);
//!                                   another model's stay; answers {forgotten}
//! GET    /api/runs/{run}/communities/{community}/claims/contested   per paper, its contested claims (`Contested`)
//! ```
//!
//! The GET asks nothing: the interface shows its estimate, and the user starts the work with a click (the POST),
//! capped at the budget the user agreed to. The model is the one chosen in the settings, of its provider. A start
//! is refused when it names no model or another than the one chosen, and when the worst case of asking now is
//! more than the budget (the reviews were read, or the PDF changed, since the estimate was shown).

use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get};
use serde::{Deserialize, Serialize};

use crate::api::reviews::papers_of;
use crate::api::routes::{ApiError, Shared, on_the_side};
use crate::features::claim_tree::asking::{Asked, Askers, AskingError, ask};
use crate::features::claim_tree::{self, ClaimsView, Contested};
use crate::jobs::{Busy, Finished, Phase};
use crate::library::{Feature, PaperReading, Subject};
use crate::model::chosen::{Accepted, Terms, budget};
use crate::model::language::{Estimate, Provider, Transport};

/// The most one paper's two calls may be allowed to spend, in US dollars: far above what they take (a few cents
/// to a dollar), so that a mistyped budget is refused rather than spent.
pub const MOST_BUDGET_USD: f64 = 10.0;
/// The same for a model with no price, in tokens.
pub const MOST_BUDGET_TOKENS: u64 = 5_000_000;
/// What the job is called in `/api/status`.
const KIND: &str = "reading a paper's claims";
/// A paper id longer than this is no paper's.
const LONGEST_ID: usize = 200;
/// A hundredth of a cent, in dollars: the rounding of a figure on its way to the interface and back.
const SLACK_USD: f64 = 1e-4;
const NO_MODEL: &str = "name the model whose estimate was shown (provider and model)";
const GREW: &str = "asking this paper's claims would now cost more than the budget given (its reviews were read, or \
its PDF changed, since the estimate was shown): look at the new estimate before asking";

/// Does what the user accepted cover the worst case of asking now? Not when the estimate grew after it was shown:
/// the paper's reviews were read since (the mapping's call is added), its PDF was replaced (more text is sent).
/// The budget alone would only stop the second call part way, after the first was paid for.
fn covers(cost: &Estimate, accepted: Accepted) -> bool {
    if cost.calls == 0 {
        return true;
    }
    match (cost.most_usd, accepted) {
        (Some(most), Accepted::Dollars(usd)) => most.dollars() <= usd + SLACK_USD,
        (None, Accepted::Tokens(tokens)) => cost.most_tokens <= tokens,
        _ => false,
    }
}

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/claims", get(read).post(start).delete(forget)),
        ("/api/runs/{run}/communities/{community}/claims/contested", get(community_contested)),
    ]
}

/// `ClaimsStart` in the contracts.
#[derive(Deserialize)]
pub struct Start {
    /// The most the calls may spend, in US dollars, or in tokens for a model with no price: what the user agreed
    /// to after seeing the estimate.
    #[serde(alias = "accepted_usd")]
    budget_usd: Option<f64>,
    #[serde(default)]
    #[serde(alias = "accepted_tokens")]
    budget_tokens: Option<u64>,
    /// The model whose estimate was shown, and its provider.
    #[serde(default)]
    provider: Option<Provider>,
    #[serde(default)]
    model: Option<String>,
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

pub async fn read(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<ClaimsView>, ApiError> {
    paper_id(&id)?;
    let view = on_the_side(move || {
        let chosen = engine.chosen()?;
        claim_tree::view(&*engine.library()?, &id, &chosen)?.ok_or(ApiError::NotFound)
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
    let Json(asked) = body?;
    let accepted = match Accepted::of(asked.budget_usd, asked.budget_tokens) {
        Ok(Accepted::Shown) => return Err(ApiError::Invalid("give the budget the calls may spend".to_owned())),
        Ok(accepted) => accepted,
        Err(why) => return Err(ApiError::Invalid(why.to_string())),
    };
    let transport = Arc::clone(&engine.transport);
    let job = on_the_side(move || begin(&engine, id, &asked, accepted, transport)).await?;
    Ok(Json(Started { job }))
}

/// Start the job that asks what is not kept about paper `id` within what was `accepted`, over `transport` (the
/// network, or the tests' fake). Answers the job's number.
fn begin(
    engine: &Shared,
    id: String,
    asked: &Start,
    accepted: Accepted,
    transport: Arc<dyn Transport>,
) -> Result<u64, ApiError> {
    {
        let library = engine.library()?;
        let chosen = engine.chosen()?;
        let endpoint = chosen.endpoint().map_err(|why| ApiError::Invalid(why.to_string()))?;
        // the estimate the budget was accepted on is of one model: a request that does not say which is refused
        let model = asked.model.as_deref().ok_or_else(|| ApiError::Invalid(NO_MODEL.to_owned()))?;
        chosen.as_shown(asked.provider, model).map_err(|why| ApiError::Invalid(why.to_string()))?;
        match library.paper_texts(&[id.as_str()])?.pop() {
            Some(PaperReading::Read(_)) => {}
            Some(PaperReading::NotInLibrary) | None => return Err(ApiError::NotFound),
            Some(PaperReading::NoPdf { .. } | PaperReading::Unreadable { .. }) => {
                return Err(ApiError::Invalid(AskingError::NoPdf.to_string()));
            }
        }
        // in dollars for a model with a price, in tokens for one without, as the estimate was shown
        let view = claim_tree::view(&library, &id, &chosen)?.ok_or(ApiError::NotFound)?;
        let cost = view.estimate().map(|estimate| estimate.cost.clone()).ok_or(ApiError::NotFound)?;
        let terms = Terms::AtMost { usd: MOST_BUDGET_USD, tokens: MOST_BUDGET_TOKENS };
        let budget = budget(&cost, accepted, terms).map_err(|why| ApiError::Invalid(why.to_string()))?;
        if !covers(&cost, accepted) {
            return Err(ApiError::Invalid(GREW.to_owned()));
        }
        let about = crate::jobs::About { feature: Feature::ClaimTree, subject: Subject::Paper(id.clone()), item: None };
        let job = engine.job.start_about(KIND, about, move |tracker| {
            let askers = Askers { library: &library, transport: &*transport, endpoint, recorder: &*library };
            let done = ask(&askers, &id, &chosen, &budget, tracker).map_err(|error| error.to_string())?;
            tracker.log(&format!("The claim tree: {}", told(&done.tree)));
            tracker.log(&format!("The reviews mapped onto it: {}", told(&done.mapping)));
            // a call that was asked and gave nothing usable, or was not started, fails the job: the interface
            // then says why, rather than offering the same button again as if nothing had happened
            for (what, asked) in [("The claim tree", &done.tree), ("The reviews' mapping", &done.mapping)] {
                if matches!(asked, Asked::Failed(_) | Asked::NotStarted(_)) {
                    return Err(format!("{what}: {}", told(asked)));
                }
            }
            Ok(Finished::Feature { feature: Feature::ClaimTree, subject: Subject::Paper(id) })
        })?;
        Ok(job)
    }
}

pub async fn forget(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Forgotten>, ApiError> {
    paper_id(&id)?;
    let forgotten = on_the_side(move || {
        // forgotten while the paper's claims are being asked, the answer would be kept again just after
        let job = engine.job.snapshot(usize::MAX);
        if job.state == Phase::Running && job.kind == KIND {
            return Err(ApiError::Busy(Busy { kind: KIND.to_owned() }));
        }
        let library = engine.library()?;
        library.paper(&id)?.ok_or(ApiError::NotFound)?;
        // what the view shows, of the model chosen now: another model's tree of the paper stays
        let chosen = engine.chosen()?;
        Ok(claim_tree::forget(&library, &id, &chosen)?)
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
        let chosen = engine.chosen()?;
        Ok(claim_tree::contested(&*engine.library()?, &papers, &chosen)?)
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

    use super::*;
    use crate::api::config::tests::STRONG;
    use crate::api::config::{Environment, Listen};
    use crate::api::routes::Engine;
    use crate::api::tests::served;
    use crate::features::claim_tree::asking::tests::{
        FORUM, LINES, PAPER, keep_reviews, mapping_answer, reply, tree_answer,
    };
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::tests::paper;
    use crate::library::{Known, Library};
    use crate::model::language::fake::Fake;
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

    /// The engine with a library of a paper with a PDF and reviews, one without a PDF, and a run of both.
    fn engine(language: bool) -> (Engine, tempfile::TempDir) {
        engine_with(language, true)
    }

    /// [`engine`], the reviews kept or not.
    fn engine_with(language: bool, reviews: bool) -> (Engine, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        {
            let library = Library::open(&data.path().join("library")).unwrap();
            let mut with_pdf = paper(PAPER);
            with_pdf.url = Some(FORUM.to_owned());
            library.add_papers(&[with_pdf, paper("2601.00002")], None, Known::Keep).unwrap();
            library.keep_pdf(PAPER, &pdf_with(&LINES)).unwrap();
            if reviews {
                keep_reviews(&library);
            }
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
            engine.set_claude_key("test-key-not-real", Some("http://127.0.0.1:9"));
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

    #[test]
    fn a_start_takes_the_accepted_amount_under_either_name() {
        let named = |body: serde_json::Value| serde_json::from_value::<super::Start>(body).unwrap();
        let old = named(serde_json::json!({"budget_usd": 1.5}));
        let new = named(serde_json::json!({"accepted_usd": 1.5}));
        assert_eq!((old.budget_usd, new.budget_usd), (Some(1.5), Some(1.5)));
        assert_eq!(named(serde_json::json!({"accepted_tokens": 9})).budget_tokens, Some(9));
    }

    #[tokio::test]
    async fn another_provider_is_priced_asked_in_its_own_shape_kept_and_asked_again_when_the_provider_changes() {
        use crate::api::other_providers::{
            BUDGET, Chosen as Asked, MODEL, Scripted, accepting, call, choose, finished, priced_for, router, sent_to,
        };

        let (_, data) = engine(false);
        let (tree, mapping) = (tree_answer().to_string(), mapping_answer().to_string());
        let transport =
            Scripted::new(
                move |body: &str| if body.contains("TASK: the claim tree") { tree.clone() } else { mapping.clone() },
            );
        let router = router(data.path(), Asked::OpenAi, &transport);
        let path = format!("/api/papers/{PAPER}/claims");
        let mut sent = 0;
        for chosen in [Asked::OpenAi, Asked::ThisComputer] {
            choose(data.path(), chosen);
            let (_, view) = call(&router, "GET", &path, None).await;
            assert_eq!(
                (view["provider"].clone(), view["model"].clone()),
                (json!(chosen.provider().id()), json!(MODEL)),
                "{view}"
            );
            assert_eq!(view["tree"]["state"], "not_asked");
            let cost = view["estimate"].clone();
            assert_eq!(cost["to_ask"], 2);
            priced_for(&cost, chosen);
            assert_eq!(transport.sent().len(), sent, "a view sends nothing");

            let (status, started) = call(&router, "POST", &path, Some(accepting(&cost, BUDGET))).await;
            assert_eq!(status, StatusCode::OK, "{started}");
            let done = finished(&router, &started["job"]).await;
            assert_eq!(done["state"], "done", "{done}");
            let made = json!({"kind": "feature", "feature": "claim_tree", "subject": {"paper": PAPER}});
            assert_eq!((done["outcome"].clone(), done["result"].clone()), (made, Value::Null), "no run to open");
            let all = transport.sent();
            assert_eq!(all.len() - sent, 2, "the tree, then the reviews' mapping");
            sent_to(&all[sent..], chosen);
            sent = all.len();

            let (_, view) = call(&router, "GET", &path, None).await;
            assert_eq!(
                (view["tree"]["state"].clone(), view["mapping"]["state"].clone()),
                (json!("made"), json!("made")),
                "{view}"
            );
            assert_eq!(view["tree"]["nodes"][0]["evidence"][0]["established"]["kind"], "verified");
            assert_eq!(view["estimate"]["to_ask"], 0, "kept: nothing left to ask");
            let enough = match chosen {
                Asked::OpenAi => json!({"budget_usd": 1.0, "provider": "openai", "model": MODEL}),
                Asked::ThisComputer => json!({"budget_tokens": 100_000, "provider": "compatible", "model": MODEL}),
            };
            let (_, started) = call(&router, "POST", &path, Some(enough)).await;
            assert_eq!(finished(&router, &started["job"]).await["state"], "done");
            assert_eq!(transport.sent().len(), sent, "kept: not asked again");
        }
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
        assert_eq!(view["estimate"]["counting"]["kind"], "assumed");
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
        let budget = |usd: f64| {
            Some(json!({"budget_usd": usd, "provider": "anthropic", "model": "claude-opus-5-5"}).to_string())
        };
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
        // no model named: the budget was accepted on no one model's estimate
        let (status, said) = ask(&router, "POST", &path, Some(json!({"budget_usd": 0.5}).to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(said["error"].as_str().unwrap().contains("model"), "{said}");
        let another = Some(json!({"budget_usd": 0.5, "provider": "anthropic", "model": "claude-haiku-5"}).to_string());
        assert_eq!(ask(&router, "POST", &path, another).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_budget_accepted_before_the_reviews_were_read_is_refused_once_the_mapping_adds_to_it() {
        let (engine, data) = engine_with(true, false);
        let router = router(engine);
        let path = format!("/api/papers/{PAPER}/claims");
        let (_, before) = ask(&router, "GET", &path, None).await;
        assert_eq!(before["estimate"]["to_ask"], 1, "the tree alone");
        let shown = before["estimate"]["most_usd"].as_f64().unwrap();
        // the interface's budget: the worst case shown, up to the cent
        let accepted = (shown * 100.0).ceil() / 100.0;
        keep_reviews(&Library::open(&data.path().join("library")).unwrap());
        let (_, after) = ask(&router, "GET", &path, None).await;
        assert_eq!(after["estimate"]["to_ask"], 2);
        assert!(after["estimate"]["most_usd"].as_f64().unwrap() > accepted, "{after}");
        let start = json!({"budget_usd": accepted, "provider": "anthropic", "model": "claude-opus-5-5"});
        let (status, said) = ask(&router, "POST", &path, Some(start.to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{said}");
        assert!(said["error"].as_str().unwrap().contains("new estimate"), "{said}");
    }

    #[tokio::test]
    async fn a_second_job_while_one_runs_is_busy() {
        let (engine, _data) = engine(true);
        let engine: Shared = Arc::new(engine);
        let (release, wait) = mpsc::channel::<()>();
        engine
            .job
            .start("a run", move |_| {
                wait.recv().map(|()| crate::jobs::Finished::Nothing).map_err(|error| error.to_string())
            })
            .unwrap();
        let refused = start(State(Arc::clone(&engine)), Path(PAPER.to_owned()), Ok(Json(enough()))).await;
        assert!(matches!(refused, Err(ApiError::Busy(_))), "{:?}", refused.err());
        release.send(()).unwrap();
    }

    /// A start that accepts two dollars (more than the worst case), for the model the settings choose (of the Claude API).
    fn enough() -> Start {
        Start { budget_usd: Some(2.0), budget_tokens: None, provider: None, model: Some("claude-opus-5-5".to_owned()) }
    }

    /// Wait for the job to end, and say how it ended.
    fn ended(engine: &Shared) -> crate::jobs::Snapshot<crate::jobs::Finished> {
        for _ in 0..500 {
            let job = engine.job.snapshot(0);
            if job.state != Phase::Running {
                return job;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the job did not end");
    }

    #[tokio::test]
    async fn a_started_job_keeps_both_answers_and_a_redo_forgets_them_but_not_while_asking() {
        let (engine, _data) = engine(true);
        let engine: Shared = Arc::new(engine);
        let answers = vec![reply(&tree_answer()), reply(&mapping_answer())];
        let job =
            begin(&engine, PAPER.to_owned(), &enough(), Accepted::Dollars(2.0), Arc::new(Fake::new(answers))).unwrap();
        let done = ended(&engine);
        assert_eq!((done.id, done.kind.as_str(), done.state), (job, KIND, Phase::Done), "{:?}", done.log);
        let Json(view) = read(State(Arc::clone(&engine)), Path(PAPER.to_owned())).await.unwrap();
        assert!(matches!(view.tree(), crate::features::claim_tree::TreeState::Made { .. }));
        assert!(matches!(view.mapping(), crate::features::claim_tree::MappingState::Made { .. }));

        let (release, wait) = mpsc::channel::<()>();
        engine
            .job
            .start(KIND, move |_| {
                wait.recv().map(|()| crate::jobs::Finished::Nothing).map_err(|error| error.to_string())
            })
            .unwrap();
        let refused = forget(State(Arc::clone(&engine)), Path(PAPER.to_owned())).await;
        assert!(matches!(refused, Err(ApiError::Busy(_))));
        release.send(()).unwrap();
        ended(&engine);
        let Json(forgotten) = forget(State(Arc::clone(&engine)), Path(PAPER.to_owned())).await.unwrap();
        assert_eq!(forgotten.forgotten, 2);
        assert!(matches!(forget(State(engine), Path("nowhere".to_owned())).await, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn a_job_whose_call_gave_nothing_usable_fails_and_says_why() {
        let (engine, _data) = engine(true);
        let engine: Shared = Arc::new(engine);
        let job = begin(
            &engine,
            PAPER.to_owned(),
            &enough(),
            Accepted::Dollars(2.0),
            Arc::new(Fake::new(vec![reply(&json!({"not": "a tree"}))])),
        )
        .unwrap();
        let done = ended(&engine);
        assert_eq!((done.id, done.state), (job, Phase::Failed));
        assert!(done.error.unwrap().starts_with("The claim tree: no usable answer"));
    }

    /// Keep, in the engine's library under `data`, the tree and the mapping of the model the settings choose, as a
    /// job would have.
    fn keep_both(data: &std::path::Path) {
        use crate::features::claim_tree::asking::{MAPPING_PROMPT, TREE_PROMPT, checked_tree, mapping_key, tree_key};
        use crate::features::claim_tree::mapping::{self, SaidMapping};
        use crate::features::claim_tree::points::review_points;
        use crate::features::claim_tree::tree::SaidTree;
        use crate::library::Slot;
        use crate::model::chosen::Chosen;

        let library = Library::open(&data.join("library")).unwrap();
        let model = "claude-opus-5-5";
        let chosen = Chosen::claude(model);
        let slot = |key| Slot { feature: Feature::ClaimTree, key, subject: Subject::Paper(PAPER.to_owned()) };
        let said: SaidTree = serde_json::from_value(tree_answer()).unwrap();
        library.keep(&slot(tree_key(&chosen, PAPER)), model, TREE_PROMPT, &said).unwrap();
        let Some(PaperReading::Read(passages)) = library.paper_texts(&[PAPER]).unwrap().pop() else { panic!() };
        let tree = checked_tree(&said, PAPER, &passages).unwrap();
        let points = review_points(&library.reviews(PAPER).unwrap().unwrap().discussion).points;
        let question = mapping::question(&tree, &points);
        let said: SaidMapping = serde_json::from_value(mapping_answer()).unwrap();
        library.keep(&slot(mapping_key(&chosen, PAPER, &question)), model, MAPPING_PROMPT, &said).unwrap();
    }

    #[tokio::test]
    async fn every_answer_of_the_claims_routes_fits_its_type_in_the_contracts() {
        use crate::api::contract::assert_fits;

        let (engine, data) = engine(false);
        let router = router(engine);
        let path = format!("/api/papers/{PAPER}/claims");
        let (_, not_asked) = ask(&router, "GET", &path, None).await;
        assert_eq!(not_asked["tree"]["state"], "not_asked");
        assert_fits("ClaimsView", &not_asked, &[]);
        let (_, no_pdf) = ask(&router, "GET", "/api/papers/2601.00002/claims", None).await;
        assert_eq!(no_pdf["tree"]["state"], "no_text");
        assert_fits("ClaimsView", &no_pdf, &[]);
        keep_both(data.path());
        let (_, made) = ask(&router, "GET", &path, None).await;
        assert_eq!((made["tree"]["state"].clone(), made["mapping"]["state"].clone()), (json!("made"), json!("made")));
        assert!(made["node_reviews"].as_array().is_some_and(|nodes| nodes.len() > 1), "{made}");
        assert_fits("ClaimsView", &made, &[]);
        let (_, contested) = ask(&router, "GET", "/api/runs/r1/communities/c0/claims/contested", None).await;
        assert_eq!(contested["papers_mapped"], 1, "{contested}");
        assert_fits("ContestedClaims", &contested, &[]);
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
