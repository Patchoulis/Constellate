//! The checks every request passes before any route sees it, and the headers every response carries.
//!
//! The engine can spend money (Claude) and delete runs, so it answers only:
//!
//! * requests **addressed to it** under the name it is served at (`Host`): a web page cannot reach it through a
//!   name of its own that it points at this address (DNS rebinding);
//! * from **no browser page but the interface** (`Origin`): another site open in a browser here is refused even
//!   before the token is looked at (cross-site request forgery);
//! * on this computer, carrying **the token** (`Authorization: Bearer <token>`), compared in constant time:
//!   another program on this computer cannot use the engine without it;
//! * across a network, from **the owner, signed in** ([`super::login`]): every route but the sign-in routes
//!   themselves needs the owner's session, and a request that changes something needs the `Origin` header and
//!   the session's anti-forgery token as well. The token is accepted instead of a session only when the owner
//!   allowed it for programs (`CONSTELLATE_ALLOW_TOKEN=true`).
//!
//! These checks are one middleware around the whole router, unknown paths included ([`check`]): a route added
//! later is behind them without anyone having to remember it.
//!
//! A refused request is told only which rule it broke in words that give nothing away about the token: a missing
//! token and a wrong one get the same answer. Across a network, an address that keeps sending wrong tokens is
//! made to wait ([`Strikes`]), and so is one that keeps failing to sign in. No response ever carries a CORS
//! header: the interface reaches the engine through its own origin, so no other origin is ever allowed to read
//! an answer.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::config::{Access, Listen, Mode, Origin, Programs, Transport};
use super::login::oidc::Provider;
use super::login::password::{Hashing, PasswordError, SetupCode};
use super::login::{self, Login, LoginError};
use super::routes::ApiError;

/// The largest request body accepted: the largest thing the interface sends is a page's text.
pub const LARGEST_BODY: usize = 4 * 1024 * 1024;

pub(crate) const STRIKE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Wrong tokens: an address may send 10 within [`STRIKE_WINDOW`] before it is made to wait for the rest of it,
/// a whole IPv6 /64 network ([`Sender::Network`]) 200.
const WRONG_TOKENS: Allowance = Allowance { address: 10, network: 200, everyone: None };
/// Addresses remembered at once; beyond it the oldest are forgotten, so the count cannot grow without bound.
const MOST_REMEMBERED: usize = 10_000;
/// The Content-Security-Policy of every answer that is not a page: nothing may load, nothing may frame it.
const NOTHING_ALLOWED: &str = "default-src 'none'; frame-ancestors 'none'";

/// How many failures are allowed within [`STRIKE_WINDOW`], to whom.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Allowance {
    /// One address.
    pub(crate) address: u32,
    /// An IPv6 /64 network.
    pub(crate) network: u32,
    /// Everyone together, when there is such a limit: against guessing spread over many addresses.
    pub(crate) everyone: Option<u32>,
}

/// Which browser pages may send requests.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pages {
    /// Only the interface, at exactly this origin.
    Only(Origin),
    /// No origin was configured, on this computer: any page served from this computer (development). The token
    /// is still needed, and a page that is not the interface does not have it.
    FromThisComputer,
    /// No origin was configured, across a network: no browser page at all.
    None,
}

/// Everything the checks need, made once when the engine starts.
#[derive(Clone)]
pub struct Guard(Arc<Rules>);

struct Rules {
    door: Door,
    /// What `Host` may be: each name and address the engine is reached by, with its port.
    authorities: Vec<String>,
    pages: Pages,
    /// Strict-Transport-Security is sent only for HTTPS across a network: a browser applies it to every port of
    /// the host, and on `localhost` it would force the person's other local servers to HTTPS too.
    hsts: bool,
    /// Only across a network: on this computer, whoever could guess could also read the token's file.
    strikes: Option<Strikes>,
    /// On this computer: the name a forwarding server says the browser used must be this computer's too.
    local: bool,
}

/// A token's SHA-256: digests of the same length are compared, so the comparison takes the same time whatever
/// was sent, and says nothing about the token's length either.
type Digested = [u8; 32];

