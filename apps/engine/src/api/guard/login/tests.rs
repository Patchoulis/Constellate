//! The sign-in through the HTTP layer, as a browser and an attacker meet it: every route behind it, the
//! password and the provider's way in, sessions, and requests from other sites.

#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;
use url::Url;

use super::oidc::tests::{Fake, Signer, claims, rsa_signer};
use super::password::{Hashing, SetupCode};
use super::sessions::CSRF_HEADER;
use crate::api::guard::config::tests::STRONG;
use crate::api::guard::config::{Environment, Listen};
use crate::api::routes::{self, Engine};
use crate::settings::Choices;

const AT: &str = "192.0.2.7:8765";
const ORIGIN: &str = "https://papers.test";
const OWNER: &str = "Owner.Name@example.org";
const GOOD: &str = "violet kettle orbit 42";
const SESSION: &str = "__Host-constellate";

fn choices() -> Choices {
    let list = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    Choices {
        embedding_models: list(&["Qwen/Qwen3-Embedding-0.6B", "BAAI/bge-large-en-v1.5"]),
        clustering: list(&["spectral", "kmeans"]),
        acceptance: list(&["all", "oral"]),
        flags: list(&["llm_declared"]),
        describe_models: list(&["claude-opus-5-5"]),
    }
}

/// An engine across a network, as configured by `pairs` on top of the network address and the origin.
struct Served {
    router: Router,
    code: Option<SetupCode>,
    fake: Fake,
    _data: tempfile::TempDir,
}

fn served_with(pairs: &[(&str, &str)], signers: &[&Signer]) -> Served {
    let base = [("CONSTELLATE_BIND", "192.0.2.7"), ("CONSTELLATE_USE_HTTPS", "false"), ("CONSTELLATE_ORIGIN", ORIGIN)];
    let all: Vec<(&str, &str)> = base.into_iter().chain(pairs.iter().copied()).collect();
    let listen = Listen::from_environment(&Environment::from_pairs(all)).unwrap();
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new(signers);
    let engine = Engine::new(data.path().to_path_buf(), choices());
    let (router, _, code) =
        crate::api::application(&listen, AT.parse().unwrap(), engine, routes::routes(), Arc::new(fake.clone()), || {
            Ok(Hashing::light())
        })
        .unwrap();
    Served { router, code, fake, _data: data }
}

/// Password sign-in only.
fn with_password() -> Served {
    served_with(&[("CONSTELLATE_ALLOW_PASSWORD", "true"), ("CONSTELLATE_OWNER_EMAIL", OWNER)], &[])
}

/// Sign-in with the provider only.
fn with_provider(signer: &Signer) -> Served {
    served_with(
        &[
            ("CONSTELLATE_OIDC_ISSUER", super::oidc::tests::ISSUER),
            ("CONSTELLATE_OIDC_CLIENT_ID", super::oidc::tests::CLIENT),
            ("CONSTELLATE_OIDC_CLIENT_SECRET", "client-secret.test"),
            ("CONSTELLATE_OWNER_EMAIL", super::oidc::tests::OWNER),
        ],
        &[signer],
    )
}

fn asked(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(path).header(header::HOST, AT)
}

/// A request the interface sends: from its origin, with JSON.
fn sent(method: &str, path: &str) -> axum::http::request::Builder {
    asked(method, path).header(header::ORIGIN, ORIGIN).header(header::CONTENT_TYPE, "application/json")
}

fn from(mut request: Request<Body>, remote: &str) -> Request<Body> {
    request.extensions_mut().insert(ConnectInfo::<SocketAddr>(remote.parse().unwrap()));
    request
}

async fn answer(router: &Router, request: Request<Body>) -> (StatusCode, Value, HeaderMap) {
    let response = router.clone().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap_or_default();
    (parts.status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), parts.headers)
}

/// The `Set-Cookie` header that sets `name`.
fn set_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

/// The value a `Set-Cookie` header gives its cookie.
fn value_of(set: &str) -> String {
    set.split(';').next().unwrap().split_once('=').unwrap().1.to_owned()
}

