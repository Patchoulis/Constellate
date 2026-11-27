//! The owner's sign-in, when the engine listens on a network. On this computer nobody signs in: the desktop
//! shell's token is enough, and none of this exists.
//!
//! A server has one owner, and the only way in is to be them:
//!
//! | module        | what lives there                                                                   |
//! |---------------|------------------------------------------------------------------------------------|
//! | [`oidc`]      | sign-in with Google, or any OpenID Connect provider: the flow and the ID token's checks |
//! | [`password`]  | sign-in with a password, set once with a setup code printed at start, kept as Argon2id |
//! | [`sessions`]  | the sessions a sign-in starts, kept on the server, and their cookie               |
//! | [`page`]      | the sign-in page                                                                   |
//!
//! Which ways in there are is decided at start from the environment ([`Methods`]): Google when its client is
//! configured, a password when `CONSTELLATE_ALLOW_PASSWORD=true`, or both. With neither, the engine does not
//! start across a network.
//!
//! The routes, all under `/auth`: [`PUBLIC`] are answered to anyone (that is how one signs in), the others
//! need the owner's session like every route of the engine. The checks themselves are made in front of the
//! whole router ([`super::security`]).
//!
//! ## Requests in the owner's name from other sites (CSRF)
//!
//! A browser sends the session cookie by itself, so another site the owner has open could try to make it send a
//! request to the engine. Three things stop that, each enough on its own in current browsers:
//!
//! 1. The cookie is `SameSite=Strict`: a browser does not send it with any request another site started, not
//!    even a link followed from there.
//! 2. Every request that changes something must carry `Origin`, and it must be the interface's exact origin
//!    (`CONSTELLATE_ORIGIN`). Browsers send `Origin` with every such request and a page cannot change it, so a
//!    request without it was not sent by a browser page, and a program has no cookie to send.
//! 3. It must also carry the session's own random anti-forgery token in the `X-Constellate-Csrf` header, which
//!    only the interface can read (`GET /auth/session`, answered to the interface's own origin and to no other,
//!    since the engine sends no CORS header): a site that somehow got the cookie sent would still not have it.
//!    A header of its own name also cannot be added by a plain HTML form at all.
//!
//! Signing in is guarded too (a site must not sign the owner's browser into an account of its choosing): the
//! sign-in requests need `Origin` and a JSON body, which a form of another site cannot send without the
//! browser asking the engine first (it never agrees). The provider's way back (`/auth/callback`) is tied to
//! the browser that started the sign-in by a cookie of its own, and to that one attempt by `state`.

pub mod oidc;
pub mod page;
pub mod password;
pub mod sessions;

use std::fmt;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::config::{Cookie, Environment, Origin, switch};
use super::security::{self, Allowance, Strikes};
use crate::api::routes::{ApiError, Shared};
use oidc::{Callback, Oidc, Provider};
use password::{Hashing, PasswordError, Passwords, Setup, SetupCode};
use sessions::{Current, Sessions, cookies};

pub const OIDC_ISSUER: &str = "CONSTELLATE_OIDC_ISSUER";
pub const OIDC_CLIENT_ID: &str = "CONSTELLATE_OIDC_CLIENT_ID";
pub const OIDC_CLIENT_SECRET: &str = "CONSTELLATE_OIDC_CLIENT_SECRET";
pub const OWNER_EMAIL: &str = "CONSTELLATE_OWNER_EMAIL";
pub const ALLOW_PASSWORD: &str = "CONSTELLATE_ALLOW_PASSWORD";

/// The issuer when none is configured.
pub const GOOGLE: &str = "https://accounts.google.com";
/// The sign-in page.
pub const PAGE: &str = "/auth/";

/// Every request to a sign-in route that does something (starting a sign-in, coming back from the provider, a
/// password): 30 from one address in ten minutes, 300 from an IPv6 /64.
const ATTEMPTS: Allowance = Allowance { address: 30, network: 300, everyone: None };
/// Wrong passwords and setup codes: 10 from one address in ten minutes, 100 from an IPv6 /64, and 1000 from
/// everyone together. The last is a ceiling on guessing spread over many addresses (at most 144,000 guesses a
/// day, against a password of 12 characters or more that is no common one), set well above what one /64 or a
/// few addresses can reach, so that locking the owner out takes a hundred addresses rather than one. Past it,
/// password sign-in waits for everyone, the owner included, for the rest of the ten minutes; Google sign-in does
/// not.
const PASSWORD_FAILURES: Allowance = Allowance { address: 10, network: 100, everyone: Some(1000) };

