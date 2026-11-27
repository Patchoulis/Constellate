//! The checks every request passes before any route sees it, and the headers every response carries.
//!
//! The engine can spend money (Claude) and delete runs, so it answers only:
//!
//! * requests **addressed to it** under the name it is served at (`Host`): a web page cannot reach it through a
//!   name of its own that it points at this address (DNS rebinding);
//! * from **no browser page but the interface** (`Origin`): another site open in a browser here is refused even
//!   before the token is looked at (cross-site request forgery);
//! * carrying **the token** (`Authorization: Bearer <token>`), compared in constant time: another program on
//!   this computer, or anyone on the network, cannot use the engine without it.
//!
//! A refused request is told only which rule it broke in words that give nothing away about the token: a missing
//! token and a wrong one get the same answer. Across a network, an address that keeps sending wrong tokens is
//! made to wait ([`Strikes`]). No response ever carries a CORS header: the interface reaches the engine through
//! its own origin, so no other origin is ever allowed to read an answer.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::config::{Listen, Mode, Origin, Transport};
use super::routes::ApiError;

/// The largest request body accepted: the largest thing the interface sends is a page's text.
pub const LARGEST_BODY: usize = 4 * 1024 * 1024;

/// Wrong tokens an address may send within [`STRIKE_WINDOW`] before it is made to wait for the rest of it.
const MOST_STRIKES: u32 = 10;
/// The same for a whole IPv6 /64 network ([`Sender::Network`]).
const MOST_NETWORK_STRIKES: u32 = 200;
pub(crate) const STRIKE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Addresses remembered at once; beyond it the oldest are forgotten, so the count cannot grow without bound.
const MOST_REMEMBERED: usize = 10_000;

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
    /// The token's SHA-256: digests of the same length are compared, so the comparison takes the same time
    /// whatever was sent, and says nothing about the token's length either.
    token: [u8; 32],
    /// What `Host` may be: each name and address the engine is reached by, with its port.
    authorities: Vec<String>,
    pages: Pages,
    /// Strict-Transport-Security is sent only for HTTPS across a network: a browser applies it to every port of
    /// the host, and on `localhost` it would force the person's other local servers to HTTPS too.
    hsts: bool,
    /// Only across a network: on this computer, whoever could guess could also read the token's file.
    strikes: Option<Strikes>,
}

impl Guard {
    /// The rules for `listen`, now that it is bound to `bound` (whose port is the real one).
    pub fn new(listen: &Listen, bound: SocketAddr) -> Guard {
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
        let strikes = (listen.mode() == Mode::Network).then(Strikes::default);
        Guard(Arc::new(Rules { token: digest(listen.token().expose().as_bytes()), authorities, pages, hsts, strikes }))
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

    fn carries_the_token(&self, headers: &HeaderMap) -> bool {
        let Ok(Some(value)) = single(headers, header::AUTHORIZATION) else {
            return false;
        };
        let Some((scheme, token)) = value.split_once(' ') else {
            return false;
        };
        // the scheme's name is case-insensitive (RFC 9110); the token is compared as sent
        scheme.eq_ignore_ascii_case("bearer") && bool::from(digest(token.as_bytes()).ct_eq(&self.token))
    }
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

/// Who wrong tokens are counted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Sender {
    /// One address.
    Address(IpAddr),
    /// An IPv6 /64 network: a machine is usually given a whole /64, and could otherwise take a new address for
    /// every guess. It is allowed far more wrong tokens than one address, since its other addresses are other
    /// machines on the same network (the person's own among them), which one guesser must not lock out cheaply.
    Network([u16; 4]),
}

impl Sender {
    /// What a remote address counts against: itself, and for IPv6 its /64 too.
    fn all(remote: IpAddr) -> impl Iterator<Item = Sender> {
        let remote = remote.to_canonical();
        let network = match remote {
            IpAddr::V6(ip) => {
                let segments = ip.segments();
                Some(Sender::Network([segments[0], segments[1], segments[2], segments[3]]))
            }
            IpAddr::V4(_) => None,
        };
        std::iter::once(Sender::Address(remote)).chain(network)
    }

    fn most(self) -> u32 {
        match self {
            Sender::Address(_) => MOST_STRIKES,
            Sender::Network(_) => MOST_NETWORK_STRIKES,
        }
    }
}

/// Wrong tokens counted per sender, in a window that starts with its first wrong one.
#[derive(Default)]
struct Strikes(Mutex<HashMap<Sender, (u32, Instant)>>);

impl Strikes {
    fn is_waiting(&self, remote: IpAddr, now: Instant) -> bool {
        let counted = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Sender::all(remote).any(|sender| {
            counted
                .get(&sender)
                .is_some_and(|(count, since)| *count >= sender.most() && now.duration_since(*since) < STRIKE_WINDOW)
        })
    }

