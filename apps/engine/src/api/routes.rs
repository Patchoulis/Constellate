//! What the interface can ask, and how the engine's errors are told to it.
//!
//! ```text
//! GET /api/status?since=N   the job (jobs.rs) with its log from line N on, its progress and what it made, the
//!                           warnings about how it listens, the mode (local or network), the settings the server
//!                           owns, and whether a key for the chosen provider is set (never the key)
//! POST /api/job/cancel      {id}: ask the running job to stop (it finishes what is under way)
//! GET /api/settings         the settings
//! PUT /api/settings         change some of them: a JSON object of the ones to change
//! ```
//!
//! and a paper's reviews, read from the page the user opened ([`crate::api::papers::reviews`]), and a paper's text and where a
//! quote is in it ([`crate::api::papers::passages`]).
//!
//! Across a network the owner's sign-in routes are served too (`/auth/...`, [`crate::api::guard::login`]).
//!
//! A route is a function of the shared [`Engine`]; adding one is a line in [`table`] (or a `Router<Shared>` of
//! its own given to [`secured`]). Whatever is added passes the security layer like everything else: it is
//! applied around the whole router, unknown paths included.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use axum::{Json, middleware};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::api::guard::config::Mode;
use crate::api::guard::security::{self, Guard, LARGEST_BODY};
use crate::jobs::{Busy, Counted, Finished, Job, Phase, Progress};
use crate::library::{Library, LibraryError};
use crate::model::chosen::Chosen;
use crate::model::language::{Endpoint, Provider, Transport, Unavailable};
use crate::settings::{self, Choices, Settings, SettingsError};

/// How long a request may take from its first byte to its answer: a client that sends its body slowly (to hold
/// the engine's connections) is cut off.
const PATIENCE: Duration = Duration::from_secs(30);

/// What every route shares.
pub struct Engine {
    /// The one background job (a run, reading declarations, a feature asking the model).
    pub job: Job<Finished>,
    /// The data folder: settings, library, runs.
    pub data: PathBuf,
    /// What each choice among the settings may be.
    pub choices: Choices,
    /// What is unsafe about how the engine listens ([`crate::api::guard::config::Listen::warnings`]), for the interface.
    pub warnings: Vec<String>,
    /// This computer only, or a network: set by the server from how it listens.
    pub mode: Mode,
    /// The language model's providers, as their variables were read at start ([`super::models`]): the one place a
    /// feature's endpoint comes from, in the program and in the tests alike.
    pub providers: crate::model::language::Providers,
    /// How the language model's requests reach its provider: the network, or a test's fake (which reaches nothing,
    /// so that no test of a route can ever call a real provider).
    pub transport: Arc<dyn Transport>,
    /// Papers' PDFs on demand: fetched once, politely, when a paper's is not kept ([`crate::papers::pdf`]). A test
    /// gives it a fake fetcher, so that no test reaches the network.
    pub pdfs: crate::papers::pdf::Pdfs,
    /// The library, opened when a route first needs it (and tried again after a failure to open).
    library: Mutex<Option<Arc<Library>>>,
    /// The person's runs: the one being made, the list, the loaded ones ([`crate::pipeline::served`]).
    pub runs: crate::pipeline::served::Runs,
    /// What of the earlier version's data folder could not be moved in when the library was opened, in words.
    import_problems: Mutex<Vec<String>>,
}

pub type Shared = Arc<Engine>;

impl Engine {
    pub fn new(data: PathBuf, choices: Choices) -> Engine {
        let providers = Default::default();
        Engine {
            job: Job::new(),
            data,
            choices,
            warnings: Vec::new(),
            mode: Mode::Local,
            providers,
            // in the tests, a transport that reaches nothing: a test that gives a key and starts a job must
            // never call a real provider, even one it did not mean to start
            #[cfg(not(test))]
            transport: Arc::new(crate::model::language::Https::default()),
            #[cfg(test)]
            transport: Arc::new(crate::model::language::fake::Fake::default()),
            pdfs: crate::papers::pdf::Pdfs::default(),
            library: Mutex::new(None),
            runs: Default::default(),
            import_problems: Mutex::new(Vec::new()),
        }
    }