/// Why the sign-in configuration is refused. Never repeats a value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "the engine listens on a network, so its owner must sign in, but no way to sign in is configured. For \
         Google, register an OAuth client (redirect address: CONSTELLATE_ORIGIN followed by /auth/callback) \
         and set {OIDC_CLIENT_ID}, {OIDC_CLIENT_SECRET} and {OWNER_EMAIL}; for a password, set \
         {ALLOW_PASSWORD}=true; or both. apps/engine/README.md says how"
    )]
    NoMethod,
    #[error("{given} is set but {missing} is not: a provider's client needs both")]
    HalfClient { given: &'static str, missing: &'static str },
    #[error("{OWNER_EMAIL} must be set to the owner's email address: it is the one account allowed to sign in")]
    OwnerNeeded,
    #[error("{OWNER_EMAIL} must be one email address, name@domain")]
    NotAnEmail,
    #[error("{OIDC_ISSUER} must be the provider's https address, without a query, such as {GOOGLE}")]
    NotAnIssuer,
    #[error("{ALLOW_PASSWORD} must be true or false")]
    NotASwitch,
}

/// A secret read from the environment. Shown by no `Debug`.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(hidden)")
    }
}

/// The owner's email address, the one account the provider may sign in.
///
/// Compared with the token's `email` claim as follows: both are trimmed and split at their one `@`; the domain
/// is compared without regard to ASCII case (domains are case-insensitive, RFC 1035), the part before the `@`
/// exactly (RFC 5321 leaves its case to the domain's mail server, and Google returns the address as the account
/// has it, so the owner writes it as Google shows it). Nothing else is changed: no dots or `+tags` are
/// removed, since that is how Gmail delivers mail, not who an account is. Shown by no `Debug`, since it is the
/// owner's.
#[derive(Clone, PartialEq, Eq)]
pub struct Owner {
    local: String,
    domain: String,
}

impl Owner {
    pub fn parse(text: &str) -> Option<Owner> {
        let text = text.trim();
        let (local, domain) = text.split_once('@')?;
        let plain = !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && text.chars().all(|c| !c.is_whitespace() && !c.is_control());
        plain.then(|| Owner { local: local.to_owned(), domain: domain.to_ascii_lowercase() })
    }

    pub fn address(&self) -> String {
        format!("{}@{}", self.local, self.domain)
    }
}

impl fmt::Debug for Owner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Owner(hidden)")
    }
}

/// The client registered with the provider for this server.
#[derive(Clone)]
pub struct Client {
    issuer: String,
    id: String,
    secret: Secret,
    owner: Owner,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Client").field("issuer", &self.issuer).finish_non_exhaustive()
    }
}

/// The ways the owner may sign in.
#[derive(Clone, Debug)]
pub enum Methods {
    /// Only with the provider.
    Provider(Client),
    /// Only with a password. The owner's address, when it is given, is what a password may not be.
    Password { owner: Option<Owner> },
    /// Either.
    Both(Client),
}

/// The sign-in, as configured.
#[derive(Clone, Debug)]
pub struct SignIn {
    methods: Methods,
    cookie: Cookie,
    /// Where the provider sends the browser back: the interface's origin and `/auth/callback`.
    redirect: String,
}

impl SignIn {
    /// Read from the environment. `origin` is where people open the interface, `cookie` how the session cookie
    /// is sent (both decided by [`super::config`]).
    pub(crate) fn from_environment(
        environment: &Environment,
        origin: &Origin,
        cookie: Cookie,
    ) -> Result<SignIn, ConfigError> {
        let owner =
            environment.get(OWNER_EMAIL).map(|text| Owner::parse(text).ok_or(ConfigError::NotAnEmail)).transpose()?;
        let issuer = environment.get(OIDC_ISSUER).unwrap_or(GOOGLE);
        let parsed = url::Url::parse(issuer).map_err(|_| ConfigError::NotAnIssuer)?;
        if parsed.scheme() != "https" || parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(ConfigError::NotAnIssuer);
        }
        let password = match environment.get(ALLOW_PASSWORD) {
            Some(text) => switch(text).ok_or(ConfigError::NotASwitch)?,
            None => false,
        };
        let client = match (environment.get(OIDC_CLIENT_ID), environment.get(OIDC_CLIENT_SECRET)) {
            (Some(id), Some(secret)) => Some(Client {
                issuer: issuer.trim_end_matches('/').to_owned(),
                id: id.to_owned(),
                secret: Secret(secret.to_owned()),
                owner: owner.clone().ok_or(ConfigError::OwnerNeeded)?,
            }),
            (Some(_), None) => {
                return Err(ConfigError::HalfClient { given: OIDC_CLIENT_ID, missing: OIDC_CLIENT_SECRET });
            }
            (None, Some(_)) => {
                return Err(ConfigError::HalfClient { given: OIDC_CLIENT_SECRET, missing: OIDC_CLIENT_ID });
            }
            (None, None) => None,
        };
        let methods = match (client, password) {
            (Some(client), true) => Methods::Both(client),
            (Some(client), false) => Methods::Provider(client),
            (None, true) => Methods::Password { owner },
            (None, false) => return Err(ConfigError::NoMethod),
        };
        Ok(SignIn { methods, cookie, redirect: format!("{}/auth/callback", origin.as_str()) })
    }

    pub fn cookie(&self) -> Cookie {
        self.cookie
    }

    pub fn methods(&self) -> &Methods {
        &self.methods
    }
}

