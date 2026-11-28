//! The owner's sessions: kept on the server, named to the browser by a random id in a cookie.
//!
//! A session id is 256 bits from the operating system's random source and means nothing by itself: the table
//! here is what makes it a session, so ending one is removing it (no signed cookie stays valid until it expires),
//! and a restart signs everyone out. The table is keyed by the id's SHA-256, so that looking an id up compares
//! digests, never the id itself, and a lookup's timing says nothing about the ids that exist.
//!
//! A session ends at the first of: [`IDLE`] without a request, [`LIFETIME`] after it started, sign-out,
//! sign-out everywhere, or a new sign-in from the same browser (the id is never reused: a sign-in always issues
//! a fresh one, so an id planted in a browser before sign-in is worth nothing after it). There are at most
//! [`MOST_SESSIONS`]: one owner needs a handful, and a new one beyond that pushes out the least recently used.
//!
//! Each session has its own anti-forgery token, sent back by the interface in [`CSRF_HEADER`] on every request
//! that changes something (see the reasons in [`super`]).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::api::guard::config::Cookie;

/// A session that sees no request for this long ends: a window left open over lunch stays signed in, one left
/// overnight does not.
pub const IDLE: Duration = Duration::from_secs(2 * 60 * 60);
/// A session ends this long after sign-in whatever happens: a working day, then the owner signs in again.
pub const LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
/// Sessions kept at once.
pub const MOST_SESSIONS: usize = 32;
/// The header the interface echoes the session's anti-forgery token in.
pub const CSRF_HEADER: &str = "x-constellate-csrf";

/// The session cookie's name. `__Host-` makes a browser accept it only with `Secure`, `Path=/` and no `Domain`:
/// it is then this host's alone, and no other host (a neighbouring subdomain) can set or overwrite it.
const SECURE_NAME: &str = "__Host-constellate";
/// Its name without the prefix, which a browser refuses without `Secure`: only under the HTTPS opt-out.
const PLAIN_NAME: &str = "constellate";
/// The cookie that ties a sign-in with the provider to the browser that started it.
const SECURE_FLOW_NAME: &str = "__Host-constellate-flow";
const PLAIN_FLOW_NAME: &str = "constellate-flow";

/// 32 bytes from the operating system's random source, in base64url (43 characters).
pub fn random_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

/// Two secrets compared in constant time, whatever their lengths.
pub fn same_secret(sent: &str, kept: &str) -> bool {
    bool::from(digest(sent).ct_eq(&digest(kept)))
}

struct Session {
    csrf: String,
    started: Instant,
    used: Instant,
}

impl Session {
    fn is_over(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.used) >= IDLE || now.saturating_duration_since(self.started) >= LIFETIME
    }
}

/// A session just started: what the browser is given.
#[derive(Clone)]
pub struct Issued {
    /// The cookie's value.
    pub id: String,
    pub csrf: String,
}

/// A session a request belongs to.
#[derive(Clone)]
pub struct Current {
    id: String,
    csrf: String,
}

impl Current {
    pub fn csrf(&self) -> &str {
        &self.csrf
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Does the request carry this session's anti-forgery token?
    pub fn vouched_for(&self, headers: &HeaderMap) -> bool {
        let mut sent = headers.get_all(CSRF_HEADER).iter();
        match (sent.next().and_then(|value| value.to_str().ok()), sent.next()) {
            (Some(token), None) => same_secret(token, &self.csrf),
            _ => false,
        }
    }
}

/// The table of sessions.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<[u8; 32], Session>>);

impl Sessions {
    /// Start a session, ending the ones the browser still carried (`replacing`): every sign-in gets a new id.
    pub fn start<'a>(
        &self,
        replacing: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> Result<Issued, getrandom::Error> {
        let issued = Issued { id: random_id()?, csrf: random_id()? };
        let mut table = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for old in replacing {
            table.remove(&digest(old));
        }
        table.retain(|_, session| !session.is_over(now));
        while table.len() >= MOST_SESSIONS {
            let oldest = table.iter().min_by_key(|(_, session)| session.used).map(|(key, _)| *key);
            match oldest {
                Some(key) => table.remove(&key),
                None => break,
            };
        }
        table.insert(digest(&issued.id), Session { csrf: issued.csrf.clone(), started: now, used: now });
        Ok(issued)
    }

    /// The session one of `ids` names, if it is still on; it counts as used now. One that is over is removed.
    pub fn find<'a>(&self, ids: impl IntoIterator<Item = &'a str>, now: Instant) -> Option<Current> {
        let mut table = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for id in ids {
            let key = digest(id);
            let Some(session) = table.get_mut(&key) else { continue };
            if session.is_over(now) {
                table.remove(&key);
                continue;
            }
            session.used = now;
            return Some(Current { id: id.to_owned(), csrf: session.csrf.clone() });
        }
        None
    }