    /// What of the earlier version's data folder could not be moved in, in words: empty when everything was (or
    /// the library has not been opened yet).
    pub fn import_problems(&self) -> Vec<String> {
        self.import_problems.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The library in the data folder, opened once and shared by every request.
    pub fn library(&self) -> Result<Arc<Library>, ApiError> {
        let mut opened = self.library.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(library) = opened.as_ref() {
            return Ok(Arc::clone(library));
        }
        let library = Arc::new(Library::open(&self.data.join("library"))?);
        // pages kept by an older reader of reviews are read again once, here: until then they would not be shown
        if let Err(error) = library.read_stored_pages_again() {
            eprintln!("library: reviews pages could not be read again: {error}");
        }
        // the Python backend's data folder, moved in once, part by part (library/core/import.rs); what could not be is
        // told in /api/library and as a warning in /api/status, and tried again at the next start
        let moved = library.import_old(&self.data.join("library"));
        if moved != crate::library::core::import::Moved::default() {
            eprintln!("library: the earlier version's data folder: {moved:?}");
        }
        *self.import_problems.lock().unwrap_or_else(PoisonError::into_inner) = moved.problems();
        // calls left with their outcome unknown by an earlier run (a crash after the provider billed): told, never
        // asked again by themselves
        match library.unsettled().map(|unsettled| unsettled.said()) {
            Ok(Some(said)) => eprintln!("constellate: {said}"),
            Ok(None) => {}
            Err(error) => eprintln!("library: the calls whose outcome is unknown could not be read: {error}"),
        }
        *opened = Some(Arc::clone(&library));
        Ok(library)
    }

    /// Where the language model is asked: the provider chosen in the settings, with its key and address as
    /// read at start, or why it cannot be asked (its key is not set). Every feature that asks the model asks
    /// here, so that the choice in the interface is the one that is used.
    ///
    /// Settings that cannot be read (a damaged or hand-edited file) say nothing of which provider was chosen: that
    /// is told ([`Unavailable::SettingsUnreadable`]), never taken for the Claude API, which the server may not be
    /// set to send papers' text to.
    pub fn endpoint(&self) -> Result<Endpoint, Unavailable> {
        let settings = self.read_settings()?;
        self.endpoint_for(settings.describe_provider)
    }

    /// The settings, or why the provider to ask cannot be known from them.
    fn read_settings(&self) -> Result<Settings, Unavailable> {
        settings::load(&self.settings_file(), &self.choices).map_err(|error| {
            eprintln!("settings: {error}");
            Unavailable::SettingsUnreadable
        })
    }

    fn endpoint_for(&self, provider: Provider) -> Result<Endpoint, Unavailable> {
        self.providers.endpoint(provider)
    }

    /// For the tests: the Claude API with a made-up `key`, at `base` when one is given (an address where nothing
    /// listens). Whatever is sent goes through the tests' transport, which reaches nothing.
    #[cfg(test)]
    pub fn set_claude_key(&mut self, key: &str, base: Option<&str>) {
        let (key, base) = (key.to_owned(), base.map(str::to_owned));
        self.providers = crate::model::language::Providers::from_lookup(|name| match name {
            crate::model::language::KEY_VARIABLE => Some(key.clone()),
            crate::model::language::BASE_VARIABLE => base.clone(),
            _ => None,
        });
    }

    /// The provider, model and effort every feature asks with, and the endpoint it asks at, read together from
    /// the settings ([`Chosen::new`]): the one place a feature learns which model it asks, so that what it
    /// estimates, keys its answers under and sends cannot be of different providers. Settings that cannot be read
    /// are an error naming the file, never a provider guessed.
    pub fn chosen(&self) -> Result<Chosen, ApiError> {
        let settings = self.read_settings().map_err(|why| ApiError::Invalid(why.to_string()))?;
        let provider = settings.describe_provider;
        Chosen::new(self.endpoint_for(provider), provider, &settings).map_err(|why| ApiError::Invalid(why.to_string()))
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data.join("settings.json")
    }
}

/// Why a request was not answered, told as `{ "error": "..." }` with the status that says what kind of reason it
/// is.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// What was sent is wrong; the message says how, for the person who sent it.
    #[error("{0}")]
    Invalid(String),
    #[error("not found")]
    NotFound,
    /// Something is already running; the message names it.
    #[error(transparent)]
    Busy(#[from] Busy),
    #[error("the request body is larger than {} MB", LARGEST_BODY / (1024 * 1024))]
    TooLarge,
    /// The same answer for a missing token and a wrong one.
    #[error("this request needs the engine's token")]
    Unauthorized,
    /// Across a network, a request without the owner's session (or an allowed token).
    #[error("sign in first, at /auth/")]
    SignInNeeded,
    /// The same answer for a wrong password, a wrong setup code, and a way of signing in that is not offered.
    #[error("sign-in was refused")]
    SignInRefused,
    #[error("{0}")]
    Forbidden(&'static str),
    /// A setting the server decides ([`server_owned`]).
    #[error(
        "{0} is decided by the server this window is connected to: it is the same for every window, and \
         changing it would embed the whole library again. It is set in the server's own settings"
    )]
    ServerOwned(&'static str),
    #[error("too many attempts: try again later")]
    TooManyAttempts,
    /// The engine's own files could not be read or written. What exactly went wrong (with the file's path) goes
    /// to the engine's error output, not to whoever asked.
    #[error("the engine could not read or write its files")]
    Files,
    #[error("the engine failed unexpectedly")]
    Internal,
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::Invalid(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Busy(_) => StatusCode::CONFLICT,
            ApiError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::Unauthorized | ApiError::SignInNeeded | ApiError::SignInRefused => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) | ApiError::ServerOwned(_) => StatusCode::FORBIDDEN,
            ApiError::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Files | ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let mut response = (status, Json(serde_json::json!({ "error": self.to_string() }))).into_response();
        let headers = response.headers_mut();
        match self {
            ApiError::Unauthorized => {
                headers.insert(header::WWW_AUTHENTICATE, header::HeaderValue::from_static("Bearer"));
            }
            // RFC 9110 asks a 401 to name a scheme; a session is no registered one, so the realm says where
            ApiError::SignInNeeded | ApiError::SignInRefused => {
                headers.insert(
                    header::WWW_AUTHENTICATE,
                    header::HeaderValue::from_static("Session realm=\"constellate\", sign_in=\"/auth/\""),
                );
            }
            ApiError::TooManyAttempts => {
                headers.insert(header::RETRY_AFTER, header::HeaderValue::from(security::STRIKE_WINDOW.as_secs()));
            }
            _ => {}
        }
        response
    }
}