/// What lets a request in, as the checks hold it.
enum Door {
    /// On this computer: the token.
    Token(Digested),
    /// Across a network: the owner's session; or the token, if the owner allowed it for programs.
    SignIn { login: Arc<Login>, programs: Option<Digested> },
}

/// What lets a request in, made once at start from how the engine listens: [`Admission::new`] is the only way
/// to make one, so the door always matches the mode.
pub struct Admission(Door);

impl Admission {
    /// The door for `listen`. Across a network that is the owner's sign-in, with its password kept in `data`,
    /// the provider reached through `provider`, and passwords hashed with what `hashing` makes (only called
    /// across a network: it costs one Argon2 computation). Answers the setup code to print, when there is one.
    pub fn new(
        listen: &Listen,
        data: &Path,
        provider: Arc<dyn Provider>,
        hashing: impl FnOnce() -> Result<Hashing, PasswordError>,
    ) -> Result<(Admission, Option<SetupCode>), LoginError> {
        match listen.access() {
            Access::Token { token, .. } => Ok((Admission(Door::Token(digest(token.expose().as_bytes()))), None)),
            Access::SignIn { sign_in, programs } => {
                let (login, code) = Login::new(sign_in.clone(), data, provider, hashing()?, Instant::now())?;
                let programs = match programs {
                    Programs::Refused => None,
                    Programs::Token(token) => Some(digest(token.expose().as_bytes())),
                };
                Ok((Admission(Door::SignIn { login: Arc::new(login), programs }), code))
            }
        }
    }

    /// The owner's sign-in, across a network: its routes are served too.
    pub fn login(&self) -> Option<Arc<Login>> {
        match &self.0 {
            Door::Token(_) => None,
            Door::SignIn { login, .. } => Some(Arc::clone(login)),
        }
    }
}