/// Why the sign-in could not be made ready at start.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error(transparent)]
    Password(#[from] PasswordError),
}

/// The ways in, ready.
enum Ways {
    Provider(Box<Oidc>),
    Password(Passwords),
    Both(Box<Oidc>, Passwords),
}

impl Ways {
    fn provider(&self) -> Option<&Oidc> {
        match self {
            Ways::Provider(oidc) | Ways::Both(oidc, _) => Some(oidc),
            Ways::Password(_) => None,
        }
    }

    fn password(&self) -> Option<&Passwords> {
        match self {
            Ways::Password(passwords) | Ways::Both(_, passwords) => Some(passwords),
            Ways::Provider(_) => None,
        }
    }
}

/// The owner's sign-in while the engine runs: the ways in, the sessions, the counts of attempts.
pub struct Login {
    cookie: Cookie,
    ways: Ways,
    owner: Option<Owner>,
    sessions: Sessions,
    hashing: Hashing,
    hashing_at_once: Semaphore,
    attempts: Strikes,
    password_failures: Strikes,
}

impl Login {
    /// Ready the ways in. A password that is still to be set gets a setup code, answered to be printed.
    pub fn new(
        sign_in: SignIn,
        data: &Path,
        provider: Arc<dyn Provider>,
        hashing: Hashing,
        now: Instant,
    ) -> Result<(Login, Option<SetupCode>), LoginError> {
        let open = |owner: Option<&Owner>| Passwords::open(data, hashing.clone(), owner.map(Owner::address), now);
        let oidc = |client: Client| Box::new(Oidc::new(client, sign_in.redirect.clone(), Arc::clone(&provider)));
        let (ways, owner, code) = match sign_in.methods {
            Methods::Provider(client) => {
                let owner = client.owner.clone();
                (Ways::Provider(oidc(client)), Some(owner), None)
            }
            Methods::Password { owner } => {
                let (passwords, code) = open(owner.as_ref())?;
                (Ways::Password(passwords), owner, code)
            }
            Methods::Both(client) => {
                let owner = client.owner.clone();
                let (passwords, code) = open(Some(&owner))?;
                (Ways::Both(oidc(client), passwords), Some(owner), code)
            }
        };
        let login = Login {
            cookie: sign_in.cookie,
            ways,
            owner,
            sessions: Sessions::default(),
            hashing,
            hashing_at_once: Semaphore::new(password::HASHING_AT_ONCE),
            attempts: Strikes::new(ATTEMPTS),
            password_failures: Strikes::new(PASSWORD_FAILURES),
        };
        Ok((login, code))
    }

    /// The session the request's cookie names, if it is on.
    pub fn session(&self, headers: &HeaderMap, now: Instant) -> Option<Current> {
        self.sessions.find(cookies(headers, self.cookie.session_name()), now)
    }

    /// Start a session for a browser that just signed in, ending any it still carried.
    fn start_session(&self, headers: &HeaderMap) -> Result<sessions::Issued, ApiError> {
        self.sessions
            .start(cookies(headers, self.cookie.session_name()), Instant::now())
            .map_err(|_| ApiError::Internal)
    }

    /// May `sender` try to sign in now? Every attempt counts; `guessing` ones (a password, a setup code) are
    /// also held back while too many of them failed.
    fn may_try(&self, sender: Option<IpAddr>, guessing: bool) -> Result<(), ApiError> {
        let now = Instant::now();
        let waiting =
            self.attempts.is_waiting(sender, now) || (guessing && self.password_failures.is_waiting(sender, now));
        if waiting {
            return Err(ApiError::TooManyAttempts);
        }
        self.attempts.strike(sender, now);
        Ok(())
    }

    #[cfg(test)]
    pub fn sessions(&self) -> &Sessions {
        &self.sessions
    }
}

