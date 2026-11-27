//! What the interface asks the engine, over HTTP, answered only to the app itself.
//!
//! | module       | what lives there                                                                   |
//! |--------------|------------------------------------------------------------------------------------|
//! | [`config`]   | how the engine listens, from the environment: this computer only, or a network     |
//! | [`connections`] | how many connections there may be, and how long one may stay silent             |
//! | [`security`] | the checks every request passes (address, origin, token or session) and its headers |
//! | [`login`]    | across a network: the owner's sign-in, with Google or a password, and sessions     |
//! | [`routes`]   | the routes, the state they share and how errors are told                           |
//! | [`reviews`]  | a paper's reviews read from the page the user opened, and what they come to        |
//! | [`passages`] | a paper's text from its PDF, and where a quote is in it                            |
//! | [`stances`]  | where a community's papers agree and disagree on a question                        |
//! | [`models`]   | the language models' providers and models, for the interface to choose from        |
//!
//! [`Server`] puts them together: bound to its address, it says once where it is ([`Server::startup_line`]) and
//! serves until it is told to stop.

pub mod answers;
pub mod citation_contexts;
pub mod claim_tree;
pub mod config;
pub mod connections;
#[cfg(test)]
pub(crate) mod contract;
pub mod critique;
pub mod explanation;
pub mod extraction;
pub mod login;
pub mod models;
#[cfg(test)]
pub(crate) mod other_providers;
pub mod passages;
pub mod reviews;
pub mod routes;
pub mod security;
pub mod stances;

use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum_server::Handle;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use hyper_util::rt::{TokioExecutor, TokioTimer};
use hyper_util::server::conn::auto::Builder;

use config::{Listen, Transport};
use connections::Seats;
use login::oidc::{self, Provider};
use login::password::{Hashing, PasswordError, SetupCode};
use routes::{Engine, Shared};
use security::{Admission, Guard};

/// How long a client may take to send a request's headers before its connection is closed.
const HEADERS_PATIENCE: Duration = Duration::from_secs(10);
/// The most a request's headers may take up.
const LARGEST_HEADERS: usize = 64 * 1024;
/// How long requests still being answered get to finish when the engine stops.
const LAST_ORDERS: Duration = Duration::from_secs(5);

/// The engine's HTTP server, bound and ready.
pub struct Server {
    listener: TcpListener,
    bound: SocketAddr,
    listen: Listen,
    engine: Shared,
    app: Router,
    /// How long a connection may stay silent ([`connections::QUIET`]; shorter in tests).
    quiet: Duration,
    /// The code to set the owner's first password with, until it is printed.
    setup: Option<SetupCode>,
}

impl Server {
    /// Bind to the address `listen` says, and put `routes` behind the security layer (and, across a network,
    /// the owner's sign-in).
    pub fn bind(listen: Listen, engine: Engine, routes: Router<Shared>) -> io::Result<Server> {
        let listener = TcpListener::bind(listen.address())?;
        Server::on(listener, listen, engine, routes)
    }

    /// Serve on a socket bound already. The rules are those of `listen`, the address the socket's own.
    pub(crate) fn on(
        listener: TcpListener,
        listen: Listen,
        engine: Engine,
        routes: Router<Shared>,
    ) -> io::Result<Server> {
        Server::with(listener, listen, engine, routes, Arc::new(oidc::Http::default()), Hashing::strong)
    }

    /// [`Server::on`] with the provider reached through `provider` and passwords hashed as `hashing` says.
    pub(crate) fn with(
        listener: TcpListener,
        listen: Listen,
        engine: Engine,
        routes: Router<Shared>,
        provider: Arc<dyn Provider>,
        hashing: impl FnOnce() -> Result<Hashing, PasswordError>,
    ) -> io::Result<Server> {
        let bound = listener.local_addr()?;
        let (app, engine, setup) = application(&listen, bound, engine, routes, provider, hashing)?;
        Ok(Server { listener, bound, listen, engine, app, quiet: connections::QUIET, setup })
    }