impl Guard {
    /// The rules for `listen`, now that it is bound to `bound` (whose port is the real one), with the door
    /// made for it.
    pub fn new(listen: &Listen, bound: SocketAddr, admission: Admission) -> Guard {
        let https = matches!(listen.transport(), Transport::Https(_));
        let hsts = https && listen.mode() == Mode::Network;
        let default_port = if https { 443 } else { 80 };
        let address = match bound.ip() {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let mut authorities = Vec::new();
        for name in std::iter::once(address).chain(listen.names()) {
            if bound.port() == default_port {
                authorities.push(name.clone());
            }
            authorities.push(format!("{name}:{}", bound.port()));
        }
        let pages = match (listen.origin(), listen.mode()) {
            (Some(origin), _) => Pages::Only(origin.clone()),
            (None, Mode::Local) => Pages::FromThisComputer,
            (None, Mode::Network) => Pages::None,
        };
        let strikes = (listen.mode() == Mode::Network).then(|| Strikes::new(WRONG_TOKENS));
        let local = listen.mode() == Mode::Local;
        Guard(Arc::new(Rules { door: admission.0, authorities, pages, hsts, strikes, local }))
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The one value of a header that must appear at most once; `Err` when it appears more often or is not text.
fn single(headers: &HeaderMap, name: HeaderName) -> Result<Option<&str>, ()> {
    let mut values = headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (None, _) => Ok(None),
        (Some(value), None) => value.to_str().map(Some).map_err(|_| ()),
        (Some(_), Some(_)) => Err(()),
    }
}

impl Rules {
    /// Is the request addressed to this engine? HTTP/1 says where in `Host`, HTTP/2 in the request's authority;
    /// whichever are present must be one of ours.
    fn addressed_to_us(&self, request: &Request) -> bool {
        let ours = |authority: &str| self.authorities.iter().any(|one| one.eq_ignore_ascii_case(authority));
        let host = match single(request.headers(), header::HOST) {
            Ok(host) => host,
            Err(()) => return false,
        };
        let authority = request.uri().authority().map(|authority| authority.as_str());
        match (host, authority) {
            (None, None) => false,
            (Some(one), None) | (None, Some(one)) => ours(one),
            (Some(host), Some(authority)) => ours(host) && ours(authority),
        }
    }

    /// On this computer, was the request forwarded (by the interface's server, which rewrites `Host` to the
    /// engine's own address) for a name that is this computer's? A page whose own name was pointed at this
    /// computer (DNS rebinding) reaches the interface's server under that name, which arrives here only in
    /// `X-Forwarded-Host`. Across a network the front's public name is the owner's to choose, and is not checked.
    fn forwarded_for_us(&self, headers: &HeaderMap) -> bool {
        !self.local
            || headers.get_all("x-forwarded-host").iter().all(|value| {
                value.to_str().is_ok_and(|value| {
                    !value.contains(',')
                        && Origin::parse(&format!("http://{}", value.trim()))
                            .is_some_and(|origin| from_this_computer(&origin))
                })
            })
    }

    /// Does the request come from no browser page, or from one that may send it?
    fn sent_by_an_allowed_page(&self, headers: &HeaderMap) -> bool {
        let origin = match single(headers, header::ORIGIN) {
            Ok(None) => return true, // not sent by a browser page: the token decides
            Ok(Some(origin)) => origin,
            Err(()) => return false,
        };
        match &self.pages {
            Pages::Only(expected) => Origin::parse(origin).is_some_and(|origin| &origin == expected),
            Pages::FromThisComputer => Origin::parse(origin).is_some_and(|origin| from_this_computer(&origin)),
            Pages::None => false,
        }
    }
}

fn carries_the_token(headers: &HeaderMap, token: &Digested) -> bool {
    let Ok(Some(value)) = single(headers, header::AUTHORIZATION) else {
        return false;
    };
    let Some((scheme, sent)) = value.split_once(' ') else {
        return false;
    };
    // the scheme's name is case-insensitive (RFC 9110); the token is compared as sent
    scheme.eq_ignore_ascii_case("bearer") && bool::from(digest(sent.as_bytes()).ct_eq(token))
}

/// Does the request change something? Everything but reading does.
fn changes_something(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Is the origin's host this computer: `localhost`, `127.x.x.x` or `[::1]`?
fn from_this_computer(origin: &Origin) -> bool {
    let authority = origin.as_str().split_once("://").map_or("", |(_, authority)| authority);
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// Who failures are counted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Sender {
    /// One address.
    Address(IpAddr),
    /// An IPv6 /64 network: a machine is usually given a whole /64, and could otherwise take a new address for
    /// every guess. It is allowed far more failures than one address, since its other addresses are other
    /// machines on the same network (the person's own among them), which one guesser must not lock out cheaply.
    Network([u16; 4]),
    /// Everyone together.
    Everyone,
}

impl Sender {
    /// What a remote address counts against: itself, for IPv6 its /64 too, and everyone when there is such a
    /// limit. A request whose address is not known counts against everyone only.
    fn all(remote: Option<IpAddr>, allowance: Allowance) -> impl Iterator<Item = Sender> {
        let remote = remote.map(|remote| remote.to_canonical());
        let network = match remote {
            Some(IpAddr::V6(ip)) => {
                let segments = ip.segments();
                Some(Sender::Network([segments[0], segments[1], segments[2], segments[3]]))
            }
            Some(IpAddr::V4(_)) | None => None,
        };
        let everyone = allowance.everyone.map(|_| Sender::Everyone);
        remote.map(Sender::Address).into_iter().chain(network).chain(everyone)
    }

    fn most(self, allowance: Allowance) -> u32 {
        match self {
            Sender::Address(_) => allowance.address,
            Sender::Network(_) => allowance.network,
            Sender::Everyone => allowance.everyone.unwrap_or(u32::MAX),
        }
    }
}

/// Failures counted per sender, in a window that starts with its first failure.
pub(crate) struct Strikes {
    allowance: Allowance,
    counted: Mutex<HashMap<Sender, (u32, Instant)>>,
}

impl Strikes {
    pub(crate) fn new(allowance: Allowance) -> Strikes {
        Strikes { allowance, counted: Mutex::new(HashMap::new()) }
    }

    pub(crate) fn is_waiting(&self, remote: Option<IpAddr>, now: Instant) -> bool {
        let counted = self.counted.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Sender::all(remote, self.allowance).any(|sender| {
            counted.get(&sender).is_some_and(|(count, since)| {
                *count >= sender.most(self.allowance) && now.duration_since(*since) < STRIKE_WINDOW
            })
        })
    }

    pub(crate) fn strike(&self, remote: Option<IpAddr>, now: Instant) {
        let mut counted = self.counted.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for sender in Sender::all(remote, self.allowance) {
            if counted.len() >= MOST_REMEMBERED && !counted.contains_key(&sender) {
                counted.retain(|_, (_, since)| now.duration_since(*since) < STRIKE_WINDOW);
                if counted.len() >= MOST_REMEMBERED {
                    // never the count of everyone: many addresses must not be a way to reset it
                    let oldest = counted
                        .iter()
                        .filter(|(sender, _)| **sender != Sender::Everyone)
                        .min_by_key(|(_, (_, since))| *since)
                        .map(|(sender, _)| *sender);
                    if let Some(oldest) = oldest {
                        counted.remove(&oldest);
                    }
                }
            }
            let entry = counted.entry(sender).or_insert((0, now));
            if now.duration_since(entry.1) >= STRIKE_WINDOW {
                *entry = (0, now); // a new window
            }
            entry.0 = entry.0.saturating_add(1);
        }
    }
}

/// The address a request came from, when the server recorded it (tests may not).
pub(crate) fn sender(extensions: &axum::http::Extensions) -> Option<IpAddr> {
    extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(remote)| remote.ip())
}

/// The checks, as middleware: every request passes them in this order, before any route.
pub async fn check(State(guard): State<Guard>, request: Request, next: Next) -> Response {
    let rules = &guard.0;
    let refusal = refusal(rules, &request);
    let mut response = match refusal {
        // a person who opened a page of the engine without being signed in is shown where to sign in
        Some(ApiError::SignInNeeded) if is_navigation(&request) => {
            let mut response = StatusCode::SEE_OTHER.into_response();
            response.headers_mut().insert(header::LOCATION, HeaderValue::from_static(login::PAGE));
            response
        }
        Some(error) => error.into_response(),
        None => next.run(request).await,
    };
    told_as_json(&mut response);
    protect(response.headers_mut(), rules.hsts);
    response
}

/// Is this a browser opening a page (rather than a script asking for data)?
fn is_navigation(request: &Request) -> bool {
    let header = |name| request.headers().get(name).and_then(|value| value.to_str().ok()).unwrap_or("");
    request.method() == Method::GET
        && (header("sec-fetch-mode") == "navigate" || header(header::ACCEPT.as_str()).contains("text/html"))
}

/// Why the request is refused, if it is.
fn refusal(rules: &Rules, request: &Request) -> Option<ApiError> {
    let sender = sender(request.extensions());
    let now = Instant::now();
    // An address that sent too many wrong tokens is waiting: no token it sends is looked at, so that it learns
    // nothing more, not even from a right guess. The owner's session from that address is still let in (it
    // gives nothing away about the token), so that guesses sent through a shared address (a proxy in front of
    // the engine, a NAT) cannot lock the signed-in owner out.
    let waiting = match (&rules.strikes, sender) {
        (Some(strikes), Some(sender)) => strikes.is_waiting(Some(sender), now),
        _ => false,
    };
    if !rules.addressed_to_us(request) || !rules.forwarded_for_us(request.headers()) {
        return Some(ApiError::Forbidden("this engine answers only requests addressed to it"));
    }
    if !rules.sent_by_an_allowed_page(request.headers()) {
        return Some(ApiError::Forbidden("requests from other sites are refused"));
    }
    if let Err(error) = admitted(rules, request, waiting, now) {
        if let (ApiError::Unauthorized, Some(strikes), Some(sender)) = (&error, &rules.strikes, sender) {
            strikes.strike(Some(sender), now);
        }
        return Some(error);
    }
    // refused at once when it says it is too large; one that does not say is cut off at the limit as it is read
    let length = request.headers().get(header::CONTENT_LENGTH).and_then(|value| value.to_str().ok());
    if length.is_some_and(|length| length.parse::<u64>().map_or(true, |length| length > LARGEST_BODY as u64)) {
        return Some(ApiError::TooLarge);
    }
    None
}

/// Does the request come through the door? [`ApiError::Unauthorized`] (a wrong token) counts as a strike.
/// `waiting`: the sender sent too many wrong tokens, and no token of its is looked at.
fn admitted(rules: &Rules, request: &Request, waiting: bool, now: Instant) -> Result<(), ApiError> {
    let headers = request.headers();
    let (login, programs) = match &rules.door {
        Door::Token(_) if waiting => return Err(ApiError::TooManyAttempts),
        Door::Token(token) => {
            return if carries_the_token(headers, token) { Ok(()) } else { Err(ApiError::Unauthorized) };
        }
        Door::SignIn { login, programs } => (login, programs),
    };
    // Origin was checked above when it was sent; across a network a request that changes something must send
    // it. Only a browser sends cookies by itself, and every browser sends Origin with such a request.
    let changes = changes_something(request.method());
    let without_origin = changes && !headers.contains_key(header::ORIGIN);
    if login::is_public(request.method(), request.uri().path()) {
        // signing in changes something too: who is signed in (no other site may sign the owner's browser in)
        return if without_origin {
            Err(ApiError::Forbidden("signing in must be done from the sign-in page"))
        } else {
            Ok(())
        };
    }
    if let Some(session) = login.session(headers, now) {
        if without_origin {
            return Err(ApiError::Forbidden("a request that changes something must come from the interface"));
        }
        if changes && !session.vouched_for(headers) {
            return Err(ApiError::Forbidden("a request that changes something must carry the session's token"));
        }
        return Ok(());
    }
    if headers.contains_key(header::AUTHORIZATION) {
        // a program's token is not sent by a browser by itself, so it needs neither Origin nor a session token
        return match programs {
            _ if waiting => Err(ApiError::TooManyAttempts),
            Some(token) if carries_the_token(headers, token) => Ok(()),
            _ => Err(ApiError::Unauthorized),
        };
    }
    Err(ApiError::SignInNeeded)
}

/// The answers the router and its layers give with an empty body (a method a route does not take, a request
/// that took too long) get the same `{ "error": ... }` as every other refusal.
fn told_as_json(response: &mut Response) {
    let said = match response.status() {
        StatusCode::METHOD_NOT_ALLOWED => "this request is not taken here",
        StatusCode::REQUEST_TIMEOUT => "the request took too long to arrive",
        _ => return,
    };
    if response.headers().contains_key(header::CONTENT_TYPE) {
        return;
    }
    let headers = response.headers_mut();
    headers.remove(header::CONTENT_LENGTH);
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    *response.body_mut() = Body::from(serde_json::json!({ "error": said }).to_string());
}

/// The headers every response carries, refusals and errors included.
fn protect(headers: &mut HeaderMap, hsts: bool) {
    let set = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
    ];
    for (name, value) in set {
        headers.insert(name, HeaderValue::from_static(value));
    }
    // nothing may frame an answer, unless the route says its own page may (a paper's PDF, shown in the reader's
    // frame; SAMEORIGIN at most, which is all a route ever sets)
    if headers.get(header::X_FRAME_OPTIONS).is_none_or(|value| value != "SAMEORIGIN") {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    // a policy that allows nothing, unless the route serves a page and set its own (the sign-in page's, which
    // allows only its own script and style)
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(NOTHING_ALLOWED));
    }
    if hsts {
        headers.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=31536000"));
    }
    // whatever a route might add: no other origin may ever read an answer
    let cors: Vec<HeaderName> =
        headers.keys().filter(|name| name.as_str().starts_with("access-control-")).cloned().collect();
    for name in cors {
        headers.remove(name);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_guesser_on_an_ipv6_network_does_not_lock_out_its_neighbours_unless_the_network_keeps_guessing() {
        let strikes = Strikes::new(WRONG_TOKENS);
        let now = Instant::now();
        let (guesser, neighbour): (IpAddr, IpAddr) =
            ("2001:db8:1:2:aaaa::1".parse().unwrap(), "2001:db8:1:2:bbbb::9".parse().unwrap());
        for _ in 0..WRONG_TOKENS.address {
            strikes.strike(Some(guesser), now);
        }
        assert!(strikes.is_waiting(Some(guesser), now));
        assert!(!strikes.is_waiting(Some(neighbour), now));
        // a new address for every guess: the network as a whole is then made to wait
        for number in 0..WRONG_TOKENS.network {
            let rotating = IpAddr::from([0x2001, 0xdb8, 1, 2, 0xcccc, 0, 0, u16::try_from(number).unwrap()]);
            strikes.strike(Some(rotating), now);
        }
        assert!(strikes.is_waiting(Some(neighbour), now));
        assert!(!strikes.is_waiting(Some("2001:db8:1:3::1".parse().unwrap()), now));
        // an IPv4 address written as IPv6 is the IPv4 address
        for _ in 0..WRONG_TOKENS.address {
            strikes.strike(Some("::ffff:192.0.2.1".parse().unwrap()), now);
        }
        assert!(strikes.is_waiting(Some("192.0.2.1".parse().unwrap()), now));
    }

    #[test]
    fn a_sender_waits_after_too_many_wrong_tokens_until_its_window_ends() {
        let strikes = Strikes::new(WRONG_TOKENS);
        let (sender, start) = ("192.0.2.1".parse().unwrap(), Instant::now());
        for _ in 0..WRONG_TOKENS.address {
            assert!(!strikes.is_waiting(Some(sender), start));
            strikes.strike(Some(sender), start);
        }
        assert!(strikes.is_waiting(Some(sender), start));
        assert!(!strikes.is_waiting(Some("192.0.2.2".parse().unwrap()), start)); // others are not held up
        assert!(!strikes.is_waiting(Some(sender), start + STRIKE_WINDOW));
    }

    #[test]
    fn the_remembered_senders_never_grow_beyond_their_limit() {
        let strikes = Strikes::new(WRONG_TOKENS);
        let now = Instant::now();
        for number in 0..(MOST_REMEMBERED as u32 + 50) {
            strikes.strike(Some(IpAddr::from(number.to_be_bytes())), now);
        }
        assert_eq!(strikes.counted.lock().unwrap().len(), MOST_REMEMBERED);
    }

    #[test]
    fn failures_spread_over_many_addresses_make_everyone_wait_where_there_is_a_limit_for_everyone() {
        let strikes = Strikes::new(Allowance { address: 10, network: 100, everyone: Some(50) });
        let now = Instant::now();
        for number in 0..50u32 {
            strikes.strike(Some(IpAddr::from((0xc000_0200 + number).to_be_bytes())), now);
        }
        assert!(strikes.is_waiting(Some("198.51.100.1".parse().unwrap()), now));
        assert!(strikes.is_waiting(None, now));
        assert!(!strikes.is_waiting(Some("198.51.100.1".parse().unwrap()), now + STRIKE_WINDOW));
        // and many more addresses than are remembered do not reset the count of everyone
        for number in 0..(MOST_REMEMBERED as u32 + 10) {
            strikes.strike(Some(IpAddr::from((0x0a00_0000 + number).to_be_bytes())), now + STRIKE_WINDOW);
        }
        assert!(strikes.counted.lock().unwrap().contains_key(&Sender::Everyone));
        // without a limit for everyone, nothing is counted for an unknown address
        let tokens = Strikes::new(WRONG_TOKENS);
        tokens.strike(None, now);
        assert!(tokens.counted.lock().unwrap().is_empty());
    }

    #[test]
    fn pages_from_this_computer_are_told_apart_from_others() {
        for local in ["http://localhost:3000", "http://127.0.0.1:1", "https://[::1]:8443"] {
            assert!(from_this_computer(&Origin::parse(local).unwrap()), "{local}");
        }
        for other in ["http://localhost.evil.example", "http://127.0.0.1.evil.example", "https://evil.example"] {
            assert!(!from_this_computer(&Origin::parse(other).unwrap()), "{other}");
        }
    }
}
