//! The AI-text detector ([`crate::detector`]): its state and what was measured about it, what building it would
//! cost, building it, the year check, and a paper's verdict.
//!
//! ```text
//! GET  /api/detector            `DetectorInfo`: the state (off, untrained, building n of m, ready, stale) and why,
//!                               what was measured on held-out papers and which models wrote what it learnt
//! GET  /api/detector/estimate   `DetectorEstimate`: what building would ask and cost with the chosen model; nothing
//!                               is sent
//! POST /api/detector/build      {provider?, model?, budget_usd? | budget_tokens?}: build the training set and fit
//!                               the classifier, as the one job (progress and stop in /api/status), within the
//!                               budget given (by default the estimate's worst case); `{outcome: "started", job}`.
//!                               Refused when `provider` and `model` (those whose estimate was shown) are no longer
//!                               the chosen ones. Once built, building again asks nothing and fits it again
//! POST /api/detector/years      the year check, as the job: a week of arXiv in each year scored; arXiv only, free
//! GET  /api/papers/{id}/ai      `AiVerdict | null`: the paper's score, the threshold and the words that raised it
//! ```
//!
//! The detector is switched on and off by the `detect_ai` setting (`PUT /api/settings`), as before.
//! [`library_part`] is what `GET /api/library` tells as `ai_detector` and `ai_validation`.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get, post};
use serde::{Deserialize, Serialize};

use super::routes::{ApiError, Engine, Shared, on_the_side};
use crate::detector::classifier::{Measured, Round};
use crate::detector::training::{self, Building, Estimate, KIND, MOST_TOKENS, MOST_USD, Sizes};
use crate::detector::{self, AiVerdict, Availability, DetectorError, Told, Validation, availability, generator_name};
use crate::jobs::{Finished, Phase};
use crate::library::Library;
use crate::model::chosen::{Accepted, Chosen, Terms, budget};
use crate::model::language::{Budget, Limit, Provider, Unavailable};
use crate::settings;
use crate::sources::Http;

/// The longest paper id taken: longer is no paper's.
const LONGEST_ID: usize = 300;
/// What the year check's job is called.
const YEARS_KIND: &str = "checking the AI-text detector over the years";

pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/detector", get(info)),
        ("/api/detector/estimate", get(estimate)),
        ("/api/detector/build", post(build)),
        ("/api/detector/years", post(years)),
        ("/api/papers/{id}/ai", get(paper)),
    ]
}

impl From<DetectorError> for ApiError {
    fn from(error: DetectorError) -> Self {
        match error {
            DetectorError::Library(error) => error.into(),
            DetectorError::Unreadable(why) => {
                eprintln!("detector: {why}");
                ApiError::Files
            }
        }
    }
}

/// The build job under way, if it is: its generations done of how many.
fn building(engine: &Engine) -> Option<(u64, u64)> {
    let job = engine.job.snapshot(usize::MAX);
    (job.state == Phase::Running && job.kind == KIND)
        .then(|| job.progress.map_or((0, 0), |progress| (progress.done, progress.of)))
}

fn enabled(engine: &Engine) -> Result<bool, ApiError> {
    Ok(settings::load(&engine.settings_file(), &engine.choices)?.detect_ai)
}

/// What building would cost with the chosen model, as a sentence's part: "about USD 3.10 (at most USD 9.40) with
/// gpt-6.2-sol of OpenAI". None when it cannot be estimated in dollars: then no cost is named.
fn cost(library: &Library, chosen: &Chosen) -> Option<String> {
    let estimate = training::estimate(library, chosen, Sizes::RECIPE).ok()?;
    let (usd, most) = (estimate.cost.cost.usd?, estimate.cost.cost.most_usd?);
    Some(format!("about USD {:.2} (at most USD {:.2}) with {chosen}", usd.dollars(), most.dollars()))
}

/// The state, and the classifier when there is one.
fn state(engine: &Engine, library: &Library) -> Result<(Availability, Option<Arc<detector::Trained>>), ApiError> {
    let trained = detector::model(library)?;
    let state = availability(enabled(engine)?, building(engine), trained.as_deref());
    Ok((state, trained))
}