    /// Tell whoever runs the server, on standard error, the code to set the owner's first password with, if
    /// there is one to set. The code is not kept: this is the one place it is ever shown.
    pub fn announce_setup_code(&mut self) {
        let Some(code) = self.setup.take() else { return };
        let page = self
            .listen
            .origin()
            .map_or_else(|| "the sign-in page".to_owned(), |origin| format!("{}/auth/", origin.as_str()));
        eprintln!("constellate: no password is set for the owner yet. Within 15 minutes, open {page} and enter");
        eprintln!("constellate: this setup code: {}", code.expose());
        eprintln!("constellate: it works once; restart the engine for a new one.");
    }

    /// The address and port served at (the port is the real one when any free port was asked for).
    pub fn bound(&self) -> SocketAddr {
        self.bound
    }

    pub fn engine(&self) -> Shared {
        Arc::clone(&self.engine)
    }

    /// One line of JSON that says where the engine is, for the process that started it:
    ///
    /// ```text
    /// {"listening":"http://127.0.0.1:52817","scheme":"http","address":"127.0.0.1","port":52817,"token":"…"}
    /// ```
    ///
    /// `token` is there only when the engine made it up itself. The line goes to standard output, which the
    /// desktop shell reads over a pipe; whoever reads it must not write it anywhere else (a log, a file), since
    /// with the token anyone can act as the app.
    pub fn startup_line(&self) -> String {
        let scheme = self.listen.scheme();
        let mut line = serde_json::json!({
            "listening": format!("{scheme}://{}", self.bound),
            "scheme": scheme,
            "address": self.bound.ip().to_string(),
            "port": self.bound.port(),
        });
        if let Some(token) = self.listen.generated_token() {
            line["token"] = token.expose().into();
        }
        line.to_string()
    }

    /// Serve until `stop` completes, then let the requests being answered finish (for a few seconds at most).
    pub async fn run(self, stop: impl Future<Output = ()> + Send + 'static) -> io::Result<()> {
        let handle = Handle::new();
        let stopping = handle.clone();
        tokio::spawn(async move {
            stop.await;
            stopping.graceful_shutdown(Some(LAST_ORDERS));
        });
        let service = self.app.into_make_service_with_connect_info::<SocketAddr>();
        let seats = Seats::new(self.listen.mode(), self.quiet);
        match self.listen.transport() {
            Transport::Https(tls) => {
                // the seat is taken before the TLS handshake, which the acceptor gives 10 seconds
                let tls = RustlsAcceptor::new(RustlsConfig::from_config(tls.server_config())).acceptor(seats);
                let mut server = axum_server::from_tcp(self.listener).acceptor(tls);
                patient(server.http_builder());
                server.handle(handle).serve(service).await
            }
            Transport::Loopback(_) | Transport::PlainByChoice(_) => {
                let mut server = axum_server::from_tcp(self.listener).acceptor(seats);
                patient(server.http_builder());
                server.handle(handle).serve(service).await
            }
        }
    }
}

/// Everything that answers requests for `listen`, bound at `bound`: `routes`, and across a network the
/// sign-in's, behind the checks of [`security`]. Answers the setup code of the owner's password, if one is due.
pub(crate) fn application(
    listen: &Listen,
    bound: SocketAddr,
    mut engine: Engine,
    routes: Router<Shared>,
    provider: Arc<dyn Provider>,
    hashing: impl FnOnce() -> Result<Hashing, PasswordError>,
) -> io::Result<(Router, Shared, Option<SetupCode>)> {
    engine.warnings = listen.warnings();
    engine.mode = listen.mode();
    let (admission, setup) =
        Admission::new(listen, &engine.data, provider, hashing).map_err(|error| io::Error::other(error.to_string()))?;
    let routes = match admission.login() {
        Some(login) => routes.merge(login::routes(login)),
        None => routes,
    };
    let engine = Arc::new(engine);
    let app = routes::secured(routes, Arc::clone(&engine), Guard::new(listen, bound, admission));
    Ok((app, engine, setup))
}

