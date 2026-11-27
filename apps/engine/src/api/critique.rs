//! A paper's weaknesses ([`crate::analysis::critique`]): the reviewers' points from the reviews page the user
//! opened, and a model's reading of the paper, asked for only on a click once its estimate is shown.
//!
//! ```text
//! GET    /api/papers/{id}/critique   the reviewers' part and the model's part (`Critique`): the reading kept, or
//!                                    what asking would cost (nothing is sent), or why it cannot be asked
//! POST   /api/papers/{id}/critique   {model?, budget_usd?}: ask, as the one job (progress and stop in /api/status),
//!                                    refused when `model` (the one whose estimate was shown) is no longer chosen;
//!                                    `{outcome: "started", job}`, or `{outcome: "kept"}` when it was read before.
//!                                    The budget is the estimate's worst case unless a lower or higher one is given
//! DELETE /api/papers/{id}/critique   forget the model's reading (a redo): `{forgotten}`
//! ```
//!
//! Every quote of a kept reading is checked again against the paper's text when it is served: a kept answer is
//! the model's words, never evidence.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get};
use serde::{Deserialize, Serialize};

use super::routes::{ApiError, Shared, on_the_side};
use crate::analysis::critique::{Plan, Reading, ReviewersPart, read, reviewers};
use crate::evidence::{PaperText, Papers};
use crate::jobs::Tracker;
use crate::library::{Feature, Library, PaperReading, Subject};
use crate::model::batch::{Asking, BatchEstimate, IN_FLIGHT, Outcome, Stopped};
use crate::model::language::{Budget, DEFAULT_MODEL, Endpoint, Https, Limit, Transport};
use crate::settings;
use crate::usage::Usd;

/// The most one paper's reading may be allowed to cost, in US dollars, whatever is asked: one call, however long
/// the paper, costs well under this.
pub const MOST_USD: f64 = 5.0;
/// The longest paper id taken: longer is no paper's.
const LONGEST_ID: usize = 300;

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![("/api/papers/{id}/critique", get(view).post(start).delete(redo))]
}

/// The model's part of a paper's weaknesses.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelPart {
    /// The library has no PDF of the paper: only the reviewers' part can be shown.
    NoPdf,
    /// Its PDF could not be read.
    Unreadable,
    /// Not asked yet (or asked for a question since changed, such as new reviews): what asking would cost.
    NotAsked {
        /// The model chosen in the settings, which would be asked.
        model: String,
        estimate: BatchEstimate,
        /// Whether the model can be asked at all (a key is set); never the key.
        api_key_set: bool,
        points_asked: usize,
        /// Where the paper's text would be cut, in characters, when it is too long to send whole.
        cut: Option<usize>,
    },
    /// The reading kept, its quotes checked again now.
    Answered {
        /// The model that answered, as the API named it.
        model: String,
        kept_at: String,
        reading: Reading,
        points_asked: usize,
        cut: Option<usize>,
    },
}

/// `Critique` in the contracts.
#[derive(Serialize)]
pub struct Critique {
    pub paper_id: String,
    pub reviewers: ReviewersPart,
    pub model: ModelPart,
}

/// What a POST came to.
#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Started {
    Started {
        job: u64,
    },
    /// Read before: nothing was asked.
    Kept,
}

#[derive(Default, Deserialize)]
struct Start {
    budget_usd: Option<f64>,
    /// The model whose estimate the user saw.
    model: Option<String>,
}

#[derive(Serialize)]
struct Forgotten {
    forgotten: usize,
}

/// A paper as both parts start from it.
struct Paper {
    library: Arc<Library>,
    id: String,
    reviewers: ReviewersPart,
    /// The plan when the paper's PDF was read, with its passages; else what its text came to.
    text: Result<(Plan, PaperText), ModelPart>,
}

fn known(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > LONGEST_ID { Err(ApiError::NotFound) } else { Ok(()) }
}