impl From<settings::Invalid> for ApiError {
    fn from(error: settings::Invalid) -> Self {
        ApiError::Invalid(error.to_string())
    }
}

impl From<SettingsError> for ApiError {
    fn from(error: SettingsError) -> Self {
        match error {
            SettingsError::Invalid(error) => error.into(),
            SettingsError::Store(error) => {
                eprintln!("settings: {error}");
                ApiError::Files
            }
        }
    }
}

impl From<crate::library::core::store::StoreError> for ApiError {
    fn from(error: crate::library::core::store::StoreError) -> Self {
        eprintln!("{error}");
        ApiError::Files
    }
}

impl From<LibraryError> for ApiError {
    fn from(error: LibraryError) -> Self {
        match error {
            LibraryError::UnknownPaper(_) => ApiError::NotFound,
            error => {
                eprintln!("library: {error}");
                ApiError::Files
            }
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::TooLarge
        } else {
            ApiError::Invalid("the request body must be a JSON object".to_owned())
        }
    }
}

impl From<QueryRejection> for ApiError {
    fn from(_: QueryRejection) -> Self {
        ApiError::Invalid("the query is not what this request takes".to_owned())
    }
}

/// `name` if it is exactly one folder or file name, for a route that takes one in its path (a run's name): no
/// separator, no `..`, nothing hidden, no drive, no Windows device (`NUL`, `COM1`), nothing that is not
/// printable. Anything else is not found, so that a path cannot reach outside the folder it is meant for.
pub fn one_name(name: &str) -> Result<&str, ApiError> {
    let printable = name.chars().all(|c| !c.is_control());
    let plain = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.ends_with(['.', ' ']) // Windows drops these, so "a." would be "a"
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']);
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let numbered = |prefix: &str| {
        let mut rest = stem.strip_prefix(prefix).unwrap_or("x").chars();
        // Windows counts the superscript digits too
        matches!((rest.next(), rest.next()), (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None))
    };
    let device = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || numbered("COM")
        || numbered("LPT");
    if printable && plain && !device && Path::new(name).file_name().is_some_and(|file| file == name) {
        Ok(name)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Run file work (which may wait for a lock) off the threads that answer requests.
pub(crate) async fn on_the_side<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.map_err(|_| ApiError::Internal)?
}

/// Every route the engine has, by path. A test walks this list to see that each one is refused to whoever has
/// not signed in, so a route is added here and nowhere else.
///
/// The api's own routes and each feature's (`crate::features::<feature>::routes::table()`) are added one line
/// each, in the fixed order below, the order they were added in; a new one goes at the end.
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    let mut table = vec![
        ("/api/status", get(status)),
        ("/api/job/cancel", post(cancel_job)),
        ("/api/settings", get(read_settings).put(change_settings)),
        ("/api/usage/unsettled", get(unsettled)),
    ];
    table.extend(crate::api::papers::reviews::table());
    table.extend(crate::api::papers::passages::table());
    table.extend(crate::features::critique::routes::table());
    table.extend(crate::features::stances::routes::table());
    table.extend(crate::features::extraction::routes::table());
    table.extend(crate::features::explanation::routes::table());
    table.extend(crate::features::claim_tree::routes::table());
    table.extend(crate::features::citation_contexts::routes::table());
    table.extend(crate::api::papers::citations::table());
    table.extend(crate::features::answers::routes::table());
    table.extend(super::models::table());
    table.extend(crate::features::writing::routes::table());
    table.extend(super::runs::table());
    table.extend(crate::features::detector::routes::table());
    table.extend(super::usage::table());
    table.extend(super::papers::table());
    table
}

