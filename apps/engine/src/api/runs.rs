//! The runs: making one, listing them, serving one, renaming and deleting; and what the run form offers
//! (the catalogue) and what the library holds.
//!
//! ```text
//! POST   /api/runs               start a run (RunRequest): a selection, or `rerun` an earlier run; `redo` steps;
//!                                `describe`: what was accepted for naming it. One that would make an existing run
//!                                again is refused (409, `{error, replaces: RunReplaced}`) unless `replace: true`
//! GET    /api/runs               every run, the most recently made first (RunSummary[]); with any of
//!                                ?offset=&limit=&sort=&search= one page of them (RunPage)
//! GET    /api/runs/{name}        its communities (RunResult), as its communities.json has them
//! GET    /api/runs/{name}/graph  its papers and links (GraphData), as its graph.json has them
//! PATCH  /api/runs/{name}        {title}: show it under another title; its name stays
//! DELETE /api/runs/{name}        delete it (the library's papers stay)
//! GET    /api/catalog            what the run form and the settings offer (Catalog)
//! GET    /api/library            what the library holds (Library)
//! ```

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};
use serde_json::{Value, json};

use super::routes::{ApiError, Shared, on_the_side, one_name};
use crate::analysis::graph::cluster::Method;
use crate::features::writing::describe;
use crate::jobs::Finished;
use crate::model::chosen::Accepted;
use crate::model::embed::Device;
use crate::model::language::Provider;
use crate::pipeline::results::files::{self, Replaced, RunFileError};
use crate::pipeline::served::{ServeError, Which};
use crate::pipeline::steps::gather::Sources;
use crate::pipeline::steps::seams::Naming;
use crate::pipeline::{self, Context, Redo, RedoStep, Request};
use crate::runs::index;
use crate::selection::Selection;
use crate::settings;
use crate::sources::conferences::acceptance::Acceptance;
use crate::text::declarations::Flag;

/// The most conferences or categories one run may name.
pub const MOST_PICKED: usize = 100;
/// The longest a conference's or category's name may be, in characters.
pub const LONGEST_PICK: usize = 200;
/// The longest a search of the list may be, in characters.
pub const LONGEST_SEARCH: usize = 200;
/// The longest title a request may send (it is cut to [`files::LONGEST_TITLE`]).
pub const LONGEST_TITLE_SENT: usize = 10_000;

pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/runs", get(list).post(start)),
        ("/api/runs/{name}", get(communities).patch(rename).delete(remove)),
        ("/api/runs/{name}/graph", get(graph)),
        ("/api/catalog", get(catalog)),
        ("/api/library", get(library)),
    ]
}

impl From<ServeError> for ApiError {
    fn from(error: ServeError) -> Self {
        match error {
            ServeError::NotFound => ApiError::NotFound,
            ServeError::BeingMade(name) => ApiError::Busy(crate::jobs::Busy { kind: format!("making {name}") }),
            ServeError::Library(error) => error.into(),
            ServeError::Files(error) => {
                eprintln!("runs: {error}");
                ApiError::Files
            }
        }
    }
}

impl From<RunFileError> for ApiError {
    fn from(error: RunFileError) -> Self {
        ServeError::from(error).into()
    }
}

/// The strings of a list in the request, each checked for size.
fn strings(body: &Value, key: &str) -> Result<Vec<String>, ApiError> {
    let Some(value) = body.get(key).filter(|value| !value.is_null()) else { return Ok(Vec::new()) };
    let list = value.as_array().ok_or_else(|| ApiError::Invalid(format!("`{key}` must be a list")))?;
    if list.len() > MOST_PICKED {
        return Err(ApiError::Invalid(format!("at most {MOST_PICKED} {key} can be asked for at once")));
    }
    list.iter()
        .map(|item| match item.as_str() {
            Some(text) if text.chars().count() <= LONGEST_PICK => Ok(text.to_owned()),
            Some(_) => Err(ApiError::Invalid(format!("each of `{key}` must be at most {LONGEST_PICK} characters"))),
            None => Err(ApiError::Invalid(format!("each of `{key}` must be a name"))),
        })
        .collect()
}

fn text<'a>(body: &'a Value, key: &str) -> Result<Option<&'a str>, ApiError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.len() <= LONGEST_PICK => Ok(Some(text)),
        Some(_) => Err(ApiError::Invalid(format!("`{key}` must be a date, YYYY-MM-DD"))),
    }
}

/// What a run request asks for: what to make, and what the person accepted for naming its communities.
pub struct Asked {
    pub request: Request,
    pub naming: Naming,
}

/// Why a run request starts nothing.
pub enum NotStarted {
    Refused(ApiError),
    /// It would make an existing run again in its folder, and `replace` was not sent: what would be replaced, to
    /// be shown before the person confirms.
    Replaces(Box<Replaced>),
}