/// What `GET /api/library` tells of the detector: `ai_detector` (the state and why) and `ai_validation` (what was
/// measured, once there is a classifier).
pub fn library_part(engine: &Engine) -> Result<(Told, Option<Validation>), ApiError> {
    let library = engine.library()?;
    let (state, trained) = state(engine, &library)?;
    let cost = match state {
        Availability::Untrained => engine.chosen().ok().and_then(|chosen| cost(&library, &chosen)),
        _ => None,
    };
    let told = Told { state: state.name(), why: state.why(cost.as_deref()) };
    let by_year = library.detector_built()?.1;
    Ok((told, trained.map(|trained| Validation::of(&trained, by_year))))
}

/// Which models wrote the mirrors trained on, and how many.
#[derive(Serialize)]
pub struct WrittenBy {
    /// As shown: "claude-sonnet-5-5 (Anthropic)".
    pub name: String,
    pub provider: Option<String>,
    pub model: String,
    pub mirrors: usize,
}

/// What was measured about the classifier, in full (`DetectorReport` in the contracts).
#[derive(Serialize)]
pub struct Report {
    pub trained_at: String,
    pub written_by: Vec<WrittenBy>,
    pub trained_on: usize,
    pub held_out: usize,
    pub false_alarms: f64,
    pub measured: Measured,
    pub rounds: Vec<Round>,
    pub machine_words: Vec<String>,
    pub human_words: Vec<String>,
}

/// `DetectorInfo` in the contracts.
#[derive(Serialize)]
pub struct Info {
    #[serde(flatten)]
    pub state: Availability,
    pub why: String,
    pub validation: Option<Validation>,
    pub report: Option<Report>,
}

async fn info(State(engine): State<Shared>) -> Result<Json<Info>, ApiError> {
    let told = on_the_side(move || {
        let (Told { why, .. }, validation) = library_part(&engine)?;
        let library = engine.library()?;
        let (state, trained) = state(&engine, &library)?;
        let report = trained.map(|trained| Report {
            trained_at: trained.trained_at.clone(),
            written_by: trained
                .report
                .written_by
                .iter()
                .map(|by| WrittenBy {
                    name: generator_name(by.provider.as_deref(), &by.model),
                    provider: by.provider.clone(),
                    model: by.model.clone(),
                    mirrors: by.mirrors,
                })
                .collect(),
            trained_on: trained.report.trained_on,
            held_out: trained.report.held_out,
            false_alarms: trained.report.false_alarms,
            measured: trained.report.measured.clone(),
            rounds: trained.report.rounds.clone(),
            machine_words: trained.report.machine_words.clone(),
            human_words: trained.report.human_words.clone(),
        });
        Ok(Info { state, why, validation, report })
    })
    .await?;
    Ok(Json(told))
}

/// `DetectorEstimate` in the contracts.
#[derive(Serialize)]
pub struct Estimated {
    pub provider: Provider,
    pub model: String,
    /// Whether the model can be asked at all (its provider's key is set); never the key.
    pub api_key_set: bool,
    #[serde(flatten)]
    pub estimate: Estimate,
}

async fn estimate(State(engine): State<Shared>) -> Result<Json<Estimated>, ApiError> {
    let told = on_the_side(move || {
        let library = engine.library()?;
        let chosen = engine.chosen()?;
        let estimate = training::estimate(&library, &chosen, Sizes::RECIPE)?;
        Ok(Estimated {
            provider: chosen.provider(),
            model: chosen.model().to_owned(),
            api_key_set: chosen.can_ask(),
            estimate,
        })
    })
    .await?;
    Ok(Json(told))
}

/// What a POST came to.
#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Started {
    Started { job: u64 },
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    budget_usd: Option<f64>,
    budget_tokens: Option<u64>,
    /// The model whose estimate the user saw, and its provider.
    model: Option<String>,
    provider: Option<Provider>,
}

fn body_of(body: &Bytes) -> Result<Start, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Start::default());
    }
    let not_an_object = || ApiError::Invalid("the request body must be a JSON object of the budget".to_owned());
    match serde_json::from_slice(body).map_err(|_| not_an_object())? {
        object @ serde_json::Value::Object(_) => serde_json::from_value(object).map_err(|_| not_an_object()),
        _ => Err(not_an_object()),
    }
}

/// arXiv's pace: a real wait.
fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