/// Set the password with the setup code; answers the session cookie and its anti-forgery token.
async fn set_up(served: &Served) -> (String, String) {
    let code = served.code.as_ref().unwrap().expose().to_owned();
    let body = json!({ "code": code, "password": GOOD }).to_string();
    let (status, told, headers) =
        answer(&served.router, sent("POST", "/auth/setup").body(Body::from(body)).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{told}");
    let cookie = value_of(&set_cookie(&headers, SESSION).unwrap());
    (cookie, told["csrf"].as_str().unwrap().to_owned())
}

async fn sign_in(served: &Served, remote: &str, password: &str) -> (StatusCode, Value, HeaderMap) {
    let body = json!({ "password": password }).to_string();
    let request = from(sent("POST", "/auth/password").body(Body::from(body)).unwrap(), remote);
    answer(&served.router, request).await
}

fn with_cookie(builder: axum::http::request::Builder, cookie: &str) -> axum::http::request::Builder {
    builder.header(header::COOKIE, format!("{SESSION}={cookie}"))
}

/// Every route there is, with a sample value for each parameter of its path.
fn every_route() -> Vec<String> {
    let engine = routes::table().into_iter().map(|(path, _)| path);
    let sign_in = super::table().into_iter().map(|(path, _)| path);
    engine
        .chain(sign_in)
        .map(|path| {
            path.split('/')
                .map(|part| if part.starts_with('{') { "sample" } else { part })
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

#[tokio::test]
async fn across_a_network_every_route_but_signing_in_is_refused_to_whoever_has_not_signed_in() {
    let served = with_password();
    let methods =
        [Method::GET, Method::HEAD, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS];
    let mut walked = 0;
    for path in every_route().iter().chain(&["/".to_owned(), "/api/nothing-here".to_owned(), "/auth/x".to_owned()]) {
        for method in &methods {
            if super::is_public(method, path) {
                continue;
            }
            for origin in [None, Some(ORIGIN)] {
                let mut request = asked(method.as_str(), path);
                if let Some(origin) = origin {
                    request = request.header(header::ORIGIN, origin);
                }
                let (status, _, headers) = answer(&served.router, request.body(Body::empty()).unwrap()).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path} answered without signing in");
                assert_eq!(headers[header::CACHE_CONTROL], "no-store");
                walked += 1;
            }
        }
    }
    assert!(walked > 50, "the walk saw only {walked} requests");
    // a cookie nobody issued, or a token when tokens are not allowed, is no better
    let (status, _, _) =
        answer(&served.router, with_cookie(asked("GET", "/api/status"), "made-up").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let bearer = asked("GET", "/api/status").header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
    assert_eq!(answer(&served.router, bearer.body(Body::empty()).unwrap()).await.0, StatusCode::UNAUTHORIZED);
}

#[test]
fn the_only_routes_open_to_anyone_are_the_sign_in_routes() {
    let sign_in: Vec<&str> = super::table().into_iter().map(|(path, _)| path).collect();
    for (method, path) in super::PUBLIC {
        assert!(sign_in.contains(&path), "{method} {path} is public but is not a sign-in route");
        assert!(path == "/auth" || path.starts_with("/auth/"));
    }
    for path in ["/auth/signout", "/auth/signout-everywhere", "/api/status", "/api/settings"] {
        for method in [Method::GET, Method::POST, Method::PUT] {
            assert!(!super::is_public(&method, path), "{method} {path}");
        }
    }
    // the path as requested, never a variant of it
    for variant in ["/auth/session/", "//auth/session", "/auth/%73ession", "/AUTH/session", "/auth/session?x"] {
        assert!(!super::is_public(&Method::GET, variant), "{variant}");
    }
}

#[tokio::test]
async fn a_browser_that_opens_a_page_without_being_signed_in_is_sent_to_the_sign_in_page() {
    let served = with_password();
    let opened = asked("GET", "/").header("sec-fetch-mode", "navigate").body(Body::empty()).unwrap();
    let (status, _, headers) = answer(&served.router, opened).await;
    assert_eq!((status, headers[header::LOCATION].to_str().unwrap()), (StatusCode::SEE_OTHER, "/auth/"));

    let (status, _, headers) = answer(&served.router, asked("GET", "/auth/").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(policy.contains("default-src 'none'") && policy.contains("script-src 'self'"));
    assert!(!policy.contains("unsafe"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (_, _, headers) = answer(&served.router, asked("GET", "/auth/sign-in.js").body(Body::empty()).unwrap()).await;
    assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/javascript"));
    // an answer that is not a page allows nothing
    let (_, _, headers) = answer(&served.router, asked("GET", "/auth/session").body(Body::empty()).unwrap()).await;
    assert_eq!(headers[header::CONTENT_SECURITY_POLICY], "default-src 'none'; frame-ancestors 'none'");
}

#[tokio::test]
async fn the_session_answer_tells_how_to_sign_in_and_never_the_owners_address_to_a_stranger() {
    let served = with_password();
    let (status, told, _) = answer(&served.router, asked("GET", "/auth/session").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (told["signed_in"].clone(), told["methods"].clone(), told["setup"].clone()),
        (json!(false), json!(["password"]), json!(true))
    );
    assert!(told.get("csrf").is_none() && told.get("email").is_none());
    assert!(!told.to_string().to_lowercase().contains("owner.name"));

    let (cookie, csrf) = set_up(&served).await;
    let (_, told, _) =
        answer(&served.router, with_cookie(asked("GET", "/auth/session"), &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!((told["signed_in"].clone(), told["csrf"].clone()), (json!(true), json!(csrf)));
    assert_eq!(told["email"], OWNER);
    assert_eq!(told["setup"], false);
}

#[tokio::test]
async fn the_setup_code_sets_the_password_once_and_signs_the_owner_in_with_a_cookie_that_only_this_site_gets() {
    let served = with_password();
    let code = served.code.as_ref().unwrap().expose().to_owned();
    // a weak password is refused with the reason, and the code still works
    let weak = json!({ "code": code, "password": "password1234" }).to_string();
    let (status, told, _) = answer(&served.router, sent("POST", "/auth/setup").body(Body::from(weak)).unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(told["error"].as_str().unwrap().contains("common") && !told.to_string().contains("password1234"));
    let wrong = json!({ "code": "AAAAA-BBBBB-CCCCC-DDDDD", "password": GOOD }).to_string();
    let (status, told, _) = answer(&served.router, sent("POST", "/auth/setup").body(Body::from(wrong)).unwrap()).await;
    assert_eq!((status, told), (StatusCode::UNAUTHORIZED, json!({"error": "sign-in was refused"})));

    let body = json!({ "code": code, "password": GOOD }).to_string();
    let (status, told, headers) =
        answer(&served.router, sent("POST", "/auth/setup").body(Body::from(body)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(told["csrf"].as_str().unwrap().len() >= 43);
    let set = set_cookie(&headers, SESSION).unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age=43200"] {
        assert!(set.split("; ").any(|part| part == attribute), "{attribute} missing from {set}");
    }
    assert!(!set.contains("Domain") && value_of(&set).len() >= 43);
    assert!(!set.contains(GOOD) && !told.to_string().contains(GOOD));

    // single use
    let again = json!({ "code": code, "password": "another fine password 7" }).to_string();
    assert_eq!(
        answer(&served.router, sent("POST", "/auth/setup").body(Body::from(again)).unwrap()).await.0,
        StatusCode::UNAUTHORIZED
    );
    // and the cookie lets the owner in
    let cookie = value_of(&set);
    let (status, _, _) =
        answer(&served.router, with_cookie(asked("GET", "/api/status"), &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_wrong_password_and_a_way_in_that_is_not_offered_get_the_same_answer() {
    let served = with_password();
    set_up(&served).await;
    let (status, wrong, headers) = sign_in(&served, "198.51.100.1:4000", "violet kettle orbit 43").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(set_cookie(&headers, SESSION).is_none());
    let (status, right, headers) = sign_in(&served, "198.51.100.1:4000", GOOD).await;
    assert_eq!(status, StatusCode::OK, "{right}");
    assert!(set_cookie(&headers, SESSION).is_some());

    let signer = rsa_signer(0, "k1");
    let google_only = with_provider(&signer);
    let (status, not_offered, _) = sign_in(&google_only, "198.51.100.1:4000", GOOD).await;
    assert_eq!((status, not_offered), (StatusCode::UNAUTHORIZED, wrong));
    let setup = json!({ "code": "AAAAA-BBBBB-CCCCC-DDDDD", "password": GOOD }).to_string();
    let (status, told, _) =
        answer(&google_only.router, sent("POST", "/auth/setup").body(Body::from(setup)).unwrap()).await;
    assert_eq!((status, told), (StatusCode::UNAUTHORIZED, json!({"error": "sign-in was refused"})));
}

#[tokio::test]
async fn an_address_that_keeps_guessing_passwords_is_made_to_wait_even_with_the_right_one_and_others_are_not() {
    let served = with_password();
    set_up(&served).await;
    for guess in 0..10 {
        let (status, _, _) = sign_in(&served, "198.51.100.9:4000", &format!("wrong guess number {guess}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _, headers) = sign_in(&served, "198.51.100.9:4001", GOOD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key(header::RETRY_AFTER));
    assert_eq!(sign_in(&served, "198.51.100.10:4000", GOOD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn guessing_spread_over_many_addresses_stops_password_sign_in_for_everyone() {
    let served = with_password();
    set_up(&served).await;
    // one wrong password from each of a thousand addresses: none of them alone is anywhere near its limit
    for number in 0..1000u32 {
        let remote = format!("198.18.{}.{}:4000", number / 250, number % 250);
        let (status, _, _) = sign_in(&served, &remote, "a wrong password, guessed").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(sign_in(&served, "192.0.2.200:4000", GOOD).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn signing_in_needs_the_interfaces_origin_and_a_json_body() {
    let served = with_password();
    let code = served.code.as_ref().unwrap().expose().to_owned();
    let body = json!({ "code": code, "password": GOOD }).to_string();
    for origin in [None, Some("https://evil.test"), Some("null")] {
        let mut request = asked("POST", "/auth/setup").header(header::CONTENT_TYPE, "application/json");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let (status, _, _) = answer(&served.router, request.body(Body::from(body.clone())).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    // a form another site could send: not JSON
    let form = asked("POST", "/auth/setup")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("code={code}&password=x")))
        .unwrap();
    assert_eq!(answer(&served.router, form).await.0, StatusCode::BAD_REQUEST);
    // and none of it used the code up
    assert!(served.code.is_some());
    set_up(&served).await;
}

#[tokio::test]
async fn a_change_made_with_the_session_needs_the_origin_and_the_sessions_own_token() {
    let served = with_password();
    let (cookie, csrf) = set_up(&served).await;
    let change = json!({ "clustering": "kmeans" }).to_string();
    let put = || with_cookie(asked("PUT", "/api/settings"), &cookie).header(header::CONTENT_TYPE, "application/json");

    let (status, told, _) =
        answer(&served.router, put().header(CSRF_HEADER, &csrf).body(Body::from(change.clone())).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no Origin: {told}");
    let (status, _, _) =
        answer(&served.router, put().header(header::ORIGIN, ORIGIN).body(Body::from(change.clone())).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no anti-forgery token");
    let wrong = put().header(header::ORIGIN, ORIGIN).header(CSRF_HEADER, "made-up-token");
    assert_eq!(answer(&served.router, wrong.body(Body::from(change.clone())).unwrap()).await.0, StatusCode::FORBIDDEN);
    let other_site = put().header(header::ORIGIN, "https://evil.test").header(CSRF_HEADER, &csrf);
    assert_eq!(
        answer(&served.router, other_site.body(Body::from(change.clone())).unwrap()).await.0,
        StatusCode::FORBIDDEN
    );

    let right = put().header(header::ORIGIN, ORIGIN).header(CSRF_HEADER, &csrf);
    let (status, told, _) = answer(&served.router, right.body(Body::from(change)).unwrap()).await;
    assert_eq!((status, told["clustering"].clone()), (StatusCode::OK, json!("kmeans")));
    // reading needs neither
    let (status, _, _) =
        answer(&served.router, with_cookie(asked("GET", "/api/settings"), &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_embedding_model_belongs_to_the_server_and_is_refused_from_the_interface() {
    let served = with_password();
    let (cookie, csrf) = set_up(&served).await;
    let put = |body: Value| {
        with_cookie(sent("PUT", "/api/settings"), &cookie)
            .header(CSRF_HEADER, &csrf)
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (status, told, _) = answer(&served.router, put(json!({ "embedding_model": "BAAI/bge-large-en-v1.5" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(told["error"].as_str().unwrap().starts_with("embedding_model is decided by the server"));
    // sent unchanged, with another change: fine
    let (status, told, _) =
        answer(&served.router, put(json!({ "embedding_model": "Qwen/Qwen3-Embedding-0.6B", "max_links": 5 }))).await;
    assert_eq!((status, told["max_links"].clone()), (StatusCode::OK, json!(5)));
    // nor any other way of changing what the server owns: with other keys, as null, in another case, the
    // provider instead of the model
    for sneaked in [
        json!({ "max_links": 6, "embedding_model": "BAAI/bge-large-en-v1.5" }),
        json!({ "embedding_model": null }),
        json!({ "embedding_model": "qwen/qwen3-embedding-0.6b" }),
        json!({ "describe_provider": "openai" }),
        json!({ "describe_provider": "Anthropic" }),
        json!({ "describe_provider": null, "describe_model": "claude-opus-5-5" }),
        json!({ "reading_provider": "openai" }),
        json!({ "reading_provider": "openai", "reading_model": "gpt-6.2-sol" }),
    ] {
        let (status, _, _) = answer(&served.router, put(sneaked.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{sneaked}");
    }
    // while the server reads papers with its naming model, a window choosing a model or effort for reading would
    // pin the reading provider where the naming one is now: only the owner does that
    for pinning in [
        json!({ "reading_model": "claude-opus-5-5" }),
        json!({ "reading_effort": "low" }),
        json!({ "reading_provider": "anthropic" }),
    ] {
        let (status, _, _) = answer(&served.router, put(pinning.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{pinning}");
    }
    // sent as it is (null, or the naming provider it means) changes nothing and is fine
    let (status, told, _) = answer(&served.router, put(json!({ "reading_provider": null }))).await;
    assert_eq!((status, told["reading_provider"].clone()), (StatusCode::OK, Value::Null));
    // the owner chose another provider to read papers, in the server's own file: a window may pick its effort, but
    // neither set it back to the naming model (papers would go elsewhere) nor move it
    let file = served._data.path().join("settings.json");
    let mut stored: serde_json::Map<String, Value> = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    stored.extend([
        ("reading_provider".to_owned(), json!("openai")),
        ("reading_model".to_owned(), json!("gpt-6.2-sol")),
        ("reading_effort".to_owned(), json!("medium")),
    ]);
    std::fs::write(&file, Value::Object(stored).to_string()).unwrap();
    let (status, told, _) = answer(&served.router, put(json!({ "reading_effort": "low" }))).await;
    assert_eq!((status, told["reading_provider"].clone()), (StatusCode::OK, json!("openai")));
    for moving in [json!({ "reading_provider": null }), json!({ "reading_provider": "anthropic" })] {
        let (status, _, _) = answer(&served.router, put(moving.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{moving}");
    }
    // a key spelled differently is no setting at all: ignored, and nothing changes
    let (status, told, _) = answer(&served.router, put(json!({ "Embedding_Model": "BAAI/bge-large-en-v1.5" }))).await;
    assert_eq!((status, told["embedding_model"].clone()), (StatusCode::OK, json!("Qwen/Qwen3-Embedding-0.6B")));
    assert_eq!((told["describe_provider"].clone(), told["max_links"].clone()), (json!("anthropic"), json!(5)));
    let (_, told, _) =
        answer(&served.router, with_cookie(asked("GET", "/api/status"), &cookie).body(Body::empty()).unwrap()).await;
    let owned = json!(["embedding_model", "describe_provider", "reading_provider"]);
    assert_eq!((told["mode"].clone(), told["server_owned"].clone()), (json!("network"), owned));
}

#[tokio::test]
async fn signing_in_again_issues_a_new_session_and_ends_the_one_the_browser_had() {
    let served = with_password();
    let (old, _) = set_up(&served).await;
    let body = json!({ "password": GOOD }).to_string();
    let request = with_cookie(sent("POST", "/auth/password"), &old).body(Body::from(body)).unwrap();
    let (status, _, headers) = answer(&served.router, request).await;
    assert_eq!(status, StatusCode::OK);
    let new = value_of(&set_cookie(&headers, SESSION).unwrap());
    assert_ne!(new, old);
    let status_with = |cookie: String| with_cookie(asked("GET", "/api/status"), &cookie).body(Body::empty()).unwrap();
    assert_eq!(answer(&served.router, status_with(old)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(answer(&served.router, status_with(new)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn signing_out_ends_the_session_and_signing_out_everywhere_ends_every_one() {
    let served = with_password();
    let (first, first_csrf) = set_up(&served).await;
    let (_, told, headers) = sign_in(&served, "198.51.100.1:4000", GOOD).await;
    let second = value_of(&set_cookie(&headers, SESSION).unwrap());
    let second_csrf = told["csrf"].as_str().unwrap().to_owned();
    let works = |cookie: &str| with_cookie(asked("GET", "/api/status"), cookie).body(Body::empty()).unwrap();

    // signing out is a change like any other: not without the token (no other site can sign the owner out)
    let forged = with_cookie(sent("POST", "/auth/signout"), &first).body(Body::empty()).unwrap();
    assert_eq!(answer(&served.router, forged).await.0, StatusCode::FORBIDDEN);
    let out = with_cookie(sent("POST", "/auth/signout"), &first)
        .header(CSRF_HEADER, &first_csrf)
        .body(Body::empty())
        .unwrap();
    let (status, _, headers) = answer(&served.router, out).await;
    assert_eq!(status, StatusCode::OK);
    assert!(set_cookie(&headers, SESSION).unwrap().contains("Max-Age=0"));
    assert_eq!(answer(&served.router, works(&first)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(answer(&served.router, works(&second)).await.0, StatusCode::OK);

    let (_, _, headers) = sign_in(&served, "198.51.100.1:4000", GOOD).await;
    let third = value_of(&set_cookie(&headers, SESSION).unwrap());
    let everywhere = with_cookie(sent("POST", "/auth/signout-everywhere"), &second)
        .header(CSRF_HEADER, &second_csrf)
        .body(Body::empty())
        .unwrap();
    assert_eq!(answer(&served.router, everywhere).await.0, StatusCode::OK);
    assert_eq!(answer(&served.router, works(&second)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(answer(&served.router, works(&third)).await.0, StatusCode::UNAUTHORIZED);
    // signing out without a session is refused like any other route
    assert_eq!(
        answer(&served.router, sent("POST", "/auth/signout").body(Body::empty()).unwrap()).await.0,
        StatusCode::UNAUTHORIZED
    );
}

/// Start a sign-in with the provider: answers the state and nonce sent to it, and the flow cookie.
async fn started(served: &Served) -> (String, String, String) {
    let (status, _, headers) = answer(&served.router, asked("GET", "/auth/start").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = Url::parse(headers[header::LOCATION].to_str().unwrap()).unwrap();
    assert!(location.as_str().starts_with("https://provider.test/authorize?"));
    let query = |name: &str| location.query_pairs().find(|(key, _)| key == name).unwrap().1.into_owned();
    assert_eq!(query("redirect_uri"), format!("{ORIGIN}/auth/callback"));
    let flow = set_cookie(&headers, "__Host-constellate-flow").unwrap();
    assert!(flow.contains("SameSite=Lax") && flow.contains("Secure") && flow.contains("HttpOnly"));
    (query("state"), query("nonce"), value_of(&flow))
}

fn come_back(state: &str, flow: Option<&str>) -> Request<Body> {
    let mut request = asked("GET", &format!("/auth/callback?code=the-code&state={state}"));
    if let Some(flow) = flow {
        request = request.header(header::COOKIE, format!("__Host-constellate-flow={flow}"));
    }
    request.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn the_owner_signs_in_with_the_provider_and_comes_back_signed_in() {
    let signer = rsa_signer(0, "k1");
    let served = with_provider(&signer);
    assert!(served.code.is_none(), "no password, no setup code");
    let (state, nonce, flow) = started(&served).await;
    served.fake.script().id_token = Some(signer.sign(&claims(&nonce)));
    let (status, _, headers) = answer(&served.router, come_back(&state, Some(&flow))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
    let cookie = value_of(&set_cookie(&headers, SESSION).unwrap());
    assert!(set_cookie(&headers, "__Host-constellate-flow").unwrap().contains("Max-Age=0"));
    let (status, _, _) =
        answer(&served.router, with_cookie(asked("GET", "/api/status"), &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);

    // the same way back again (a replayed state and code) is refused
    let (status, _, headers) = answer(&served.router, come_back(&state, Some(&flow))).await;
    assert_eq!((status, headers[header::LOCATION].to_str().unwrap()), (StatusCode::SEE_OTHER, "/auth/?said=refused"));
    assert!(set_cookie(&headers, SESSION).is_none());
}

#[tokio::test]
async fn a_sign_in_with_the_provider_is_refused_for_another_person_or_another_browser() {
    let signer = rsa_signer(0, "k1");
    let served = with_provider(&signer);
    let (state, nonce, _) = started(&served).await;
    served.fake.script().id_token = Some(signer.sign(&claims(&nonce)));
    let (status, _, headers) = answer(&served.router, come_back(&state, None)).await;
    assert_eq!((status, set_cookie(&headers, SESSION)), (StatusCode::SEE_OTHER, None));

    let (state, nonce, flow) = started(&served).await;
    let mut someone_else = claims(&nonce);
    someone_else["email"] = json!("someone.else@example.org");
    served.fake.script().id_token = Some(signer.sign(&someone_else));
    let (status, told, headers) = answer(&served.router, come_back(&state, Some(&flow))).await;
    assert_eq!((status, set_cookie(&headers, SESSION)), (StatusCode::SEE_OTHER, None));
    // nothing in the answer names the owner
    let said = format!("{told} {headers:?}").to_lowercase();
    assert!(!said.contains("owner.name"), "{said}");
}

#[tokio::test]
async fn with_tokens_allowed_a_program_uses_the_token_without_a_session_or_origin_and_a_wrong_one_counts() {
    let served = served_with(
        &[("CONSTELLATE_ALLOW_PASSWORD", "true"), ("CONSTELLATE_ALLOW_TOKEN", "true"), ("CONSTELLATE_TOKEN", STRONG)],
        &[],
    );
    let change = asked("PUT", "/api/settings")
        .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "max_links": 4 }).to_string()))
        .unwrap();
    assert_eq!(answer(&served.router, change).await.0, StatusCode::OK);
    for guess in 0..10 {
        let wrong = asked("GET", "/api/status").header(header::AUTHORIZATION, format!("Bearer guess{guess}"));
        assert_eq!(
            answer(&served.router, from(wrong.body(Body::empty()).unwrap(), "198.51.100.3:1")).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let right = asked("GET", "/api/status").header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
    assert_eq!(
        answer(&served.router, from(right.body(Body::empty()).unwrap(), "198.51.100.3:2")).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // the owner behind the same address (a proxy, a NAT) is not locked out by someone else's guesses
    let (cookie, _) = set_up(&served).await;
    let owner = from(with_cookie(asked("GET", "/api/status"), &cookie).body(Body::empty()).unwrap(), "198.51.100.3:3");
    assert_eq!(answer(&served.router, owner).await.0, StatusCode::OK);
}

#[tokio::test]
async fn over_plain_http_by_choice_the_cookie_has_no_prefix_and_no_secure() {
    let base = [
        ("CONSTELLATE_BIND", "192.0.2.7"),
        ("CONSTELLATE_USE_HTTPS", "false"),
        ("CONSTELLATE_ORIGIN", "http://papers.test"),
        ("CONSTELLATE_ALLOW_PASSWORD", "true"),
    ];
    let listen = Listen::from_environment(&Environment::from_pairs(base)).unwrap();
    let data = tempfile::tempdir().unwrap();
    let (router, _, code) = crate::api::application(
        &listen,
        AT.parse().unwrap(),
        Engine::new(data.path().to_path_buf(), choices()),
        routes::routes(),
        Arc::new(Fake::new(&[])),
        || Ok(Hashing::light()),
    )
    .unwrap();
    let body = json!({ "code": code.unwrap().expose(), "password": GOOD }).to_string();
    let request = asked("POST", "/auth/setup")
        .header(header::ORIGIN, "http://papers.test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _, headers) = answer(&router, request).await;
    assert_eq!(status, StatusCode::OK);
    let set = set_cookie(&headers, "constellate").unwrap();
    assert!(!set.contains("Secure") && set.contains("HttpOnly") && set.contains("SameSite=Strict"));
    let (_, told, _) = answer(
        &router,
        asked("GET", "/api/status")
            .header(header::COOKIE, format!("constellate={}", value_of(&set)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(told["warnings"].to_string().contains("cookie is sent without Secure"));
}

#[tokio::test]
async fn on_this_computer_nothing_changes_no_sign_in_routes_and_the_token_is_all_that_is_asked() {
    let listen = Listen::from_environment(&Environment::from_pairs([("CONSTELLATE_TOKEN", STRONG)])).unwrap();
    let data = tempfile::tempdir().unwrap();
    let (router, _, code) = crate::api::application(
        &listen,
        "127.0.0.1:8765".parse().unwrap(),
        Engine::new(data.path().to_path_buf(), choices()),
        routes::routes(),
        Arc::new(Fake::new(&[])),
        || panic!("nothing is hashed on this computer"),
    )
    .unwrap();
    assert!(code.is_none());
    let local =
        |method: &str, path: &str| Request::builder().method(method).uri(path).header(header::HOST, "127.0.0.1:8765");
    for path in ["/auth/", "/auth/session", "/auth/start"] {
        assert_eq!(
            answer(&router, local("GET", path).body(Body::empty()).unwrap()).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
        let with_token = local("GET", path).header(header::AUTHORIZATION, format!("Bearer {STRONG}"));
        assert_eq!(answer(&router, with_token.body(Body::empty()).unwrap()).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, told, _) = answer(
        &router,
        local("GET", "/api/status")
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, told["mode"].clone(), told["server_owned"].clone()),
        (StatusCode::OK, json!("local"), json!([]))
    );
    // the embedding model is the person's own to change, and a change needs no Origin or session token
    let change = local("PUT", "/api/settings")
        .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "embedding_model": "BAAI/bge-large-en-v1.5" }).to_string()))
        .unwrap();
    let (status, told, _) = answer(&router, change).await;
    assert_eq!((status, told["embedding_model"].clone()), (StatusCode::OK, json!("BAAI/bge-large-en-v1.5")));
    // a page that is opened gets the same 401 as before, not a sign-in page
    let opened = local("GET", "/").header("sec-fetch-mode", "navigate").body(Body::empty()).unwrap();
    assert_eq!(answer(&router, opened).await.0, StatusCode::UNAUTHORIZED);
}