impl From<ApiError> for NotStarted {
    fn from(error: ApiError) -> Self {
        NotStarted::Refused(error)
    }
}

impl From<RunFileError> for NotStarted {
    fn from(error: RunFileError) -> Self {
        NotStarted::Refused(error.into())
    }
}

/// The `describe` of a run request: the model whose estimate was shown, and what was accepted of it.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeAccepted {
    provider: Option<Provider>,
    model: String,
    accepted_usd: Option<f64>,
    accepted_tokens: Option<u64>,
}

/// What naming the run may spend: what `describe` accepted of `GET /api/describe/estimate`, checked now so that a
/// refusal is told before the run's work; with none, nothing is sent and only names kept before are given.
fn naming(engine: &Shared, body: &Value, redo: &Redo) -> Result<Naming, ApiError> {
    let invalid = |why: &dyn std::fmt::Display| ApiError::Invalid(why.to_string());
    let Some(sent) = body.get("describe").filter(|value| !value.is_null()) else {
        if redo.contains(&RedoStep::Descriptions) {
            return Err(ApiError::Invalid(
                "describing the communities again asks the language model: send what was accepted of its estimate \
                 (`describe`)"
                    .to_owned(),
            ));
        }
        return Ok(match engine.chosen() {
            Ok(chosen) => Naming::KeptOnly(chosen),
            Err(why) => Naming::Unknown(why.to_string()),
        });
    };
    let accepted: DescribeAccepted = serde_json::from_value(sent.clone())
        .map_err(|_| invalid(&"`describe` must be {provider, model, accepted_usd | accepted_tokens}"))?;
    if accepted.model.chars().count() > LONGEST_PICK {
        return Err(invalid(&"`describe.model` is too long"));
    }
    let chosen = engine.chosen()?;
    chosen.as_shown(accepted.provider, &accepted.model).map_err(|why| invalid(&why))?;
    chosen.endpoint().map_err(|why| invalid(&why))?;
    let settings = settings::load(&engine.settings_file(), &engine.choices)?;
    let terms = Accepted::of(accepted.accepted_usd, accepted.accepted_tokens).map_err(|why| invalid(&why))?;
    let budget = describe::budget_before_run(&chosen, &settings, terms).map_err(|why| invalid(&why))?;
    Ok(Naming::Within { chosen, budget })
}

/// What a run request asks for: a selection (or an earlier run's, for `rerun`), the steps to redo, and what may be
/// spent naming its communities. A request that would make an existing run again in its folder (a `rerun`, or the
/// same selection anew) is not started unless it says `replace: true`: the person is first told what would be
/// replaced ([`NotStarted::Replaces`]), since its communities may be cut otherwise and its names, and whatever was
/// asked about its communities, would no longer apply.
pub fn request(engine: &Shared, body: &Value) -> Result<Asked, NotStarted> {
    if !body.is_object() {
        return Err(ApiError::Invalid("the request body must be a JSON object".to_owned()).into());
    }
    let redo: Redo = strings(body, "redo")?
        .iter()
        .map(|step| step.parse::<RedoStep>().map_err(ApiError::Invalid))
        .collect::<Result<_, _>>()?;
    let selection = match body.get("rerun").filter(|value| !value.is_null()) {
        Some(Value::String(name)) if !name.is_empty() => {
            let folder = files::folder_of(&engine.data, one_name(name)?)?;
            files::stored_selection(&folder)?
        }
        Some(_) => return Err(ApiError::Invalid("`rerun` must be the name of a run".to_owned()).into()),
        None => Selection::parse(
            &strings(body, "venues")?,
            text(body, "start")?,
            text(body, "end")?,
            &strings(body, "categories")?,
            body.get("include_workshops").and_then(Value::as_bool).unwrap_or(false),
        )
        .map_err(|why| ApiError::Invalid(why.to_string()))?,
    };
    // the name becomes the run's folder: one that could not be listed, opened or deleted is refused now, not
    // after the run's work
    let name = selection.name();
    if one_name(&name).is_err() {
        return Err(ApiError::Invalid(
            "the run's name made of these conferences or categories is too long; pick fewer, or shorter".to_owned(),
        )
        .into());
    }
    let replace = match body.get("replace") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(replace)) => *replace,
        Some(_) => return Err(ApiError::Invalid("`replace` must be true or false".to_owned()).into()),
    };
    if !replace && let Ok(folder) = files::folder_of(&engine.data, &name) {
        let settings = settings::load(&engine.settings_file(), &engine.choices).map_err(ApiError::from)?;
        let now = serde_json::to_value(&settings).map_err(|_| ApiError::Internal)?;
        let replaced = files::replacing(&folder, &name, &now, pipeline::CLUSTERING_VERSION)?;
        return Err(NotStarted::Replaces(Box::new(replaced)));
    }
    let naming = naming(engine, body, &redo)?;
    Ok(Asked { request: Request { selection, redo }, naming })
}