async fn build(State(engine): State<Shared>, body: Bytes) -> Result<Json<Started>, ApiError> {
    let asked = body_of(&body)?;
    let started = on_the_side(move || {
        let library = engine.library()?;
        let chosen = engine.chosen()?;
        let estimate = training::estimate(&library, &chosen, Sizes::RECIPE)?;
        // the model is asked only when there is something to ask: a built set is fitted again, with no key needed
        let endpoint = chosen.endpoint();
        if let (Err(why), true) = (&endpoint, estimate.to_generate > 0) {
            return Err(ApiError::Invalid(format!("the model cannot be asked: {why}")));
        }
        if let Some(model) = &asked.model {
            chosen.as_shown(asked.provider, model).map_err(|why| ApiError::Invalid(why.to_string()))?;
        }
        let invalid = |why: crate::model::chosen::Refused| ApiError::Invalid(why.to_string());
        let accepted = Accepted::of(asked.budget_usd, asked.budget_tokens).map_err(invalid)?;
        let budget = budget(&estimate.cost.cost, accepted, Terms::AtMost { usd: MOST_USD, tokens: MOST_TOKENS })
            .map_err(invalid)?;
        let transport = engine.transport.clone();
        let job = engine.job.start_tracked(KIND, move |tracker| {
            let fetch = Http::new();
            let building = Building {
                library: &library,
                fetch: &fetch,
                pause: &sleep,
                chosen: &chosen,
                transport: &*transport,
                endpoint,
                budget: &budget,
                sizes: Sizes::RECIPE,
            };
            match training::build(&building, tracker).map_err(|error| error.to_string())? {
                training::Built::Trained { generated, trained_on } => {
                    tracker.log(&format!("Done: {generated} abstracts written now; trained on {trained_on}."));
                }
                training::Built::Stopped { generated, why } => {
                    tracker.log(&format!(
                        "Stopped ({why}) after {generated} abstracts written now. Everything written is kept: \
                         building again asks only for the rest."
                    ));
                }
            }
            Ok(Finished::Nothing)
        })?;
        Ok(Started::Started { job })
    })
    .await?;
    Ok(Json(started))
}

async fn years(State(engine): State<Shared>, body: Bytes) -> Result<Json<Started>, ApiError> {
    if !body.iter().all(u8::is_ascii_whitespace) && body.as_ref() != b"{}" {
        return Err(ApiError::Invalid("the year check takes no request body".to_owned()));
    }
    let started = on_the_side(move || {
        let library = engine.library()?;
        let Some(trained) = detector::model(&library)? else {
            return Err(ApiError::Invalid("no AI-text detector has been trained yet".to_owned()));
        };
        let chosen = engine.chosen()?;
        let transport = engine.transport.clone();
        let job = engine.job.start_tracked(YEARS_KIND, move |tracker| {
            let fetch = Http::new();
            // the year check asks arXiv only: this budget of nothing is never drawn on
            let nothing = Budget::new(Limit::Tokens(0));
            let building = Building {
                library: &library,
                fetch: &fetch,
                pause: &sleep,
                chosen: &chosen,
                transport: &*transport,
                endpoint: Err(Unavailable::NoKey),
                budget: &nothing,
                sizes: Sizes::RECIPE,
            };
            training::check_years(&building, &trained.classifier, tracker).map_err(|error| error.to_string())?;
            Ok(Finished::Nothing)
        })?;
        Ok(Started::Started { job })
    })
    .await?;
    Ok(Json(started))
}