/// The routes that answer anyone: the sign-in page, and signing in. (`HEAD` goes with `GET`.)
pub const PUBLIC: [(Method, &str); 9] = [
    (Method::GET, "/auth"),
    (Method::GET, PAGE),
    (Method::GET, page::SCRIPT),
    (Method::GET, page::STYLE),
    (Method::GET, "/auth/session"),
    (Method::GET, "/auth/start"),
    (Method::GET, "/auth/callback"),
    (Method::POST, "/auth/password"),
    (Method::POST, "/auth/setup"),
];

/// Is `path` (exactly as requested, not decoded) a route anyone may ask for with `method`?
pub fn is_public(method: &Method, path: &str) -> bool {
    let method = if method == Method::HEAD { &Method::GET } else { method };
    PUBLIC.iter().any(|(public, route)| public == method && *route == path)
}

/// Every sign-in route, public or not.
pub fn table() -> Vec<(&'static str, MethodRouter<Arc<Login>>)> {
    vec![
        ("/auth", get(page::page)),
        (PAGE, get(page::page)),
        (page::SCRIPT, get(page::script)),
        (page::STYLE, get(page::style)),
        ("/auth/session", get(session)),
        ("/auth/start", get(start)),
        ("/auth/callback", get(callback)),
        ("/auth/password", post(with_password)),
        ("/auth/setup", post(set_up)),
        ("/auth/signout", post(sign_out)),
        ("/auth/signout-everywhere", post(sign_out_everywhere)),
    ]
}

/// The sign-in routes, to be served beside the engine's own.
pub fn routes(login: Arc<Login>) -> Router<Shared> {
    table().into_iter().fold(Router::new(), |router, (path, route)| router.route(path, route)).with_state(login)
}