fn paper(engine: &Shared, id: &str) -> Result<Paper, ApiError> {
    known(id)?;
    let library = engine.library()?;
    library.paper(id)?.ok_or(ApiError::NotFound)?;
    let kept = library.reviews(id)?;
    let page = library.review_page(id)?;
    let model = settings::load(&engine.settings_file(), &engine.choices)?.describe_model;
    let model = if model.is_empty() { DEFAULT_MODEL.to_owned() } else { model };
    let text = match library.paper_texts(&[id])?.pop() {
        Some(PaperReading::Read(passages)) => {
            let plan = Plan::new(id, &model, &passages, kept.as_ref().map(|kept| &kept.discussion));
            Ok((plan, PaperText::Read(passages)))
        }
        Some(PaperReading::NoPdf { .. }) => Err(ModelPart::NoPdf),
        Some(PaperReading::Unreadable { why, .. }) => {
            eprintln!("critique: the PDF of {id} cannot be read: {why}");
            Err(ModelPart::Unreadable)
        }
        Some(PaperReading::NotInLibrary) | None => return Err(ApiError::NotFound),
    };
    let reviewers = kept.map_or(ReviewersPart::NoneKept, |kept| reviewers(&kept, page.as_ref()));
    Ok(Paper { library, id: id.to_owned(), reviewers, text })
}