/// The routes the engine has, without the security layer: [`secured`] adds it.
pub fn routes() -> Router<Shared> {
    table().into_iter().fold(Router::new(), |router, (path, route)| router.route(path, route))
}

/// The settings that belong to the server when the interface reaches it across a network: the same for every
/// window that connects, and not changed from one. The embedding model is the library's: changing it would
/// embed every paper again. The language model's provider is the server's: its key, its account and the address
/// papers' text is sent to are the server owner's (the model and effort within it may be chosen from a window).
pub fn server_owned(mode: Mode) -> &'static [&'static str] {
    match mode {
        Mode::Local => &[],
        Mode::Network => &["embedding_model", "describe_provider"],
    }
}

/// `routes` behind the security layer, with the engine they share: the one way a router is served.
pub fn secured(routes: Router<Shared>, engine: Shared, guard: Guard) -> Router {
    routes
        .fallback(|| async { ApiError::NotFound })
        // the limit for the extractors (Json), and for any route that reads the body itself
        .layer(DefaultBodyLimit::max(LARGEST_BODY))
        .layer(RequestBodyLimitLayer::new(LARGEST_BODY))
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, PATIENCE))
        .layer(middleware::from_fn_with_state(guard, security::check))
        .with_state(engine)
}

#[derive(Deserialize)]
struct Since {
    since: Option<usize>,
}

/// `Status` in the contracts: the job as it is now, and what the interface needs to know of the engine.
#[derive(Serialize)]
struct Status<'a> {
    id: u64,
    kind: String,
    state: Phase,
    /// What a finished job made ([`Finished`]): a run, a feature's answers, or nothing to open.
    outcome: Option<Finished>,
    /// Derived from `outcome`, kept for an interface that knows only the Python backend: the name of the run a
    /// finished job made, and nothing else, so that such an interface never takes a feature's job for a run. New
    /// code reads `outcome`.
    result: Option<String>,
    error: Option<String>,
    /// The log's lines from `since` on, and how many it has in all.
    log: Vec<String>,
    total: usize,
    /// Calls done of how many, and dollars spent so far, for a job that reports them.
    progress: Option<Progress>,
    /// Papers done of how many, for a job that counts them.
    papers: Option<Counted>,
    warnings: &'a [String],
    /// `local` or `network`: what kind of engine the interface is talking to.
    mode: &'static str,
    /// The settings the interface may not change ([`server_owned`]).
    server_owned: &'static [&'static str],
    /// Whether the chosen provider's model can be asked: its key is set (and its address is usable). Only that,
    /// never the key.
    api_key_set: bool,
    /// The step of the run being made and how far it has got; none when no run is being made.
    step: Option<crate::pipeline::steps::StepReport>,
    /// The feature a feature's job is (or was) for, from the moment it starts ([`crate::jobs::About`]): so that
    /// another window can tell which section a running job belongs to. None for any other job.
    feature: Option<crate::library::Feature>,
    /// What that job asks about, as `outcome`'s subject is told.
    subject: Option<crate::library::Subject>,
    /// The one item of the subject it asks about, when it is one: a question's id among a run's questions.
    item: Option<String>,
}