/// Limits on how slowly and how much a client may send once it is known which HTTP it speaks.
fn patient(builder: &mut Builder<TokioExecutor>) {
    builder.http1().timer(TokioTimer::new()).header_read_timeout(HEADERS_PATIENCE).max_buf_size(LARGEST_HEADERS);
    builder
        .http2()
        .timer(TokioTimer::new())
        .max_header_list_size(LARGEST_HEADERS as u32)
        // a peer that stops answering pings is gone: its connection is closed
        .keep_alive_interval(Some(connections::QUIET / 2))
        .keep_alive_timeout(HEADERS_PATIENCE);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::config::tests::{STRONG, certificate_in};
    use super::config::{Environment, Listen};
    use super::routes::{Engine, routes};
    use super::*;
    use crate::settings::Choices;

    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, Response, StatusCode, header};
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tower::ServiceExt;

    const AT: &str = "127.0.0.1:8765";
    const INTERFACE: &str = "http://127.0.0.1:3000";

    fn choices() -> Choices {
        let list = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        Choices {
            embedding_models: list(&["Qwen/Qwen3-Embedding-0.6B"]),
            clustering: list(&["spectral", "kmeans"]),
            acceptance: list(&["all", "oral"]),
            flags: list(&["llm_declared", "closed_code"]),
            describe_models: list(&["claude-opus-5-5"]),
        }
    }

    fn listen(pairs: &[(&str, &str)]) -> Listen {
        Listen::from_environment(&Environment::from_pairs(pairs.iter().copied())).unwrap()
    }

    /// The router as served for `listen` at `at`, with `engine`, a provider nobody reaches and quick hashing.
    pub(crate) fn served(listen: &Listen, at: &str, engine: Engine) -> (Router, Option<SetupCode>) {
        let provider = Arc::new(login::oidc::tests::Fake::new(&[]));
        let (router, _, code) =
            application(listen, at.parse().unwrap(), engine, routes(), provider, || Ok(Hashing::light())).unwrap();
        (router, code)
    }

    /// The secured router as served at [`AT`], with a data folder of its own.
    fn app(pairs: &[(&str, &str)]) -> (Router, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        let (router, _) = served(&listen(pairs), AT, Engine::new(data.path().to_path_buf(), choices()));
        (router, data)
    }

    /// Across a network, with a password to sign in with and the token allowed for programs.
    const NETWORK: [(&str, &str); 5] = [
        ("CONSTELLATE_BIND", "192.0.2.7"),
        ("CONSTELLATE_TOKEN", STRONG),
        ("CONSTELLATE_ALLOW_TOKEN", "true"),
        ("CONSTELLATE_USE_HTTPS", "false"),
        ("CONSTELLATE_ALLOW_PASSWORD", "true"),
    ];

    fn network(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        NETWORK
            .iter()
            .copied()
            .chain([("CONSTELLATE_ORIGIN", "https://papers.test")])
            .chain(extra.iter().copied())
            .collect()
    }

    fn local() -> (Router, tempfile::TempDir) {
        app(&[("CONSTELLATE_TOKEN", STRONG), ("CONSTELLATE_ORIGIN", INTERFACE)])
    }

    /// A request as the app sends it: addressed to the engine, with the token.
    fn asked(method: &str, path: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
    }

    async fn answer(router: &Router, request: Request<Body>) -> (StatusCode, Value, Response<()>) {
        let response = router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX).await.unwrap_or_default();
        let told = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (parts.status, told, Response::from_parts(parts, ()))
    }

    #[tokio::test]
    async fn a_request_from_the_app_is_answered_with_headers_that_protect_it() {
        let (router, _data) = local();
        let (status, told, response) = answer(&router, asked("GET", "/api/status").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!((told["state"].clone(), told["warnings"].clone()), (json!("idle"), json!([])));
        let headers = response.headers();
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert!(headers.keys().all(|name| !name.as_str().starts_with("access-control-")));
    }

    #[tokio::test]
    async fn a_missing_or_wrong_token_is_refused_with_the_same_answer() {
        let (router, _data) = local();
        let mut answers = Vec::new();
        for authorization in [
            None,
            Some("Bearer wrong"),
            Some(&format!("Bearer {STRONG}x")[..]),
            Some(&format!("Basic {STRONG}")[..]),
            Some(STRONG),
        ] {
            let mut request = Request::get("/api/status").header(header::HOST, AT);
            if let Some(authorization) = authorization {
                request = request.header(header::AUTHORIZATION, authorization);
            }
            let (status, told, response) = answer(&router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{authorization:?}");
            assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store"); // refusals are protected too
            answers.push(told);
        }
        assert!(answers.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(!answers[0].to_string().contains(STRONG));
        // the scheme's name in any case
        let shouted = Request::get("/api/status")
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("BEARER {STRONG}"));
        assert_eq!(answer(&router, shouted.body(Body::empty()).unwrap()).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_unknown_path_is_refused_without_the_token_and_not_found_with_it() {
        let (router, _data) = local();
        let stranger = Request::get("/api/../../etc/passwd").header(header::HOST, AT).body(Body::empty()).unwrap();
        assert_eq!(answer(&router, stranger).await.0, StatusCode::UNAUTHORIZED);
        let (status, told, _) = answer(&router, asked("GET", "/api/nothing-here").body(Body::empty()).unwrap()).await;
        assert_eq!((status, told), (StatusCode::NOT_FOUND, json!({"error": "not found"})));
        let (status, told, _) = answer(&router, asked("DELETE", "/api/status").body(Body::empty()).unwrap()).await;
        assert_eq!(
            (status, told),
            (StatusCode::METHOD_NOT_ALLOWED, json!({"error": "this request is not taken here"}))
        );
    }

    #[tokio::test]
    async fn a_request_addressed_to_another_name_is_refused_even_with_the_token() {
        let (router, _data) = local();
        for host in ["evil.example:8765", "evil.example", "127.0.0.1:9999", "127.0.0.1", "127.0.0.1:8765.evil.example"]
        {
            let request = Request::get("/api/status")
                .header(header::HOST, host)
                .header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
            assert_eq!(answer(&router, request.body(Body::empty()).unwrap()).await.0, StatusCode::FORBIDDEN, "{host}");
        }
        let twice = asked("GET", "/api/status").header(header::HOST, "evil.example").body(Body::empty()).unwrap();
        assert_eq!(answer(&router, twice).await.0, StatusCode::FORBIDDEN);
        let absolute = asked("GET", "http://evil.example/api/status").body(Body::empty()).unwrap();
        assert_eq!(answer(&router, absolute).await.0, StatusCode::FORBIDDEN);
        let nameless = Request::get("/api/status").header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
        assert_eq!(answer(&router, nameless.body(Body::empty()).unwrap()).await.0, StatusCode::FORBIDDEN);
        let by_name = Request::get("/api/status")
            .header(header::HOST, "LOCALHOST:8765")
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
        assert_eq!(answer(&router, by_name.body(Body::empty()).unwrap()).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn on_this_computer_a_request_forwarded_for_a_name_that_is_not_this_computers_is_refused() {
        // the interface's server rewrites Host to the engine's and says the browser's name in X-Forwarded-Host:
        // a page whose name was pointed at this computer (DNS rebinding) shows only there
        let (router, _data) = local();
        for (forwarded, expected) in [
            ("127.0.0.1:3000", StatusCode::OK),
            ("localhost:3000", StatusCode::OK),
            ("[::1]:3000", StatusCode::OK),
            ("rebound.evil.example:3000", StatusCode::FORBIDDEN),
            ("127.0.0.1.evil.example", StatusCode::FORBIDDEN),
            ("localhost:3000, evil.example", StatusCode::FORBIDDEN),
            ("localhost/evil", StatusCode::FORBIDDEN),
        ] {
            let request =
                asked("GET", "/api/status").header("x-forwarded-host", forwarded).body(Body::empty()).unwrap();
            assert_eq!(answer(&router, request).await.0, expected, "{forwarded}");
        }
        let twice = asked("GET", "/api/status")
            .header("x-forwarded-host", "localhost:3000")
            .header("x-forwarded-host", "evil.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(answer(&router, twice).await.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_page_of_another_site_is_refused_even_with_the_token_and_the_interface_is_not() {
        let (router, _data) = local();
        for origin in ["http://evil.example", "http://127.0.0.1:3001", "null", "http://127.0.0.1:3000.evil.example"] {
            for method in ["GET", "PUT"] {
                let request =
                    asked(method, "/api/settings").header(header::ORIGIN, origin).body(Body::from("{}")).unwrap();
                let (status, told, _) = answer(&router, request).await;
                assert_eq!(
                    (status, told),
                    (StatusCode::FORBIDDEN, json!({"error": "requests from other sites are refused"}))
                );
            }
        }
        let ours = asked("GET", "/api/settings").header(header::ORIGIN, INTERFACE).body(Body::empty()).unwrap();
        assert_eq!(answer(&router, ours).await.0, StatusCode::OK);

        // without a configured origin: pages from this computer only, on this computer; none across a network
        let (development, _data) = app(&[("CONSTELLATE_TOKEN", STRONG)]);
        for (origin, expected) in
            [("http://localhost:3000", StatusCode::OK), ("https://evil.example", StatusCode::FORBIDDEN)]
        {
            let request = asked("GET", "/api/status").header(header::ORIGIN, origin).body(Body::empty()).unwrap();
            assert_eq!(answer(&development, request).await.0, expected, "{origin}");
        }
    }

    #[tokio::test]
    async fn a_body_larger_than_the_limit_is_refused_whether_or_not_it_says_its_size() {
        let (router, _data) = local();
        let declared =
            asked("PUT", "/api/settings").header(header::CONTENT_LENGTH, "999999999").body(Body::empty()).unwrap();
        let (status, told, _) = answer(&router, declared).await;
        assert_eq!(
            (status, told),
            (StatusCode::PAYLOAD_TOO_LARGE, json!({"error": "the request body is larger than 4 MB"}))
        );
        let huge = format!("{{\"describe\": \"{}\"}}", "x".repeat(security::LARGEST_BODY));
        // no Content-Length header: cut off as it is read
        let undeclared = asked("PUT", "/api/settings")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(huge))
            .unwrap();
        assert_eq!(answer(&router, undeclared).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn settings_are_read_changed_kept_and_checked() {
        let (router, data) = local();
        let (status, told, _) = answer(&router, asked("GET", "/api/settings").body(Body::empty()).unwrap()).await;
        assert_eq!((status, told["clustering"].clone()), (StatusCode::OK, json!("spectral")));

        let change = asked("PUT", "/api/settings").header(header::CONTENT_TYPE, "application/json");
        let body = json!({"clustering": "kmeans", "max_links": "3", "junk": true}).to_string();
        let (status, told, _) = answer(&router, change.body(Body::from(body)).unwrap()).await;
        assert_eq!(
            (status, told["clustering"].clone(), told["max_links"].clone()),
            (StatusCode::OK, json!("kmeans"), json!(3))
        );
        let kept: Value = serde_json::from_slice(&std::fs::read(data.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!((kept["clustering"].clone(), kept.get("junk")), (json!("kmeans"), None));

        for (body, said) in [
            (json!({"clustering": "magic", "max_links": 4}).to_string(), "clustering must be one of spectral, kmeans"),
            ("[1, 2]".to_owned(), "the request body must be a JSON object"),
            ("{ not json".to_owned(), "the request body must be a JSON object"),
        ] {
            let change = asked("PUT", "/api/settings").header(header::CONTENT_TYPE, "application/json");
            let (status, told, _) = answer(&router, change.body(Body::from(body)).unwrap()).await;
            assert_eq!((status, told), (StatusCode::BAD_REQUEST, json!({"error": said})));
        }
        let (_, told, _) = answer(&router, asked("GET", "/api/settings").body(Body::empty()).unwrap()).await;
        assert_eq!(told["max_links"], 3); // the refused changes changed nothing
    }

    #[tokio::test]
    async fn the_status_has_the_log_from_a_line_on_and_the_warnings() {
        let data = tempfile::tempdir().unwrap();
        let listen = listen(&network(&[]));
        let engine = Engine::new(data.path().to_path_buf(), choices());
        let (release, wait) = std::sync::mpsc::channel::<()>();
        engine
            .job
            .start("a run", move |log| {
                log("[1/2] download");
                log("[2/2] embed");
                wait.recv().map_err(|error| error.to_string())?;
                Ok(crate::jobs::Finished::Run { run: "neurips2025".to_owned() })
            })
            .unwrap();
        let at = "192.0.2.7:8765";
        let (router, _) = served(&listen, at, engine);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let told = loop {
            let request = Request::get("/api/status?since=1")
                .header(header::HOST, at)
                .header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
            let (_, told, _) = answer(&router, request.body(Body::empty()).unwrap()).await;
            if told["total"] == 2 || std::time::Instant::now() > deadline {
                break told;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        release.send(()).unwrap();
        assert_eq!((told["state"].clone(), told["log"].clone()), (json!("running"), json!(["[2/2] embed"])));
        assert!(told["warnings"][0].as_str().unwrap().contains("HTTPS is off"));
        assert_eq!(
            (told["mode"].clone(), told["server_owned"].clone()),
            (json!("network"), json!(["embedding_model", "describe_provider"]))
        );
        let bad = Request::get("/api/status?since=many")
            .header(header::HOST, at)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
        assert_eq!(answer(&router, bad.body(Body::empty()).unwrap()).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn across_a_network_an_address_that_keeps_guessing_is_made_to_wait_and_others_are_not() {
        let data = tempfile::tempdir().unwrap();
        let listen = listen(&network(&[]));
        let at = "192.0.2.7:8765";
        let (router, _) = served(&listen, at, Engine::new(data.path().to_path_buf(), choices()));
        let from = |remote: &str, token: &str| {
            let mut request = Request::get("/api/status")
                .header(header::HOST, at)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(ConnectInfo::<SocketAddr>(remote.parse().unwrap()));
            request
        };
        for guess in 0..10 {
            assert_eq!(
                answer(&router, from("198.51.100.9:4000", &format!("guess{guess}"))).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        let (status, _, response) = answer(&router, from("198.51.100.9:4001", STRONG)).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS); // even the right token, so a guess cannot be confirmed
        assert!(response.headers().contains_key(header::RETRY_AFTER));
        assert_eq!(answer(&router, from("198.51.100.10:4000", STRONG)).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn behind_a_trusted_proxy_the_client_it_names_is_counted_and_its_neighbours_are_not_locked_out() {
        let data = tempfile::tempdir().unwrap();
        let listen = listen(&network(&[("CONSTELLATE_TRUSTED_PROXIES", "192.0.2.50")]));
        let at = "192.0.2.7:8765";
        let (router, _) = served(&listen, at, Engine::new(data.path().to_path_buf(), choices()));
        let via = |remote: &str, client: &str, token: &str| {
            let mut request = Request::get("/api/status")
                .header(header::HOST, at)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-forwarded-for", client)
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(ConnectInfo::<SocketAddr>(remote.parse().unwrap()));
            request
        };
        for guess in 0..10 {
            let guessed = via("192.0.2.50:4000", "203.0.113.7", &format!("guess{guess}"));
            assert_eq!(answer(&router, guessed).await.0, StatusCode::UNAUTHORIZED);
        }
        let waits = via("192.0.2.50:4000", "203.0.113.7", STRONG);
        assert_eq!(answer(&router, waits).await.0, StatusCode::TOO_MANY_REQUESTS, "the guesser waits");
        let neighbour = via("192.0.2.50:4000", "203.0.113.8", STRONG);
        assert_eq!(answer(&router, neighbour).await.0, StatusCode::OK, "another client of the same proxy does not");
        // a client that is not the proxy cannot name someone else to be counted against
        for guess in 0..10 {
            let spoofed = via("198.51.100.20:4000", &format!("203.0.113.{}", 100 + guess), "wrong");
            assert_eq!(answer(&router, spoofed).await.0, StatusCode::UNAUTHORIZED);
        }
        let caught = via("198.51.100.20:4000", "203.0.113.200", STRONG);
        assert_eq!(answer(&router, caught).await.0, StatusCode::TOO_MANY_REQUESTS);
    }

    /// Serve `server` in the background; dropping the answer stops it.
    fn serve(
        server: Server,
    ) -> (SocketAddr, tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<io::Result<()>>) {
        let bound = server.bound();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let running = tokio::spawn(server.run(async move {
            let _ = stopped.await;
        }));
        (bound, stop, running)
    }

    async fn exchange(stream: &mut (impl AsyncReadExt + AsyncWriteExt + Unpin), host: &str) -> String {
        let request = format!(
            "GET /api/status HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {STRONG}\r\nConnection: close\r\n\r\n"
        );
        let mut answer = Vec::new();
        let talk = async {
            stream.write_all(request.as_bytes()).await?;
            stream.read_to_end(&mut answer).await
        };
        let _ = tokio::time::timeout(Duration::from_secs(10), talk).await;
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[tokio::test]
    async fn on_this_computer_the_engine_serves_on_a_free_port_says_where_and_stops_when_told() {
        let data = tempfile::tempdir().unwrap();
        let server = Server::bind(listen(&[]), Engine::new(data.path().to_path_buf(), choices()), routes()).unwrap();
        let line: Value = serde_json::from_str(&server.startup_line()).unwrap();
        assert_eq!((line["scheme"].clone(), line["address"].clone()), (json!("http"), json!("127.0.0.1")));
        assert_ne!(line["port"], 0);
        let token = line["token"].as_str().unwrap().to_owned();
        assert_eq!(token.len(), 64);
        let (bound, stop, running) = serve(server);

        let mut stream = TcpStream::connect(bound).await.unwrap();
        let request = format!(
            "GET /api/status HTTP/1.1\r\nHost: {bound}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), running).await.unwrap().unwrap().unwrap();
        assert!(TcpStream::connect(bound).await.is_err()); // no longer listening
    }

    #[tokio::test]
    async fn a_connection_that_never_says_which_http_it_speaks_is_closed() {
        let data = tempfile::tempdir().unwrap();
        let mut server =
            Server::bind(listen(&[]), Engine::new(data.path().to_path_buf(), choices()), routes()).unwrap();
        server.quiet = Duration::from_millis(200);
        let (bound, stop, _running) = serve(server);
        let mut silent = TcpStream::connect(bound).await.unwrap();
        silent.write_all(b"PRI * HT").await.unwrap(); // the start of HTTP/2's preface, and then nothing
        let mut rest = Vec::new();
        let closed = tokio::time::timeout(Duration::from_secs(10), silent.read_to_end(&mut rest)).await;
        assert!(closed.is_ok(), "the connection was kept open");
        stop.send(()).unwrap();
    }

    #[tokio::test]
    async fn across_a_network_the_engine_serves_https_with_its_certificate_and_refuses_plain_http() {
        let folder = tempfile::tempdir().unwrap();
        let (certificate, key) = certificate_in(folder.path());
        // the rules of a network address, on a loopback socket: tests never listen beyond this computer
        let listen = listen(&[
            ("CONSTELLATE_BIND", "0.0.0.0"),
            ("CONSTELLATE_HOST", "localhost"),
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ALLOW_TOKEN", "true"),
            ("CONSTELLATE_ALLOW_PASSWORD", "true"),
            ("CONSTELLATE_ORIGIN", "https://localhost"),
            ("CONSTELLATE_TLS_CERT", certificate.to_str().unwrap()),
            ("CONSTELLATE_TLS_KEY", key.to_str().unwrap()),
        ]);
        assert_eq!(listen.mode(), config::Mode::Network);
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut server =
            Server::on(socket, listen, Engine::new(folder.path().join("data"), choices()), routes()).unwrap();
        assert!(server.setup.is_some(), "no password yet: a setup code is due");
        server.announce_setup_code();
        assert!(server.setup.is_none(), "the code is not kept once it is shown");
        let line: Value = serde_json::from_str(&server.startup_line()).unwrap();
        assert_eq!((line["scheme"].clone(), line.get("token")), (json!("https"), None)); // a given token is never repeated
        let (bound, stop, running) = serve(server);
        let host = format!("localhost:{}", bound.port());

        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::pem::PemObject::from_pem_file(&certificate).unwrap()).unwrap();
        let client = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut secure = connector.connect(name, TcpStream::connect(bound).await.unwrap()).await.unwrap();
        let answer = exchange(&mut secure, &host).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.to_ascii_lowercase().contains("strict-transport-security"));

        let mut plain = TcpStream::connect(bound).await.unwrap();
        let answer = exchange(&mut plain, &host).await;
        assert!(!answer.starts_with("HTTP/"), "plain HTTP was answered: {answer}");
        assert!(!answer.contains("idle"));

        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), running).await.unwrap().unwrap().unwrap();
    }
}