async fn start(State(engine): State<Shared>, body: Result<Json<Value>, JsonRejection>) -> Result<Response, ApiError> {
    let Json(body) = body?;
    let asked = Arc::clone(&engine);
    let asked = match on_the_side(move || Ok(request(&asked, &body))).await? {
        Ok(asked) => asked,
        Err(NotStarted::Refused(error)) => return Err(error),
        Err(NotStarted::Replaces(replaced)) => {
            let error = format!(
                "{} is made already. Making it again replaces it: its communities {}, and their names and whatever \
                 was asked about them would no longer apply. Send the request again with `replace: true` to make it \
                 again.",
                replaced.title,
                match replaced.communities_will {
                    files::Remade::Change => "will be cut otherwise",
                    files::Remade::MayChange => "may be cut otherwise if its papers have changed",
                }
            );
            let told = json!({ "error": error, "replaces": replaced });
            return Ok((axum::http::StatusCode::CONFLICT, Json(told)).into_response());
        }
    };
    let working = Arc::clone(&engine);
    engine.job.start_tracked("a run", move |tracker| make(&working, &asked, tracker))?;
    Ok(Json(json!({ "ok": true })).into_response())
}

/// The job of a run: made with the network's sources and the engine's model, then listed.
fn make(engine: &Shared, asked: &Asked, tracker: &crate::jobs::Tracker<'_>) -> Result<Finished, String> {
    let request = &asked.request;
    let library = engine.library().map_err(|error| error.to_string())?;
    let settings = settings::load(&engine.settings_file(), &engine.choices).map_err(|error| error.to_string())?;
    let name = request.selection.name();
    let Some(making) = engine.runs.begin_making(&name) else {
        return Err("a run is already being made".to_owned());
    };
    let fetch = crate::sources::Http::new();
    let pause = |wait: Duration| std::thread::sleep(wait);
    let context = Context {
        data: &engine.data,
        library: &library,
        settings,
        sources: Sources {
            fetch: &fetch,
            pause: &pause,
            today: chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).date_naive(),
        },
        embedding: engine.runs.embedding.as_ref(),
        board: &engine.runs.board,
        transport: &*engine.transport,
        naming: &asked.naming,
    };
    let made = pipeline::make(&context, request, tracker);
    drop(making);
    let name = made.map_err(|error| error.to_string())?;
    engine.runs.saved(&engine.data, &library, &name).map_err(|error| error.to_string())?;
    Ok(Finished::Run { run: name })
}

async fn list(
    State(engine): State<Shared>,
    RawQuery(raw): RawQuery,
    query: Result<Query<index::Query>, QueryRejection>,
) -> Result<Response, ApiError> {
    if raw.as_deref().is_none_or(str::is_empty) {
        let all = on_the_side(move || Ok(engine.runs.all(&engine.data, &*engine.library()?)?)).await?;
        return Ok(Json(all).into_response());
    }
    let Query(query) = query?;
    if query.search.chars().count() > LONGEST_SEARCH {
        return Err(ApiError::Invalid(format!("a search is at most {LONGEST_SEARCH} characters")));
    }
    let page = on_the_side(move || Ok(engine.runs.page(&engine.data, &*engine.library()?, &query)?)).await?;
    Ok(Json(page).into_response())
}

/// A run's file as it is on disk: JSON, sent as it is.
fn as_json(text: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], Body::from(text)).into_response()
}

async fn served(engine: Shared, name: String, which: Which) -> Result<Response, ApiError> {
    one_name(&name)?;
    let run = on_the_side(move || Ok(engine.runs.open(&engine.data, &*engine.library()?, &name, which)?)).await?;
    Ok(as_json(match which {
        Which::Communities => run.communities.clone(),
        Which::Graph => run.graph.clone(),
    }))
}

async fn communities(State(engine): State<Shared>, Path(name): Path<String>) -> Result<Response, ApiError> {
    served(engine, name, Which::Communities).await
}

async fn graph(State(engine): State<Shared>, Path(name): Path<String>) -> Result<Response, ApiError> {
    served(engine, name, Which::Graph).await
}

async fn rename(
    State(engine): State<Shared>,
    Path(name): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    one_name(&name)?;
    let title = match body.get("title") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(title)) if title.len() <= LONGEST_TITLE_SENT => title.clone(),
        Some(Value::String(_)) => return Err(ApiError::Invalid("the title is too long".to_owned())),
        Some(_) => return Err(ApiError::Invalid("`title` must be text".to_owned())),
    };
    let named = name.clone();
    let shown = on_the_side(move || Ok(engine.runs.rename(&engine.data, &*engine.library()?, &named, &title)?)).await?;
    Ok(Json(json!({ "name": name, "title": shown })))
}