    pub fn end(&self, id: &str) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&digest(id));
    }

    pub fn end_all(&self) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
    }
}

/// The values of every cookie called `name` the request carries (HTTP/2 may split them over several headers).
pub fn cookies<'a>(headers: &'a HeaderMap, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(move |pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key.trim() == name).then(|| value.trim().trim_matches('"'))
        })
}

impl Cookie {
    pub fn session_name(self) -> &'static str {
        match self {
            Cookie::Secure => SECURE_NAME,
            Cookie::Plain(_) => PLAIN_NAME,
        }
    }

    pub fn flow_name(self) -> &'static str {
        match self {
            Cookie::Secure => SECURE_FLOW_NAME,
            Cookie::Plain(_) => PLAIN_FLOW_NAME,
        }
    }

    fn attributes(self) -> &'static str {
        match self {
            Cookie::Secure => "HttpOnly; Secure; Path=/",
            Cookie::Plain(_) => "HttpOnly; Path=/",
        }
    }

    /// `Set-Cookie` for a session. `SameSite=Strict`: a browser never sends it with a request another site
    /// started, so no other site can make a request in the owner's name.
    pub fn session(self, id: &str) -> String {
        format!("{}={id}; {}; SameSite=Strict; Max-Age={}", self.session_name(), self.attributes(), LIFETIME.as_secs())
    }

    /// `Set-Cookie` that removes the session cookie.
    pub fn session_ended(self) -> String {
        format!("{}=; {}; SameSite=Strict; Max-Age=0", self.session_name(), self.attributes())
    }

    /// `Set-Cookie` for a sign-in with the provider. `SameSite=Lax`, not Strict: the provider sends the browser
    /// back with a top-level navigation from its own site, which carries Lax cookies and never Strict ones.
    pub fn flow(self, binding: &str, lasts: Duration) -> String {
        format!("{}={binding}; {}; SameSite=Lax; Max-Age={}", self.flow_name(), self.attributes(), lasts.as_secs())
    }

    pub fn flow_ended(self) -> String {
        format!("{}=; {}; SameSite=Lax; Max-Age=0", self.flow_name(), self.attributes())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn a_session_ends_after_its_idle_time_or_its_lifetime_whichever_comes_first() {
        let sessions = Sessions::default();
        let start = Instant::now();
        let issued = sessions.start([], start).unwrap();
        assert!(sessions.find([issued.id.as_str()], start + IDLE - Duration::from_secs(1)).is_some());
        // used just now: idle again from here
        assert!(sessions.find([issued.id.as_str()], start + IDLE + IDLE / 2).is_some());
        assert!(sessions.find([issued.id.as_str()], start + IDLE * 3).is_none(), "idle for longer than IDLE");
        assert_eq!(sessions.count(), 0); // and forgotten

        let busy = sessions.start([], start).unwrap();
        let mut now = start;
        while now < start + LIFETIME - Duration::from_secs(60) {
            now += IDLE / 2;
            if now < start + LIFETIME {
                assert!(sessions.find([busy.id.as_str()], now).is_some());
            }
        }
        assert!(sessions.find([busy.id.as_str()], start + LIFETIME).is_none(), "kept busy past its lifetime");
    }

    #[test]
    fn every_sign_in_issues_a_new_id_and_ends_the_one_the_browser_carried() {
        let sessions = Sessions::default();
        let now = Instant::now();
        let first = sessions.start([], now).unwrap();
        let second = sessions.start([first.id.as_str()], now).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.csrf, second.csrf);
        assert!(sessions.find([first.id.as_str()], now).is_none());
        assert!(sessions.find([second.id.as_str()], now).is_some());
        // an id nobody issued (planted before sign-in) is not adopted
        let planted = random_id().unwrap();
        let third = sessions.start([planted.as_str()], now).unwrap();
        assert_ne!(third.id, planted);
        assert!(sessions.find([planted.as_str()], now).is_none());
    }

    #[test]
    fn signing_out_ends_one_session_and_signing_out_everywhere_ends_all() {
        let sessions = Sessions::default();
        let now = Instant::now();
        let (one, two) = (sessions.start([], now).unwrap(), sessions.start([], now).unwrap());
        sessions.end(&one.id);
        assert!(sessions.find([one.id.as_str()], now).is_none());
        assert!(sessions.find([two.id.as_str()], now).is_some());
        sessions.end_all();
        assert!(sessions.find([two.id.as_str()], now).is_none());
    }

    #[test]
    fn the_table_never_grows_beyond_its_limit_and_lets_the_least_recently_used_go() {
        let sessions = Sessions::default();
        let start = Instant::now();
        let first = sessions.start([], start).unwrap();
        let mut last = first.clone();
        for step in 1..=MOST_SESSIONS as u32 {
            last = sessions.start([], start + Duration::from_secs(u64::from(step))).unwrap();
        }
        assert_eq!(sessions.count(), MOST_SESSIONS);
        let later = start + Duration::from_secs(1000);
        assert!(sessions.find([first.id.as_str()], later).is_none());
        assert!(sessions.find([last.id.as_str()], later).is_some());
    }

    #[test]
    fn the_anti_forgery_token_must_be_sent_once_and_exactly() {
        let sessions = Sessions::default();
        let now = Instant::now();
        let issued = sessions.start([], now).unwrap();
        let current = sessions.find([issued.id.as_str()], now).unwrap();
        let mut headers = HeaderMap::new();
        assert!(!current.vouched_for(&headers));
        headers.insert(CSRF_HEADER, HeaderValue::from_str(&issued.csrf).unwrap());
        assert!(current.vouched_for(&headers));
        headers.append(CSRF_HEADER, HeaderValue::from_str(&issued.csrf).unwrap());
        assert!(!current.vouched_for(&headers), "sent twice");
        let mut wrong = HeaderMap::new();
        wrong.insert(CSRF_HEADER, HeaderValue::from_str(&issued.id).unwrap());
        assert!(!current.vouched_for(&wrong));
    }

    #[test]
    fn cookies_are_read_from_every_cookie_header_and_written_with_their_attributes() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, HeaderValue::from_static("a=1; __Host-constellate=one"));
        headers.append(header::COOKIE, HeaderValue::from_static("__Host-constellate=\"two\""));
        assert_eq!(cookies(&headers, "__Host-constellate").collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(cookies(&headers, "constellate").count(), 0);

        let set = Cookie::Secure.session("abc");
        for part in ["__Host-constellate=abc", "HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age=43200"] {
            assert!(set.split("; ").any(|attribute| attribute == part), "{part} missing from {set}");
        }
        assert!(!set.contains("Domain"));
        assert!(Cookie::Secure.session_ended().contains("Max-Age=0"));
        let flow = Cookie::Secure.flow("xyz", Duration::from_secs(600));
        assert!(flow.starts_with("__Host-constellate-flow=xyz;") && flow.contains("SameSite=Lax"));
        assert!(flow.contains("Secure"));
    }

    #[test]
    fn a_random_id_is_43_characters_of_base64url_that_decode_to_32_bytes() {
        let ids: Vec<String> = (0..20).map(|_| random_id().unwrap()).collect();
        for id in &ids {
            assert_eq!(id.len(), 43, "{id}");
            assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{id}");
            assert_eq!(URL_SAFE_NO_PAD.decode(id).unwrap().len(), 32, "{id}");
        }
        let distinct: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), 20, "every id is new");
    }

    #[test]
    fn the_digest_is_sha256_and_secrets_are_the_same_only_when_equal_whatever_their_lengths() {
        // FIPS 180-2's test vector, and the digest of nothing
        let hex = |bytes: [u8; 32]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        assert_eq!(hex(digest("abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(digest("")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert!(same_secret("a-secret", "a-secret"));
        assert!(same_secret("", ""));
        for (sent, kept) in [("a-secret", "a-secreT"), ("a-secret", "a-secret "), ("", "a-secret"), ("a", "")] {
            assert!(!same_secret(sent, kept), "{sent:?} against {kept:?}");
        }
    }

    #[test]
    fn under_the_https_opt_out_cookies_have_no_prefix_and_no_secure_attribute() {
        use crate::api::guard::config::{Access, Environment, Listen};
        let opted_out = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_BIND", "192.168.1.20"),
            ("CONSTELLATE_USE_HTTPS", "false"),
            ("CONSTELLATE_ALLOW_PASSWORD", "true"),
            ("CONSTELLATE_ORIGIN", "http://papers.test"),
        ]))
        .unwrap();
        let Access::SignIn { sign_in, .. } = opted_out.access() else { panic!("network mode") };
        let plain = sign_in.cookie();
        assert!(matches!(plain, Cookie::Plain(_)));
        assert_eq!((plain.session_name(), plain.flow_name()), ("constellate", "constellate-flow"));
        assert_eq!(plain.session("abc"), "constellate=abc; HttpOnly; Path=/; SameSite=Strict; Max-Age=43200");
        assert_eq!(plain.session_ended(), "constellate=; HttpOnly; Path=/; SameSite=Strict; Max-Age=0");
        assert_eq!(
            plain.flow("xyz", Duration::from_secs(600)),
            "constellate-flow=xyz; HttpOnly; Path=/; SameSite=Lax; Max-Age=600"
        );
        assert_eq!(plain.flow_ended(), "constellate-flow=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0");
        assert_eq!(
            Cookie::Secure.session("abc"),
            "__Host-constellate=abc; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=43200"
        );
        assert_eq!(
            Cookie::Secure.flow_ended(),
            "__Host-constellate-flow=; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age=0"
        );
    }
}