async fn status(
    State(engine): State<Shared>,
    since: Result<Query<Since>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(Since { since }) = since?;
    let mode = match engine.mode {
        Mode::Local => "local",
        Mode::Network => "network",
    };
    let job = engine.job.snapshot(since.unwrap_or(0));
    let warnings: Vec<String> = engine.warnings.iter().cloned().chain(engine.import_problems()).collect();
    let result = match &job.result {
        Some(Finished::Run { run }) => Some(run.clone()),
        Some(Finished::Feature { .. } | Finished::Nothing) | None => None,
    };
    let (feature, subject, item) = match job.about {
        Some(about) => (Some(about.feature), Some(about.subject), about.item),
        None => (None, None, None),
    };
    let told = Status {
        feature,
        subject,
        item,
        id: job.id,
        kind: job.kind,
        state: job.state,
        outcome: job.result,
        result,
        error: job.error,
        log: job.log,
        total: job.total,
        progress: job.progress,
        papers: job.papers,
        warnings: &warnings,
        mode,
        server_owned: server_owned(engine.mode),
        api_key_set: engine.endpoint().is_ok(),
        step: engine.runs.board.now(),
    };
    Ok(Json(serde_json::to_value(told).map_err(|_| ApiError::Internal)?))
}

/// `Unsettled` in the contracts: calls asked of the language model whose outcome is unknown (an intent left by a
/// crash, a connection lost after sending: `library::money::asking`), told in the ledger's terms. They may have been
/// billed, at most this much; they are not asked again until the user starts that work again.
#[derive(Serialize)]
struct UnsettledView {
    calls: i64,
    /// US dollars, at most, of the priced ones.
    most_usd: f64,
    /// Those of a model with no known price: `most_usd` leaves them out.
    unpriced_calls: i64,
    /// Tokens of every kind, at most, of all of them.
    most_tokens: i64,
    /// When the oldest was asked (UTC); null when there is none.
    since: Option<String>,
    /// The same in a sentence, with its units; null when there is none.
    said: Option<String>,
}

async fn unsettled(State(engine): State<Shared>) -> Result<Json<UnsettledView>, ApiError> {
    let unsettled = on_the_side(move || Ok(engine.library()?.unsettled()?)).await?;
    Ok(Json(UnsettledView {
        calls: unsettled.calls,
        most_usd: unsettled.most_usd(),
        unpriced_calls: unsettled.unpriced_calls,
        most_tokens: unsettled.most_tokens,
        said: unsettled.said(),
        since: unsettled.since,
    }))
}

#[derive(Deserialize)]
struct Cancelling {
    id: u64,
}