async fn remove(State(engine): State<Shared>, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    one_name(&name)?;
    on_the_side(move || Ok(engine.runs.delete(&engine.data, &*engine.library()?, &name)?)).await?;
    Ok(Json(json!({ "ok": true })))
}

/// What an acceptance filter lets through, in the conferences' own words (the Python's `catalog._includes`).
fn includes(tiers: &[Acceptance]) -> String {
    if tiers.contains(&Acceptance::Poster) {
        return "Every accepted paper, whatever its tier".to_owned();
    }
    let terms: Vec<String> = tiers
        .iter()
        .map(|tier| {
            let name = tier.name().unwrap_or("");
            let capital: String =
                name.chars().take(1).flat_map(char::to_uppercase).chain(name.chars().skip(1)).collect();
            format!("{capital}s: {}", tier.equivalents().join(", "))
        })
        .collect();
    format!("{} (each where its conference and year count it so)", terms.join("; "))
}

/// The embedding models offered, with a few words each (the Python's `catalog.EMBEDDING_MODELS`). Only the first
/// can embed papers in this version; the others can be chosen for a library embedded by them before.
const EMBEDDING_MODELS: [(&str, &str); 5] = [
    ("Qwen/Qwen3-Embedding-0.6B", "1024 dimensions; the default"),
    ("BAAI/bge-large-en-v1.5", "1024 dimensions, general purpose; only papers embedded by it before"),
    ("BAAI/bge-base-en-v1.5", "768 dimensions, general purpose, fast; only papers embedded by it before"),
    ("allenai/specter2_base", "768 dimensions, trained on scientific papers; only papers embedded by it before"),
    (
        "sentence-transformers/all-mpnet-base-v2",
        "768 dimensions, a long-standing baseline; only papers embedded by it before",
    ),
];

/// `Catalog` in the contracts.
pub fn catalog_json() -> Value {
    let lists = crate::sources::catalog::as_json();
    let acceptance: Vec<Value> = ["all", "highlights", "oral"]
        .iter()
        .map(|name| {
            let (label, tiers) = pipeline::steps::filter::acceptance_filter(name);
            json!({"name": name, "label": label, "note": includes(tiers)})
        })
        .collect();
    json!({
        "embedding_models": EMBEDDING_MODELS.iter().map(|(name, note)| json!({"name": name, "note": note})).collect::<Vec<_>>(),
        "describe_models": crate::model::language::MODELS
            .iter()
            .map(|model| json!({"name": model.id, "label": model.label, "note": model.note}))
            .collect::<Vec<_>>(),
        "clustering": Method::ALL
            .iter()
            .map(|method| json!({"name": method.id(), "label": method.label(), "note": method.note()}))
            .collect::<Vec<_>>(),
        "acceptance": acceptance,
        "flags": [{
            "field": "Leave out papers where",
            "items": Flag::ALL.iter().map(|flag| json!({"name": flag.id(), "label": flag.label(), "note": flag.note()})).collect::<Vec<_>>(),
        }],
        "conferences": lists["conferences"],
        "categories": lists["categories"],
    })
}

async fn catalog() -> Json<Value> {
    Json(catalog_json())
}

/// What the engine says of this computer's way of embedding (`Library.machine` in the contracts): how the model did
/// embed, once it has; until then, what this computer offers it ([`crate::machine::summary`]).
fn machine(device: Option<Device>) -> Value {
    match device {
        None => {
            let offered = crate::machine::summary();
            json!({ "gpu": offered.gpu, "embedding": offered.embedding })
        }
        loaded => machine_of(loaded),
    }
}

fn machine_of(device: Option<Device>) -> Value {
    match device {
        Some(Device::GraphicsCard) => json!({
            "gpu": "the graphics card",
            "embedding": "Papers are embedded on the graphics card: about a thousand papers a minute.",
        }),
        Some(Device::Processor) => json!({
            "gpu": null,
            "embedding": "No graphics card could be used, so papers are embedded on the processor. Everything works, \
                but embedding is many times slower: allow about a minute per hundred papers. Papers already embedded \
                are never embedded again.",
        }),
        None => json!({
            "gpu": null,
            "embedding": "Papers are embedded on any graphics card the computer has (DirectX 12 on Windows, Vulkan on \
                Linux), or on the processor, many times slower, where there is none; the first run that embeds says \
                which. Papers already embedded are never embedded again.",
        }),
    }
}