    fn strike(&self, remote: IpAddr, now: Instant) {
        let mut counted = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for sender in Sender::all(remote) {
            if counted.len() >= MOST_REMEMBERED && !counted.contains_key(&sender) {
                counted.retain(|_, (_, since)| now.duration_since(*since) < STRIKE_WINDOW);
                if counted.len() >= MOST_REMEMBERED {
                    let oldest = counted.iter().min_by_key(|(_, (_, since))| *since).map(|(sender, _)| *sender);
                    if let Some(oldest) = oldest {
                        counted.remove(&oldest);
                    }
                }
            }
            let entry = counted.entry(sender).or_insert((0, now));
            if now.duration_since(entry.1) >= STRIKE_WINDOW {
                *entry = (0, now); // a new window
            }
            entry.0 += 1;
        }
    }
}

/// The checks, as middleware: every request passes them in this order, before any route.
pub async fn check(State(guard): State<Guard>, request: Request, next: Next) -> Response {
    let rules = &guard.0;
    let refusal = refusal(rules, &request);
    let mut response = match refusal {
        Some(error) => error.into_response(),
        None => next.run(request).await,
    };
    told_as_json(&mut response);
    protect(response.headers_mut(), rules.hsts);
    response
}

/// Why the request is refused, if it is.
fn refusal(rules: &Rules, request: &Request) -> Option<ApiError> {
    let sender = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(remote)| remote.ip());
    let now = Instant::now();
    if let (Some(strikes), Some(sender)) = (&rules.strikes, sender) {
        // checked first: an address that is waiting learns nothing more, not even from a right guess
        if strikes.is_waiting(sender, now) {
            return Some(ApiError::TooManyAttempts);
        }
    }
    if !rules.addressed_to_us(request) {
        return Some(ApiError::Forbidden("this engine answers only requests addressed to it"));
    }
    if !rules.sent_by_an_allowed_page(request.headers()) {
        return Some(ApiError::Forbidden("requests from other sites are refused"));
    }
    if !rules.carries_the_token(request.headers()) {
        if let (Some(strikes), Some(sender)) = (&rules.strikes, sender) {
            strikes.strike(sender, now);
        }
        return Some(ApiError::Unauthorized);
    }
    // refused at once when it says it is too large; one that does not say is cut off at the limit as it is read
    let length = request.headers().get(header::CONTENT_LENGTH).and_then(|value| value.to_str().ok());
    if length.is_some_and(|length| length.parse::<u64>().map_or(true, |length| length > LARGEST_BODY as u64)) {
        return Some(ApiError::TooLarge);
    }
    None
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
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CONTENT_SECURITY_POLICY, "default-src 'none'; frame-ancestors 'none'"),
    ];
    for (name, value) in set {
        headers.insert(name, HeaderValue::from_static(value));
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
        let strikes = Strikes::default();
        let now = Instant::now();
        let (guesser, neighbour): (IpAddr, IpAddr) =
            ("2001:db8:1:2:aaaa::1".parse().unwrap(), "2001:db8:1:2:bbbb::9".parse().unwrap());
        for _ in 0..MOST_STRIKES {
            strikes.strike(guesser, now);
        }
        assert!(strikes.is_waiting(guesser, now));
        assert!(!strikes.is_waiting(neighbour, now));
        // a new address for every guess: the network as a whole is then made to wait
        for number in 0..MOST_NETWORK_STRIKES {
            let rotating = IpAddr::from([0x2001, 0xdb8, 1, 2, 0xcccc, 0, 0, u16::try_from(number).unwrap()]);
            strikes.strike(rotating, now);
        }
        assert!(strikes.is_waiting(neighbour, now));
        assert!(!strikes.is_waiting("2001:db8:1:3::1".parse().unwrap(), now));
        // an IPv4 address written as IPv6 is the IPv4 address
        for _ in 0..MOST_STRIKES {
            strikes.strike("::ffff:192.0.2.1".parse().unwrap(), now);
        }
        assert!(strikes.is_waiting("192.0.2.1".parse().unwrap(), now));
    }

    #[test]
    fn a_sender_waits_after_too_many_wrong_tokens_until_its_window_ends() {
        let strikes = Strikes::default();
        let (sender, start) = ("192.0.2.1".parse().unwrap(), Instant::now());
        for _ in 0..MOST_STRIKES {
            assert!(!strikes.is_waiting(sender, start));
            strikes.strike(sender, start);
        }
        assert!(strikes.is_waiting(sender, start));
        assert!(!strikes.is_waiting("192.0.2.2".parse().unwrap(), start)); // others are not held up
        assert!(!strikes.is_waiting(sender, start + STRIKE_WINDOW));
    }

    #[test]
    fn the_remembered_senders_never_grow_beyond_their_limit() {
        let strikes = Strikes::default();
        let now = Instant::now();
        for number in 0..(MOST_REMEMBERED as u32 + 50) {
            strikes.strike(IpAddr::from(number.to_be_bytes()), now);
        }
        assert_eq!(strikes.0.lock().unwrap().len(), MOST_REMEMBERED);
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