async fn paper(State(engine): State<Shared>, Path(id): Path<String>) -> Result<Json<Option<AiVerdict>>, ApiError> {
    if id.is_empty() || id.len() > LONGEST_ID {
        return Err(ApiError::NotFound);
    }
    let told = on_the_side(move || {
        let library = engine.library()?;
        let paper = library.paper(&id)?.ok_or(ApiError::NotFound)?;
        Ok(detector::verdict(&library, &paper)?)
    })
    .await?;
    Ok(Json(told))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;

    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::super::config::tests::STRONG;
    use super::super::config::{Environment, Listen};
    use super::super::other_providers::{Chosen as Asked, MODEL, Scripted, call, choices, finished, router};
    use super::super::tests::served;
    use super::*;
    use crate::detector::tests::{HUMAN, MACHINE, build_in, text};
    use crate::library::Known;
    use crate::library::papers::tests::paper as record;

    const CONTRACTS: &str = include_str!("../../../../packages/contracts/src/index.ts");

    /// The fields of a TypeScript object type in the contracts, at its first level: the block that starts at
    /// `marker` (`export interface AiVerdict {`, or a field's `ai_validation: {`).
    fn fields(marker: &str) -> BTreeSet<String> {
        let start = CONTRACTS.find(marker).unwrap_or_else(|| panic!("{marker} is not in the contracts"));
        let block = &CONTRACTS[start + marker.len()..];
        // the block's first level, its comments left out, cut into members at `;` and line ends
        let (mut depth, mut comment, mut member, mut members) = (1i32, false, String::new(), Vec::new());
        let mut chars = block.chars().peekable();
        while let Some(c) = chars.next() {
            if comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    comment = false;
                }
                continue;
            }
            match c {
                '/' if chars.peek() == Some(&'*') => comment = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        members.push(std::mem::take(&mut member));
                        break;
                    }
                }
                ';' | '\n' if depth == 1 => members.push(std::mem::take(&mut member)),
                _ if depth == 1 => member.push(c),
                _ => {}
            }
        }
        members
            .iter()
            .filter_map(|member| member.split_once(':'))
            .map(|(name, _)| name.trim().trim_end_matches('?').to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    }

    fn keys(value: &Value) -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    /// Every field the engine sends is one the contract names, and every field the contract requires is sent.
    fn as_contracted(value: &Value, marker: &str, optional: &[&str]) {
        let contract = fields(marker);
        let sent = keys(value);
        assert!(sent.is_subset(&contract), "{marker}: sent {sent:?}, contract {contract:?}");
        let missing: Vec<&String> =
            contract.difference(&sent).filter(|name| !optional.contains(&name.as_str())).collect();
        assert!(missing.is_empty(), "{marker}: {missing:?} not sent");
    }

    fn data_with_papers() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let mut papers = vec![record("h"), record("m")];
        papers[0].r#abstract = text(HUMAN, 1);
        papers[1].r#abstract = text(MACHINE, 1);
        library.add_papers(&papers, None, Known::Keep).unwrap();
        data
    }

    fn quiet() -> Arc<Scripted> {
        Scripted::new(|_| json!({"abstract": text(MACHINE, 3)}).to_string())
    }

    #[tokio::test]
    async fn an_untrained_detector_says_why_and_what_building_would_cost_and_sends_nothing() {
        let data = data_with_papers();
        let transport = quiet();
        let router = router(data.path(), Asked::OpenAi, &transport);
        let (status, told) = call(&router, "GET", "/api/detector", None).await;
        assert_eq!(status, StatusCode::OK, "{told}");
        assert_eq!(
            (told["state"].clone(), told["validation"].clone(), told["report"].clone()),
            (json!("untrained"), Value::Null, Value::Null)
        );
        let why = told["why"].as_str().unwrap();
        assert!(why.contains("USD") && why.contains(MODEL) && why.contains("graphics card"), "{why}");
        as_contracted(&told, "export interface DetectorInfo {", &["done", "of", "trained_at"]);

        let (status, estimate) = call(&router, "GET", "/api/detector/estimate", None).await;
        assert_eq!(status, StatusCode::OK, "{estimate}");
        assert_eq!(
            (estimate["to_generate"].clone(), estimate["pool_kept"].clone(), estimate["built"].clone()),
            (json!(2000), json!(0), json!(false))
        );
        assert_eq!((estimate["provider"].clone(), estimate["model"].clone()), (json!("openai"), json!(MODEL)));
        assert!(estimate["cost"]["usd"].as_f64().unwrap() > 0.0);
        as_contracted(&estimate, "export interface DetectorEstimate {", &[]);
        assert!(transport.sent().is_empty(), "nothing was sent");

        let (_, verdict) = call(&router, "GET", "/api/papers/m/ai", None).await;
        assert_eq!(verdict, Value::Null, "untrained: no verdict");
    }

    #[tokio::test]
    async fn the_switch_turns_it_off_and_back_through_the_settings() {
        let data = data_with_papers();
        let router = router(data.path(), Asked::OpenAi, &quiet());
        let (status, _) = call(&router, "PUT", "/api/settings", Some(json!({"detect_ai": false}))).await;
        assert_eq!(status, StatusCode::OK);
        let (_, told) = call(&router, "GET", "/api/detector", None).await;
        assert_eq!(told["state"], "off");
        assert!(told["why"].as_str().unwrap().starts_with("Switched off"));
        call(&router, "PUT", "/api/settings", Some(json!({"detect_ai": true}))).await;
        assert_eq!(call(&router, "GET", "/api/detector", None).await.1["state"], "untrained");
    }

    #[tokio::test]
    async fn a_trained_detector_tells_what_it_was_measured_at_and_scores_a_paper_with_its_words() {
        let data = data_with_papers();
        build_in(&Library::open(&data.path().join("library")).unwrap());
        let transport = quiet();
        let router = router(data.path(), Asked::OpenAi, &transport);
        let (_, told) = call(&router, "GET", "/api/detector", None).await;
        assert_eq!(told["state"], "ready", "{told}");
        assert!(told["trained_at"].is_string());
        assert_eq!(told["validation"]["generators"], json!(["gpt-6.2-sol (OpenAI)"]));
        assert_eq!(told["report"]["written_by"][0]["mirrors"], 120);
        as_contracted(&told, "export interface DetectorInfo {", &["done", "of"]);
        as_contracted(&told["validation"], "ai_validation: {", &[]);
        as_contracted(&told["report"], "report: {", &[]);

        // scored by a run (the pipeline's one call), then shown with what raised it
        let library = Library::open(&data.path().join("library")).unwrap();
        let (log, progress, cancel) = (|_: &str| {}, |_: crate::jobs::Progress| {}, crate::jobs::Cancel::new());
        let ids = vec!["h".to_owned(), "m".to_owned()];
        let tracker = crate::jobs::Tracker::new(&log, &progress, &cancel);
        let scores = detector::score_run(&library, &crate::settings::Settings::default(), &ids, &tracker);
        assert_eq!(scores.flags(), [false, true]);
        let (status, verdict) = call(&router, "GET", "/api/papers/m/ai", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(verdict["flagged"], true);
        assert!(!verdict["markers"].as_array().unwrap().is_empty());
        as_contracted(&verdict, "export interface AiVerdict {", &[]);
        assert_eq!(call(&router, "GET", "/api/papers/h/ai", None).await.1["flagged"], false);
        assert_eq!(call(&router, "GET", "/api/papers/nowhere/ai", None).await.0, StatusCode::NOT_FOUND);
        let long = format!("/api/papers/{}/ai", "x".repeat(LONGEST_ID + 1));
        assert_eq!(call(&router, "GET", &long, None).await.0, StatusCode::NOT_FOUND);

        // what /api/library tells of it
        let engine = Engine::new(data.path().to_path_buf(), choices());
        let (told, validation) = library_part(&engine).unwrap();
        let told = serde_json::to_value(&told).unwrap();
        assert_eq!(told["state"], "ready");
        as_contracted(&told, "ai_detector: {", &[]);
        assert!(validation.is_some());

        // building again asks nothing: the classifier is fitted again, as the job
        let (_, estimate) = call(&router, "GET", "/api/detector/estimate", None).await;
        assert_eq!((estimate["to_generate"].clone(), estimate["built"].clone()), (json!(0), json!(true)));
        let shown = json!({"provider": "openai", "model": MODEL});
        let (status, started) = call(&router, "POST", "/api/detector/build", Some(shown)).await;
        assert_eq!((status, started["outcome"].clone()), (StatusCode::OK, json!("started")), "{started}");
        let done = finished(&router, &started["job"]).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["outcome"], json!({"kind": "nothing"}));
        assert!(transport.sent().is_empty(), "nothing was asked of the model");
    }

    #[tokio::test]
    async fn a_build_is_refused_without_a_key_for_another_model_than_shown_over_its_cap_or_without_an_object() {
        let data = data_with_papers();
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let (no_key, _) = served(&listen, "127.0.0.1:8765", Engine::new(data.path().to_path_buf(), choices()));
        let (status, told) = call(&no_key, "POST", "/api/detector/build", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("cannot be asked"), "{told}");

        let transport = quiet();
        let router = router(data.path(), Asked::OpenAi, &transport);
        for body in [
            json!({"provider": "openai", "model": "another-model"}),
            json!({"budget_usd": MOST_USD + 1.0}),
            json!({"budget_usd": 0}),
            json!({"budget_usd": 1, "budget_tokens": 5}),
            json!([1]),
            json!({"unknown": true}),
        ] {
            let (status, told) = call(&router, "POST", "/api/detector/build", Some(body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {told}");
        }
        let (status, _) = call(&router, "POST", "/api/detector/years", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "nothing trained to check");
        assert!(transport.sent().is_empty());
    }
}