async fn library(State(engine): State<Shared>) -> Result<Json<Value>, ApiError> {
    let answer = on_the_side(move || {
        let library = engine.library()?;
        let (detector, validation) = crate::features::detector::routes::library_part(&engine)?;
        let problems = engine.import_problems();
        Ok(json!({
            "papers": library.paper_count()?,
            "embeddings": library.embedding_counts()?,
            "ai_validation": validation,
            "ai_detector": detector,
            "machine": machine(engine.runs.embedding.loaded_device()),
            "import_problems": problems,
        }))
    })
    .await?;
    Ok(Json(answer))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::super::routes::Engine;
    use super::super::tests::served;
    use super::*;
    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::pipeline::tests::{PYTHON_RUN, python_data_folder};
    use crate::settings::Choices;

    const AT: &str = "127.0.0.1:8765";
    const CONTRACTS: &str = include_str!("../../../../packages/contracts/src/index.ts");

    /// The fields of an interface of the contracts, at its top level.
    fn contract(name: &str) -> BTreeSet<String> {
        let start = CONTRACTS.find(&format!("export interface {name} ")).unwrap_or_else(|| panic!("no {name}"));
        let body = &CONTRACTS[start..];
        let end = body.find("\n}").unwrap();
        body[..end]
            .lines()
            .skip(1)
            .filter(|line| line.starts_with('\t') && !line.starts_with("\t\t") && !line.trim_start().starts_with("/*"))
            .filter_map(|line| {
                let field = line.trim().split([':', '?']).next()?;
                (!field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_')).then(|| field.to_owned())
            })
            .collect()
    }

    fn keys(value: &Value) -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    fn choices() -> Choices {
        let list = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        Choices {
            embedding_models: list(&["Qwen/Qwen3-Embedding-0.6B"]),
            clustering: list(&["spectral", "spectral-knn", "kmeans", "fiedler"]),
            acceptance: list(&["all", "highlights", "oral"]),
            flags: list(&["llm_declared", "closed_code", "no_limitations", "human_subjects"]),
            describe_models: list(&["claude-opus-5-5"]),
        }
    }

    fn engine(data: &Path) -> axum::Router {
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        served(&listen, AT, Engine::new(data.to_path_buf(), choices())).0
    }

    async fn ask(router: &axum::Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or(Body::empty(), |body| Body::from(body.to_string())))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        (status, to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec())
    }

    async fn ask_json(router: &axum::Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, bytes) = ask(router, method, path, body).await;
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_run_the_python_made_opens_unchanged_and_its_old_library_is_moved_in_once() {
        let data = tempfile::tempdir().unwrap();
        python_data_folder(data.path());
        let router = engine(data.path());
        let (status, listed) = ask_json(&router, "GET", "/api/runs", None).await;
        assert_eq!(status, StatusCode::OK);
        let python_run = json!([{"name": PYTHON_RUN, "title": "My conference run", "papers": 36, "communities": 1,
            "modularity": 0.4123}]);
        assert_eq!(listed, python_run);
        assert_eq!(keys(&listed[0]), contract("RunSummary"));
        let folder = data.path().join("runs").join(PYTHON_RUN);
        for (path, file) in [
            (format!("/api/runs/{PYTHON_RUN}"), files::COMMUNITIES),
            (format!("/api/runs/{PYTHON_RUN}/graph"), files::GRAPH),
        ] {
            let (status, bytes) = ask(&router, "GET", &path, None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(bytes, std::fs::read(folder.join(file)).unwrap(), "{file} is served as it is on disk");
        }
        let (_, run) = ask_json(&router, "GET", &format!("/api/runs/{PYTHON_RUN}"), None).await;
        let mut fields = contract("RunResult");
        fields.remove("citations_read"); // optional: absent in runs saved before citations were counted
        fields.insert("splits".to_owned()); // the Python's, kept
        assert_eq!(keys(&run), fields);
        let (_, graph) = ask_json(&router, "GET", &format!("/api/runs/{PYTHON_RUN}/graph"), None).await;
        assert_eq!(keys(&graph), contract("GraphData"));
        assert_eq!(keys(&graph["nodes"][0]), contract("GraphNode"));
        // the library the Python kept in files is in the database now, once
        let (_, held) = ask_json(&router, "GET", "/api/library", None).await;
        let counts = (held["papers"].clone(), held["embeddings"]["Qwen/Qwen3-Embedding-0.6B"].clone());
        assert_eq!(counts, (json!(36), json!(36)));
        assert_eq!(keys(&held), contract("Library"));
        assert_eq!(held["ai_detector"]["state"], "untrained");
        let again = engine(data.path());
        let (_, held) = ask_json(&again, "GET", "/api/library", None).await;
        assert_eq!(held["papers"], 36);
        let library = crate::library::Library::open(&data.path().join("library")).unwrap();
        assert_eq!(library.import_once(&data.path().join("library")).unwrap(), None, "moved in once only");
    }

    /// The job's status once it is no longer running.
    async fn settled(router: &axum::Router) -> Value {
        for _ in 0..600 {
            let (_, status) = ask_json(router, "GET", "/api/status", None).await;
            if status["state"] != "running" {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the job did not end");
    }

    #[tokio::test]
    async fn making_an_existing_run_again_is_refused_with_what_it_replaces_until_it_is_confirmed() {
        let data = tempfile::tempdir().unwrap();
        python_data_folder(data.path());
        let router = engine(data.path());
        let before = std::fs::read(data.path().join("runs").join(PYTHON_RUN).join(files::COMMUNITIES)).unwrap();
        // a rerun, and the same selection anew: neither starts anything
        for body in [
            json!({"rerun": PYTHON_RUN}),
            json!({"venues": ["NeurIPS 2025"]}),
            json!({"rerun": PYTHON_RUN, "replace": false}),
        ] {
            let (status, told) = ask_json(&router, "POST", "/api/runs", Some(body.clone())).await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(keys(&told), ["error".to_owned(), "replaces".to_owned()].into());
            assert_eq!(keys(&told["replaces"]), contract("RunReplaced"));
            let replaces = &told["replaces"];
            assert_eq!(
                (replaces["run"].clone(), replaces["title"].clone()),
                (json!(PYTHON_RUN), json!("My conference run"))
            );
            assert_eq!((replaces["communities"].clone(), replaces["named"].clone()), (json!(1), json!(0)));
            // made by the Python's spectral clustering (a k-means of one random start): cut otherwise
            assert_eq!(replaces["communities_will"], "change");
            let changed: Vec<&str> =
                replaces["changed"].as_array().unwrap().iter().map(|one| one["setting"].as_str().unwrap()).collect();
            assert!(changed.contains(&"max_communities") && !changed.contains(&"clustering"), "{changed:?}");
            assert_eq!(keys(&replaces["changed"][0]), contract("SettingChanged"));
            assert!(told["error"].as_str().unwrap().contains("replace: true"), "{told}");
        }
        let (_, status) = ask_json(&router, "GET", "/api/status", None).await;
        assert_eq!(status["state"], "idle", "nothing was started");
        let after = std::fs::read(data.path().join("runs").join(PYTHON_RUN).join(files::COMMUNITIES)).unwrap();
        assert_eq!(after, before, "the run is as it was");
        // `replace` must be a yes or a no
        let (status, _) =
            ask_json(&router, "POST", "/api/runs", Some(json!({"rerun": PYTHON_RUN, "replace": "yes"}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // confirmed: made again, from the library alone (nothing fetched, nothing embedded), without names
        let (status, told) =
            ask_json(&router, "POST", "/api/runs", Some(json!({"rerun": PYTHON_RUN, "replace": true}))).await;
        assert_eq!((status, told), (StatusCode::OK, json!({"ok": true})));
        let status = settled(&router).await;
        assert_eq!((status["state"].clone(), status["result"].clone()), (json!("done"), json!(PYTHON_RUN)), "{status}");
        let log: Vec<&str> = status["log"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(log.iter().any(|line| line.contains("skipped: the AI-text detector is not trained")), "{log:?}");
        assert!(log.iter().any(|line| line.contains("1 of 1 already downloaded")), "{log:?}");
        let (_, run) = ask_json(&router, "GET", &format!("/api/runs/{PYTHON_RUN}"), None).await;
        assert_eq!(run["papers"], 36);
        // made by this engine now, with the same settings: the communities come out the same unless papers changed
        let (_, told) = ask_json(&router, "POST", "/api/runs", Some(json!({"rerun": PYTHON_RUN}))).await;
        assert_eq!(
            (told["replaces"]["communities_will"].clone(), told["replaces"]["changed"].clone()),
            (json!("may_change"), json!([]))
        );
    }

    #[tokio::test]
    async fn naming_a_new_run_is_refused_for_another_model_or_without_a_key_and_a_redo_of_names_needs_an_accepted_cost()
    {
        let data = tempfile::tempdir().unwrap();
        let router = engine(data.path());
        let selection = json!({"venues": ["NeurIPS 2025"]});
        let with = |describe: Value| {
            let mut body = selection.clone();
            body["describe"] = describe;
            body
        };
        let asked = [
            (with(json!({"provider": "openai", "model": "gpt-6.2-sol", "accepted_usd": 1.0})), "is now"),
            (with(json!({"provider": "anthropic", "model": "claude-opus-5-5", "accepted_usd": 1.0})), "key"),
            (with(json!({"model": "claude-opus-5-5", "accepted_usd": 1.0, "extra": 1})), "`describe` must be"),
            (json!({"venues": ["NeurIPS 2025"], "redo": ["descriptions"]}), "accepted of its estimate"),
        ];
        for (body, said) in asked {
            let (status, told) = ask_json(&router, "POST", "/api/runs", Some(body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {told}");
            assert!(told["error"].as_str().unwrap().contains(said), "{body}: {told}");
        }
        let (_, status) = ask_json(&router, "GET", "/api/status", None).await;
        assert_eq!(status["state"], "idle");
    }

    #[tokio::test]
    async fn the_library_tells_the_detectors_state_this_computer_and_what_could_not_be_moved_in() {
        let data = tempfile::tempdir().unwrap();
        python_data_folder(data.path());
        // a damaged line in the middle of the old papers' file: that part cannot be moved, the ledger still is
        let papers = data.path().join("library").join("papers.jsonl");
        let text = std::fs::read_to_string(&papers).unwrap().replacen('\n', "\n{not json\n", 1);
        std::fs::write(&papers, text).unwrap();
        std::fs::write(
            data.path().join("library").join("usage.json"),
            r#"{"since": "2026-10-01", "models": {"claude-opus-5-5": {"calls": 2, "input_tokens": 10, "output_tokens": 5, "usd": 0.5}}}"#,
        )
        .unwrap();
        let router = engine(data.path());
        let (_, held) = ask_json(&router, "GET", "/api/library", None).await;
        assert_eq!(keys(&held), contract("Library"));
        let problems = held["import_problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].as_str().unwrap().contains("papers.jsonl, line 2"), "{problems:?}");
        assert_eq!(held["papers"], 0, "nothing of that part was kept");
        let (_, status) = ask_json(&router, "GET", "/api/status", None).await;
        assert!(status["warnings"].as_array().unwrap().contains(&problems[0]), "{status}");
        let (_, usage) = ask_json(&router, "GET", "/api/usage", None).await;
        assert_eq!(usage["calls"], 2, "the ledger is moved on its own: {usage}");
        // the detector: untrained, then off once switched off; and this computer, before anything was embedded
        assert_eq!(held["ai_detector"]["state"], "untrained");
        assert_eq!(held["ai_validation"], Value::Null);
        let (status, _) = ask_json(&router, "PUT", "/api/settings", Some(json!({"detect_ai": false}))).await;
        assert_eq!(status, StatusCode::OK);
        let (_, held) = ask_json(&router, "GET", "/api/library", None).await;
        assert_eq!(held["ai_detector"]["state"], "off");
        assert_eq!(keys(&held["machine"]), ["embedding".to_owned(), "gpu".to_owned()].into());
        assert!(held["machine"]["embedding"].as_str().unwrap().contains("embedded again"));
        // mended, and opened again: moved in then, and told no more
        let text = std::fs::read_to_string(&papers).unwrap().replace("{not json\n", "");
        std::fs::write(&papers, text).unwrap();
        let again = engine(data.path());
        let (_, held) = ask_json(&again, "GET", "/api/library", None).await;
        assert_eq!((held["papers"].clone(), held["import_problems"].clone()), (json!(36), json!([])));
        let (_, usage) = ask_json(&again, "GET", "/api/usage", None).await;
        assert_eq!(usage["calls"], 2, "moved once only");
    }

    #[tokio::test]
    async fn a_run_is_renamed_without_moving_and_deleted_without_touching_the_library() {
        let data = tempfile::tempdir().unwrap();
        python_data_folder(data.path());
        let router = engine(data.path());
        let path = format!("/api/runs/{PYTHON_RUN}");
        let (status, told) = ask_json(&router, "PATCH", &path, Some(json!({"title": "  Another   name "}))).await;
        assert_eq!((status, told), (StatusCode::OK, json!({"name": PYTHON_RUN, "title": "Another name"})));
        let (_, listed) = ask_json(&router, "GET", "/api/runs", None).await;
        assert_eq!((listed[0]["name"].clone(), listed[0]["title"].clone()), (json!(PYTHON_RUN), json!("Another name")));
        let (_, page) = ask_json(&router, "GET", "/api/runs?sort=name&limit=10", None).await;
        assert_eq!((page["total"].clone(), page["rows"][0]["kind"].clone()), (json!(1), json!("run")));
        assert_eq!(page["rows"][0]["title"], "Another name");
        assert_eq!(keys(&page), contract("RunPage"));
        assert_eq!(keys(&page["rows"][0]), contract("RunListed"));
        let (_, told) = ask_json(&router, "PATCH", &path, Some(json!({"title": ""}))).await;
        assert_eq!(told["title"], PYTHON_RUN);
        let (status, _) = ask_json(&router, "PATCH", &path, Some(json!({"title": 5}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, told) = ask_json(&router, "DELETE", &path, None).await;
        assert_eq!((status, told), (StatusCode::OK, json!({"ok": true})));
        assert!(!data.path().join("runs").join(PYTHON_RUN).exists());
        assert_eq!(ask(&router, "GET", &path, None).await.0, StatusCode::NOT_FOUND);
        let (_, listed) = ask_json(&router, "GET", "/api/runs", None).await;
        assert_eq!(listed, json!([]));
        let (_, held) = ask_json(&router, "GET", "/api/library", None).await;
        assert_eq!(held["papers"], 36, "the library's papers stay");
    }

    #[tokio::test]
    async fn names_that_leave_the_runs_folder_are_not_found() {
        let data = tempfile::tempdir().unwrap();
        python_data_folder(data.path());
        std::fs::write(data.path().join("run.json"), "{}").unwrap(); // a decoy one level up
        let router = engine(data.path());
        for name in ["..", "%2E%2E", "..%2Fruns", "nope", "neurips2025%2F..", ".hidden", "NUL"] {
            for method in ["GET", "PATCH", "DELETE"] {
                let body = (method == "PATCH").then(|| json!({"title": "x"}));
                let status = ask(&router, method, &format!("/api/runs/{name}"), body).await.0;
                let refused = status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED;
                assert!(refused, "{method} {name}: {status}");
            }
            assert_eq!(ask(&router, "GET", &format!("/api/runs/{name}/graph"), None).await.0, StatusCode::NOT_FOUND);
        }
        assert!(data.path().join("run.json").exists() && data.path().join("runs").join(PYTHON_RUN).exists());
    }

    #[tokio::test]
    async fn a_bad_run_request_is_explained_and_starts_nothing() {
        let data = tempfile::tempdir().unwrap();
        let router = engine(data.path());
        let asked = [
            (json!({"start": "2026-02-01", "end": "2026-01-01"}), StatusCode::BAD_REQUEST, "after"),
            (json!({"venues": ["NeurIPS"]}), StatusCode::BAD_REQUEST, "Name Year"),
            (json!({"rerun": "nope"}), StatusCode::NOT_FOUND, "not found"),
            (json!({"rerun": "../x"}), StatusCode::NOT_FOUND, "not found"),
            (
                json!({"venues": ["NeurIPS 2025"], "redo": ["everything"]}),
                StatusCode::BAD_REQUEST,
                "cannot redo everything",
            ),
            (json!({"venues": vec!["X 2025"; MOST_PICKED + 1]}), StatusCode::BAD_REQUEST, "at most"),
            (json!({"venues": ["X".repeat(LONGEST_PICK + 1)]}), StatusCode::BAD_REQUEST, "at most"),
            (json!(["not", "an", "object"]), StatusCode::BAD_REQUEST, "JSON object"),
            (json!({"venues": vec![format!("{} 2025", "a".repeat(150)); 3]}), StatusCode::BAD_REQUEST, "too long"),
        ];
        for (body, expected, said) in asked {
            let (status, told) = ask_json(&router, "POST", "/api/runs", Some(body.clone())).await;
            assert_eq!(status, expected, "{body}");
            assert!(told["error"].as_str().unwrap().contains(said), "{body}: {told}");
        }
        let (_, status) = ask_json(&router, "GET", "/api/status", None).await;
        assert_eq!((status["state"].clone(), status["step"].clone()), (json!("idle"), Value::Null));
        assert!(keys(&status).is_subset(&contract("Status")), "{:?}", keys(&status));
        assert!(contract("Status").contains("step"));
        let search = format!("/api/runs?search={}", "x".repeat(LONGEST_SEARCH + 1));
        assert_eq!(ask(&router, "GET", &search, None).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_catalogue_offers_what_the_engine_can_do() {
        let data = tempfile::tempdir().unwrap();
        let router = engine(data.path());
        let (status, catalog) = ask_json(&router, "GET", "/api/catalog", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(keys(&catalog), contract("Catalog"));
        let names = |key: &str| -> Vec<String> {
            catalog[key].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(names("clustering"), Method::ALL.map(|method| method.id().to_owned()));
        assert_eq!(names("acceptance"), ["all", "highlights", "oral"]);
        assert_eq!(catalog["acceptance"][0]["note"], "Every accepted paper, whatever its tier");
        assert!(catalog["acceptance"][2]["note"].as_str().unwrap().starts_with("Orals: Oral"));
        let official = json!({"name": "NeurIPS", "note": "official list"});
        assert!(catalog["conferences"][0]["items"].as_array().unwrap().contains(&official));
        assert_eq!(catalog["flags"][0]["items"].as_array().unwrap().len(), 4);
        for group in ["embedding_models", "describe_models", "clustering", "acceptance"] {
            for item in catalog[group].as_array().unwrap() {
                assert!(keys(item).is_subset(&contract("CatalogItem")), "{group}");
            }
        }
    }
}