async fn view(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Critique>, ApiError> {
    let critique = on_the_side(move || {
        let api_key_set = engine.endpoint().is_ok();
        let Paper { library, id, reviewers, text } = paper(&engine, &id)?;
        let model = match text {
            Err(part) => part,
            Ok((plan, text)) => match plan.kept(&library)? {
                Some(kept) => {
                    let ids: Vec<String> = plan.points.iter().map(|point| point.id.clone()).collect();
                    let papers = Papers::from([(id.clone(), text)]);
                    ModelPart::Answered {
                        model: kept.model,
                        kept_at: kept.kept_at,
                        reading: read(&kept.answer, &id, &papers, &ids),
                        points_asked: ids.len(),
                        cut: plan.cut,
                    }
                }
                None => ModelPart::NotAsked {
                    estimate: plan.estimate(&library)?,
                    model: plan.model,
                    api_key_set,
                    points_asked: plan.points.len(),
                    cut: plan.cut,
                },
            },
        };
        Ok(Critique { paper_id: id, reviewers, model })
    })
    .await?;
    Ok(Json(critique))
}

/// The budget of one paper's call: as asked, or the estimate's worst case; in tokens when the model has no price.
fn budget_of(asked: Option<f64>, estimate: &BatchEstimate) -> Result<Budget, ApiError> {
    if let Some(usd) = asked {
        if !(usd.is_finite() && usd > 0.0) {
            return Err(ApiError::Invalid("the budget must be more than nothing".to_owned()));
        }
        if usd > MOST_USD {
            return Err(ApiError::Invalid(format!("the budget is too large: at most USD {MOST_USD} for one paper")));
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // positive and at most MOST_USD
        let picodollars = (usd * 1e12).round() as u64;
        return Ok(Budget::new(Limit::Dollars(Usd::from_picodollars(picodollars))));
    }
    Ok(match estimate.most_usd {
        Some(most) => Budget::new(Limit::Dollars(most)),
        // a model with no known price: its worst case in tokens (the prompt written to the cache, a whole answer)
        None => {
            let worst =
                estimate.by_model.iter().map(|model| model.tokens.input + model.tokens.cache_write_5m).sum::<u64>();
            Budget::new(Limit::Tokens(worst + u64::from(crate::analysis::critique::MAX_TOKENS)))
        }
    })
}

async fn start(State(engine): State<Shared>, Path(id): Path<String>, body: Bytes) -> Result<Json<Started>, ApiError> {
    let asked: Start = if body.iter().all(u8::is_ascii_whitespace) {
        Start::default()
    } else {
        // an object only: serde would read a list as the fields in order
        let not_an_object = || ApiError::Invalid("the request body must be a JSON object".to_owned());
        match serde_json::from_slice(&body).map_err(|_| not_an_object())? {
            object @ serde_json::Value::Object(_) => serde_json::from_value(object).map_err(|_| not_an_object())?,
            _ => return Err(not_an_object()),
        }
    };
    let started = on_the_side(move || {
        let Paper { library, text, .. } = paper(&engine, &id)?;
        let Ok((plan, _)) = text else {
            return Err(ApiError::Invalid("the library has no readable PDF of this paper".to_owned()));
        };
        if plan.kept(&library)?.is_some() {
            return Ok(Started::Kept);
        }
        let endpoint =
            engine.endpoint().map_err(|why| ApiError::Invalid(format!("the model cannot be asked: {why}")))?;
        // the estimate the user saw was for this model: one changed in the settings since is not asked unseen
        if asked.model.as_ref().is_some_and(|model| *model != plan.model) {
            return Err(ApiError::Invalid(format!(
                "the model chosen in the settings is now {}: look at its estimate before asking",
                plan.model
            )));
        }
        let budget = budget_of(asked.budget_usd, &plan.estimate(&library)?)?;
        let job = engine.job.start_tracked("a paper's weaknesses", move |tracker| {
            work(&library, &Https::default(), endpoint, &plan, &budget, tracker)
        })?;
        Ok(Started::Started { job })
    })
    .await?;
    Ok(Json(started))
}

/// The job: ask about one paper through `transport`, and say what came of it.
fn work(
    library: &Library,
    transport: &dyn Transport,
    endpoint: Endpoint,
    plan: &Plan,
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<String, String> {
    let asking = Asking { library, transport, endpoint, recorder: library, in_flight: IN_FLIGHT };
    match plan.ask(asking, budget, tracker).map_err(|error| error.to_string())? {
        Outcome::Answered(_) => Ok("The model read the paper for weaknesses.".to_owned()),
        Outcome::Kept(_) => Ok("The paper was read for weaknesses before: nothing was asked.".to_owned()),
        Outcome::Failed(why) => Err(why.to_string()),
        Outcome::NotStarted(Stopped::Cancelled) => Ok("Stopped before the model was asked.".to_owned()),
        Outcome::NotStarted(why) => Err(why.to_string()),
    }
}

async fn redo(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Forgotten>, ApiError> {
    let forgotten = on_the_side(move || {
        known(&id)?;
        let library = engine.library()?;
        library.paper(&id)?.ok_or(ApiError::NotFound)?;
        Ok(library.forget(Feature::Critique, &[Subject::Paper(id)])?)
    })
    .await?;
    Ok(Json(Forgotten { forgotten }))
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
    use super::*;
    use crate::library::papers::tests::paper as record;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Known, Ratings};
    use crate::settings::Choices;
    use crate::text::discussion::fixtures::ICLR_2024;
    use crate::text::discussion::read_discussion;
    use crate::text::reviews::read_ratings;

    const AT: &str = "127.0.0.1:8765";
    const FORUM: &str = "https://openreview.net/forum?id=abc123XYZ";
    const LINES: [&str; 3] = [
        "Introduction",
        "We propose an imaginary optimiser and prove that it converges on every smooth problem.",
        "We test it on two small models only and leave larger ones to future work.",
    ];

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

    /// Papers with a PDF and reviews, with a PDF only, and with reviews only.
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let mut papers = vec![record("iclr-2024-1"), record("2601.00002"), record("iclr-2024-3")];
        papers[0].url = Some(FORUM.to_owned());
        papers[2].url = Some(FORUM.to_owned());
        library.add_papers(&papers, None, Known::Keep).unwrap();
        library.keep_pdf("iclr-2024-1", &pdf_with(&LINES)).unwrap();
        library.keep_pdf("2601.00002", &pdf_with(&LINES)).unwrap();
        let ratings = read_ratings(ICLR_2024).map(|read| Ratings::from(&read));
        for id in ["iclr-2024-1", "iclr-2024-3"] {
            library.keep_reviews(id, FORUM, ICLR_2024, &read_discussion(ICLR_2024).unwrap(), ratings.clone()).unwrap();
        }
        data
    }

    fn listen() -> Listen {
        Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap()
    }

    /// An engine with a key for the model, whose address leads nowhere on this computer: should anything ever be
    /// sent by mistake, it reaches no provider.
    fn with_key(data: &tempfile::TempDir) -> Engine {
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        engine.language = Environment::from_pairs([
            ("ANTHROPIC_API_KEY", "test-key-not-real-0123456789"),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:9"),
        ])
        .language_endpoint();
        assert!(engine.language.is_ok());
        engine
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
    async fn a_paper_with_reviews_and_a_pdf_shows_the_reviewers_points_and_what_asking_would_cost() {
        let data = data();
        let (router, _) = served(&listen(), AT, Engine::new(data.path().to_path_buf(), choices()));
        let (status, told) = ask(&router, "GET", "/api/papers/iclr-2024-1/critique", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(told["reviewers"]["state"], "read");
        assert_eq!(told["reviewers"]["reviewers"].as_array().unwrap().len(), 2);
        assert_eq!(
            told["reviewers"]["reviewers"][0]["points"][0]["text"],
            "The experiments are limited to small models."
        );
        assert_eq!(told["reviewers"]["reviewers"][0]["rating"]["value"], 8);
        let model = &told["model"];
        assert_eq!(
            (model["state"].clone(), model["model"].clone(), model["api_key_set"].clone()),
            (json!("not_asked"), json!("claude-opus-5-5"), json!(false))
        );
        assert_eq!((model["estimate"]["to_ask"].clone(), model["points_asked"].clone()), (json!(1), json!(3)));
        assert!(model["estimate"]["usd"].as_f64().unwrap() > 0.0);
        assert_eq!(model["estimate"]["by_model"][0]["counting"]["kind"], "assumed");
    }

    #[tokio::test]
    async fn a_paper_without_reviews_gets_the_models_part_only_and_one_without_a_pdf_the_reviewers_only() {
        let data = data();
        let (router, _) = served(&listen(), AT, Engine::new(data.path().to_path_buf(), choices()));
        let (_, alone) = ask(&router, "GET", "/api/papers/2601.00002/critique", None).await;
        assert_eq!(
            (alone["reviewers"]["state"].clone(), alone["model"]["state"].clone()),
            (json!("none_kept"), json!("not_asked"))
        );
        assert_eq!(alone["model"]["points_asked"], 0);
        let (_, unread) = ask(&router, "GET", "/api/papers/iclr-2024-3/critique", None).await;
        assert_eq!(
            (unread["reviewers"]["state"].clone(), unread["model"]["state"].clone()),
            (json!("read"), json!("no_pdf"))
        );
        let (router, _) = served(&listen(), AT, with_key(&data));
        let (status, refused) = ask(&router, "POST", "/api/papers/iclr-2024-3/critique", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }

    #[tokio::test]
    async fn an_unknown_paper_an_overlong_id_no_key_a_budget_too_large_and_a_busy_engine_are_refused() {
        let data = data();
        let (router, _) = served(&listen(), AT, Engine::new(data.path().to_path_buf(), choices()));
        for method in ["GET", "POST", "DELETE"] {
            assert_eq!(ask(&router, method, "/api/papers/nowhere/critique", None).await.0, StatusCode::NOT_FOUND);
            let long = format!("/api/papers/{}/critique", "x".repeat(LONGEST_ID + 1));
            assert_eq!(ask(&router, method, &long, None).await.0, StatusCode::NOT_FOUND);
        }
        let (status, told) = ask(&router, "POST", "/api/papers/iclr-2024-1/critique", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("No API key"));

        let engine = with_key(&data);
        let (router, _) = served(&listen(), AT, with_key(&data));
        for budget in [
            json!({"budget_usd": MOST_USD + 1.0}),
            json!({"budget_usd": 0}),
            json!([1]),
            json!({"model": "another-model"}),
        ] {
            let (status, _) = ask(&router, "POST", "/api/papers/iclr-2024-1/critique", Some(budget.to_string())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{budget}");
        }
        // another job is running: nothing is started, and so nothing is sent
        let running = engine
            .job
            .start_tracked("a run", |tracker| {
                while !tracker.cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok("stopped".to_owned())
            })
            .unwrap();
        let job = engine.job.clone();
        let (router, _) = served(&listen(), AT, engine);
        let (status, told) = ask(&router, "POST", "/api/papers/iclr-2024-1/critique", None).await;
        assert_eq!((status, told["error"].clone()), (StatusCode::CONFLICT, json!("a run is already in progress")));
        job.cancel(running).unwrap();
    }

    #[tokio::test]
    async fn a_kept_reading_is_served_checked_again_not_asked_again_and_forgotten_by_a_redo() {
        let data = data();
        {
            // the answer kept as a job would have kept it, for the plan the routes make
            let library = Library::open(&data.path().join("library")).unwrap();
            let Some(PaperReading::Read(passages)) = library.paper_texts(&["iclr-2024-1"]).unwrap().pop() else {
                panic!()
            };
            let discussion = library.reviews("iclr-2024-1").unwrap().unwrap().discussion;
            let plan = Plan::new("iclr-2024-1", "claude-opus-5-5", &passages, Some(&discussion));
            let answer = json!({
                "points": [
                    {"point": "p1", "located": "passage", "quote": "We test it on two small models only and leave larger ones to future work", "paper_says": ""},
                    {"point": "p2", "located": "passage", "quote": "We test it on ten large models and report every seed", "paper_says": ""}
                ],
                "weaknesses": [{"kind": "claim_broader_than_evidence", "weakness": "Convergence is claimed for every smooth problem.", "quote": "prove that it converges on every smooth problem", "absent_terms": []}]
            });
            library.keep(&plan.slot(), "claude-opus-5-5", crate::analysis::critique::PROMPT, &answer).unwrap();
        }
        let (router, _) = served(&listen(), AT, with_key(&data));
        let (status, told) = ask(&router, "GET", "/api/papers/iclr-2024-1/critique", None).await;
        assert_eq!(status, StatusCode::OK);
        let model = &told["model"];
        assert_eq!(model["state"], "answered");
        assert_eq!(model["reading"]["points"].as_array().unwrap().len(), 1);
        assert_eq!(model["reading"]["points"][0]["location"]["evidence"]["established"]["kind"], "verified");
        assert_eq!(model["reading"]["left_out"]["not_in_paper"], 1);
        assert_eq!(model["reading"]["weaknesses"][0]["absence"]["kind"], "not_claimed");
        assert_eq!(ask(&router, "POST", "/api/papers/iclr-2024-1/critique", None).await.1, json!({"outcome": "kept"}));
        assert_eq!(ask(&router, "DELETE", "/api/papers/iclr-2024-1/critique", None).await.1, json!({"forgotten": 1}));
        assert_eq!(
            ask(&router, "GET", "/api/papers/iclr-2024-1/critique", None).await.1["model"]["state"],
            "not_asked"
        );
    }

    #[test]
    fn the_job_asks_through_the_transport_it_is_given_keeps_the_answer_and_says_what_came_of_it() {
        use crate::jobs::{Cancel, Progress};
        use crate::model::language::fake::{Fake, message, ok};

        let data = data();
        let library = Library::open(&data.path().join("library")).unwrap();
        let Some(PaperReading::Read(passages)) = library.paper_texts(&["2601.00002"]).unwrap().pop() else { panic!() };
        let plan = Plan::new("2601.00002", "claude-opus-5-5", &passages, None);
        let endpoint = || with_key(&data).language.unwrap();
        let budget = || budget_of(None, &plan.estimate(&library).unwrap()).unwrap();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});

        let cancel = Cancel::new();
        cancel.cancel();
        let stopped = Fake::new(Vec::new());
        let told = work(&library, &stopped, endpoint(), &plan, &budget(), &Tracker::new(&log, &progress, &cancel));
        assert_eq!(told, Ok("Stopped before the model was asked.".to_owned()));
        assert!(stopped.sent().is_empty());

        let answer = json!({"points": [], "weaknesses": []}).to_string();
        let usage = json!({"input_tokens": 100, "output_tokens": 50});
        let fake = Fake::new(vec![Ok(ok(&message("claude-opus-5-5", &answer, "end_turn", usage)))]);
        let going = Cancel::new();
        let told = work(&library, &fake, endpoint(), &plan, &budget(), &Tracker::new(&log, &progress, &going));
        assert_eq!(told, Ok("The model read the paper for weaknesses.".to_owned()));
        assert_eq!(fake.sent().len(), 1);
        assert!(plan.kept(&library).unwrap().is_some());
        let again = work(&library, &fake, endpoint(), &plan, &budget(), &Tracker::new(&log, &progress, &going));
        assert_eq!(again, Ok("The paper was read for weaknesses before: nothing was asked.".to_owned()));
        assert_eq!(fake.sent().len(), 1);
    }
}
