//! What the interface can ask, and how the engine's errors are told to it.
//!
//! ```text
//! GET /api/status?since=N   the job (jobs.rs) with its log from line N on, and the warnings about how it listens
//! GET /api/settings         the settings
//! PUT /api/settings         change some of them: a JSON object of the ones to change
//! ```
//!
//! A route is a function of the shared [`Engine`]; adding one is a line in [`routes`] (or a `Router<Shared>` of
//! its own given to [`secured`]). Whatever is added passes the security layer like everything else: it is
//! applied around the whole router, unknown paths included.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, middleware};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use super::security::{self, Guard, LARGEST_BODY};
use crate::jobs::{Busy, Job, Snapshot};
use crate::settings::{self, Choices, Settings, SettingsError};

/// How long a request may take from its first byte to its answer: a client that sends its body slowly (to hold
/// the engine's connections) is cut off.
const PATIENCE: Duration = Duration::from_secs(30);

/// What every route shares.
pub struct Engine {
    /// The one background job (a run, reading declarations).
    pub job: Job<String>,
    /// The data folder: settings, library, runs.
    pub data: PathBuf,
    /// What each choice among the settings may be.
    pub choices: Choices,
    /// What is unsafe about how the engine listens ([`super::config::Listen::warnings`]), for the interface.
    pub warnings: Vec<String>,
}

pub type Shared = Arc<Engine>;

impl Engine {
    pub fn new(data: PathBuf, choices: Choices) -> Engine {
        Engine { job: Job::new(), data, choices, warnings: Vec::new() }
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
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("too many requests with a wrong token: try again later")]
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
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
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

impl From<crate::library::store::StoreError> for ApiError {
    fn from(error: crate::library::store::StoreError) -> Self {
        eprintln!("{error}");
        ApiError::Files
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
async fn on_the_side<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.map_err(|_| ApiError::Internal)?
}

/// The routes the engine has, without the security layer: [`secured`] adds it.
pub fn routes() -> Router<Shared> {
    Router::new().route("/api/status", get(status)).route("/api/settings", get(read_settings).put(change_settings))
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

#[derive(Serialize)]
struct Status<'a> {
    #[serde(flatten)]
    job: Snapshot<String>,
    warnings: &'a [String],
}

async fn status(
    State(engine): State<Shared>,
    since: Result<Query<Since>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(Since { since }) = since?;
    let told = Status { job: engine.job.snapshot(since.unwrap_or(0)), warnings: &engine.warnings };
    Ok(Json(serde_json::to_value(told).map_err(|_| ApiError::Internal)?))
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
    let saved = on_the_side(move || Ok(settings::save(&engine.settings_file(), &changes, &engine.choices)?)).await?;
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
    fn every_error_is_told_with_its_status() {
        let busy = ApiError::from(Busy { kind: "a run".to_owned() });
        assert_eq!((busy.status(), busy.to_string().as_str()), (StatusCode::CONFLICT, "a run is already in progress"));
        assert_eq!(ApiError::TooLarge.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(ApiError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(ApiError::Invalid("x".to_owned()).status(), StatusCode::BAD_REQUEST);
    }
}