async fn cancel_job(
    State(engine): State<Shared>,
    body: Result<Json<Cancelling>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(Cancelling { id }) = body?;
    engine.job.cancel(id).map_err(|error| ApiError::Invalid(error.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn read_settings(State(engine): State<Shared>) -> Result<Json<Settings>, ApiError> {
    let settings = on_the_side(move || Ok(settings::load(&engine.settings_file(), &engine.choices)?)).await?;
    Ok(Json(settings))
}

async fn change_settings(
    State(engine): State<Shared>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Settings>, ApiError> {
    let Json(body) = body?;
    let changes: Map<String, Value> = match body {
        Value::Object(changes) => changes,
        _ => return Err(ApiError::Invalid("the request body must be a JSON object".to_owned())),
    };
    let saved = on_the_side(move || {
        let owned = server_owned(engine.mode);
        if owned.iter().any(|key| changes.contains_key(*key)) {
            // sent unchanged (an interface that sends every setting) is fine; changed is not
            let current = serde_json::to_value(settings::load(&engine.settings_file(), &engine.choices)?)
                .map_err(|_| ApiError::Internal)?;
            if let Some(key) =
                owned.iter().find(|key| changes.get(**key).is_some_and(|value| Some(value) != current.get(**key)))
            {
                return Err(ApiError::ServerOwned(key));
            }
        }
        Ok(settings::save(&engine.settings_file(), &changes, &engine.choices)?)
    })
    .await?;
    Ok(Json(saved))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_path_name_is_one_plain_name_or_not_found() {
        for good in ["neurips2025", "NeurIPS 2025 + ICML 2026", "run-3_b", "console", "com10"] {
            assert_eq!(one_name(good).unwrap(), good);
        }
        let devices = ["NUL", "con", "Com1", "lpt9.txt", "aux.json", "COM\u{b9}", "lpt\u{b3}.log", "CONIN$", "conout$"];
        let others =
            ["", ".", "..", "../data", "a/b", "a\\b", "C:", "C:\\x", ".hidden", "nul\0", "line\nbreak", "a.", "a "];
        for bad in devices.into_iter().chain(others) {
            assert!(matches!(one_name(bad), Err(ApiError::NotFound)), "{bad:?}");
        }
    }

    #[test]
    fn settings_that_cannot_be_read_ask_no_provider_rather_than_the_claude_api() {
        let data = tempfile::tempdir().unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), Choices::default());
        engine.providers = crate::model::language::Providers::from_lookup(|name| {
            matches!(name, "ANTHROPIC_API_KEY" | "OPENAI_API_KEY").then(|| "test-key-not-real".to_owned())
        });
        std::fs::write(engine.settings_file(), r#"{"describe_provider": "openai""#).unwrap();
        assert_eq!(engine.endpoint().unwrap_err(), Unavailable::SettingsUnreadable);
        let refused = engine.chosen().unwrap_err();
        assert!(matches!(refused, ApiError::Invalid(_)) && refused.to_string().contains("settings.json"), "{refused}");
        // read again once the file can be: the provider it names
        std::fs::write(engine.settings_file(), r#"{"describe_provider": "openai"}"#).unwrap();
        assert_eq!(engine.endpoint().unwrap().provider(), Provider::OpenAi);
    }

    #[tokio::test]
    async fn calls_left_with_their_outcome_unknown_are_told_in_the_ledgers_terms_as_the_contract_has_them() {
        use crate::library::money::asking::Intent;
        use crate::library::{AnswerKey, Feature, Slot, Subject};
        let data = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::new(data.path().to_path_buf(), Choices::default()));
        let Json(none) = unsettled(State(Arc::clone(&engine))).await.unwrap();
        let none = serde_json::to_value(none).unwrap();
        crate::api::contract::assert_fits("Unsettled", &none, &[]);
        assert_eq!((none["calls"].as_i64(), none["said"].is_null()), (Some(0), true));
        let slot = Slot {
            feature: Feature::Answers,
            key: AnswerKey::new(Feature::Answers, Provider::Anthropic, "claude-opus-5-5", 1, &["p"], "q"),
            subject: Subject::Run("r1".to_owned()),
        };
        let intent = Intent {
            slot: &slot,
            provider: "anthropic",
            model: "claude-opus-5-5",
            most_tokens: 9_000,
            most_micros: Some(120_000),
        };
        engine.library().unwrap().intend(&intent).unwrap();
        let Json(left) = unsettled(State(Arc::clone(&engine))).await.unwrap();
        let left = serde_json::to_value(left).unwrap();
        crate::api::contract::assert_fits("Unsettled", &left, &[]);
        assert_eq!((left["calls"].as_i64(), left["most_usd"].as_f64()), (Some(1), Some(0.12)));
        assert!(left["said"].as_str().unwrap().contains("may have been billed, about 0.1200 US dollars at most"));
    }

    #[test]
    fn every_route_of_the_table_is_documented_in_the_readme() {
        let readme = include_str!("../../README.md");
        let missing: Vec<&str> = table()
            .iter()
            .map(|(path, _)| *path)
            // written as `METHOD path` in a row, the path ending at the backtick or at its query
            .filter(|path| !readme.contains(&format!(" {path}`")) && !readme.contains(&format!(" {path}?")))
            .collect();
        assert!(missing.is_empty(), "routes not in apps/engine/README.md: {missing:?}");
    }

    #[test]
    fn every_error_is_told_with_its_status() {
        let busy = ApiError::from(Busy { kind: "a run".to_owned() });
        assert_eq!((busy.status(), busy.to_string().as_str()), (StatusCode::CONFLICT, "a run is already in progress"));
        assert_eq!(ApiError::TooLarge.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(ApiError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(ApiError::Invalid("x".to_owned()).status(), StatusCode::BAD_REQUEST);
    }
}