/// `response` with these cookies set.
fn with_cookies(mut response: Response, set: &[String]) -> Response {
    for cookie in set {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// The browser sent to the sign-in page, which says `said` (one word the page knows).
fn back_to_the_page(said: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(location) = HeaderValue::from_str(&format!("{PAGE}?said={said}")) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

/// `GET /auth/session`: who is signed in and how one signs in. The owner's address only to the owner.
async fn session(State(login): State<Arc<Login>>, headers: HeaderMap) -> Json<Value> {
    let now = Instant::now();
    let mut methods = Vec::new();
    if login.ways.provider().is_some() {
        methods.push("provider");
    }
    if login.ways.password().is_some() {
        methods.push("password");
    }
    let mut told = json!({
        "signed_in": false,
        "methods": methods,
        "provider": login.ways.provider().map(Oidc::name),
        "setup": login.ways.password().is_some_and(|passwords| passwords.awaiting_setup(now)),
    });
    if let Some(current) = login.session(&headers, now) {
        told["signed_in"] = json!(true);
        told["csrf"] = json!(current.csrf());
        told["email"] = json!(login.owner.as_ref().map(Owner::address));
    }
    Json(told)
}

/// `GET /auth/start`: off to the provider.
async fn start(State(login): State<Arc<Login>>, extensions: Extensions) -> Response {
    if login.may_try(security::sender(&extensions), false).is_err() {
        return back_to_the_page("wait");
    }
    let begun = {
        let login = Arc::clone(&login);
        tokio::task::spawn_blocking(move || match login.ways.provider() {
            Some(oidc) => oidc.begin(Instant::now()).map(Some),
            None => Ok(None),
        })
        .await
    };
    match begun {
        Ok(Ok(Some(begun))) => {
            let mut response = StatusCode::SEE_OTHER.into_response();
            match HeaderValue::from_str(&begun.location) {
                Ok(location) => response.headers_mut().insert(header::LOCATION, location),
                Err(_) => return back_to_the_page("refused"),
            };
            with_cookies(response, &[login.cookie.flow(&begun.binding, oidc::FLOW_LASTS)])
        }
        Ok(Ok(None)) => back_to_the_page("refused"),
        Ok(Err(error)) => {
            eprintln!("constellate: sign-in with the provider could not start: {error}");
            back_to_the_page("unavailable")
        }
        Err(_) => back_to_the_page("unavailable"),
    }
}

/// `GET /auth/callback`: back from the provider.
async fn callback(
    State(login): State<Arc<Login>>,
    extensions: Extensions,
    headers: HeaderMap,
    query: Result<Query<Callback>, QueryRejection>,
) -> Response {
    let ended = login.cookie.flow_ended();
    if login.may_try(security::sender(&extensions), false).is_err() {
        return with_cookies(back_to_the_page("wait"), &[ended]);
    }
    let Ok(Query(callback)) = query else { return with_cookies(back_to_the_page("refused"), &[ended]) };
    let bindings: Vec<String> = cookies(&headers, login.cookie.flow_name()).map(str::to_owned).collect();
    let finished = {
        let login = Arc::clone(&login);
        tokio::task::spawn_blocking(move || match login.ways.provider() {
            Some(oidc) => oidc.finish(&callback, bindings.iter().map(String::as_str), Instant::now()),
            None => Err(oidc::OidcError::UnknownState),
        })
        .await
    };
    match finished {
        Ok(Ok(_)) => match login.start_session(&headers) {
            Ok(issued) => with_cookies(page::signed_in(), &[login.cookie.session(&issued.id), ended]),
            Err(error) => with_cookies(error.into_response(), &[ended]),
        },
        Ok(Err(error)) => {
            eprintln!("constellate: sign-in refused: {error}");
            with_cookies(back_to_the_page("refused"), &[ended])
        }
        Err(_) => with_cookies(back_to_the_page("unavailable"), &[ended]),
    }
}

/// A text field of a JSON object, or a refusal that names it.
fn field(body: &Value, name: &'static str) -> Result<String, ApiError> {
    body.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::Invalid(format!("the request body must be a JSON object with {name}")))
}

/// A session started: the cookie, and what the interface needs to know.
fn signed_in(login: &Login, headers: &HeaderMap) -> Response {
    match login.start_session(headers) {
        Ok(issued) => {
            let told = Json(json!({ "signed_in": true, "csrf": issued.csrf })).into_response();
            with_cookies(told, &[login.cookie.session(&issued.id)])
        }
        Err(error) => error.into_response(),
    }
}

/// Run one password check off the threads that answer requests, at most [`password::HASHING_AT_ONCE`] at once.
async fn hashing<T: Send + 'static>(
    login: &Arc<Login>,
    work: impl FnOnce(&Login) -> T + Send + 'static,
) -> Result<T, ApiError> {
    let _turn = login.hashing_at_once.acquire().await.map_err(|_| ApiError::Internal)?;
    let login = Arc::clone(login);
    tokio::task::spawn_blocking(move || work(&login)).await.map_err(|_| ApiError::Internal)
}

/// `POST /auth/password` with `{"password": ...}`.
async fn with_password(
    State(login): State<Arc<Login>>,
    extensions: Extensions,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    let sender = security::sender(&extensions);
    login.may_try(sender, true)?;
    let Json(body) = body?;
    let password = field(&body, "password")?;
    let right = hashing(&login, move |login| match login.ways.password() {
        Some(passwords) => passwords.check(&password),
        // not a way in here: the same time, and the same answer, as a wrong password
        None => {
            login.hashing.decoy(&password);
            false
        }
    })
    .await?;
    if !right {
        login.password_failures.strike(sender, Instant::now());
        return Err(ApiError::SignInRefused);
    }
    Ok(signed_in(&login, &headers))
}

/// `POST /auth/setup` with `{"code": ..., "password": ...}`: the first password, with the setup code.
async fn set_up(
    State(login): State<Arc<Login>>,
    extensions: Extensions,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    let sender = security::sender(&extensions);
    login.may_try(sender, true)?;
    let Json(body) = body?;
    let (code, password) = (field(&body, "code")?, field(&body, "password")?);
    let done = hashing(&login, move |login| match login.ways.password() {
        Some(passwords) => passwords.set_up(&code, &password, Instant::now()),
        None => {
            login.hashing.decoy(&password);
            Ok(Setup::Refused)
        }
    })
    .await?;
    match done {
        Ok(Setup::Done) => Ok(signed_in(&login, &headers)),
        Ok(Setup::Weak(weak)) => Err(ApiError::Invalid(weak.to_string())),
        Ok(Setup::Refused) => {
            login.password_failures.strike(sender, Instant::now());
            Err(ApiError::SignInRefused)
        }
        Err(error) => {
            eprintln!("constellate: the password could not be set: {error}");
            Err(ApiError::Files)
        }
    }
}

/// `POST /auth/signout`: this browser's session ends.
async fn sign_out(State(login): State<Arc<Login>>, headers: HeaderMap) -> Response {
    for id in cookies(&headers, login.cookie.session_name()) {
        login.sessions.end(id);
    }
    with_cookies(Json(json!({ "signed_in": false })).into_response(), &[login.cookie.session_ended()])
}

/// `POST /auth/signout-everywhere`: every session ends, on every device.
async fn sign_out_everywhere(State(login): State<Arc<Login>>) -> Response {
    login.sessions.end_all();
    with_cookies(Json(json!({ "signed_in": false })).into_response(), &[login.cookie.session_ended()])
}

#[cfg(test)]
mod tests;
