//! Sign-in with an OpenID Connect provider (Google by default), written against the standard so that another
//! provider is a different issuer and nothing else.
//!
//! The authorization code flow with PKCE (OpenID Connect Core 1.0 section 3.1, RFC 7636):
//!
//! 1. [`Oidc::begin`]: the provider's endpoints come from its discovery document
//!    (`<issuer>/.well-known/openid-configuration`, whose `issuer` must be the configured one exactly). A random
//!    `state`, `nonce` and PKCE verifier are made and kept here, together with the digest of a random value put
//!    in a short-lived cookie: the flow can be finished only by the browser that started it, once, within
//!    [`FLOW_LASTS`]. The browser is sent to the provider with the `S256` challenge of the verifier.
//! 2. [`Oidc::finish`]: the provider sends the browser back with `code` and `state`. The flow `state` names is
//!    taken out of the table (so a replayed `state` or `code` finds nothing), the cookie is checked, and the code
//!    is exchanged at the token endpoint with the verifier and the client's secret, over HTTPS.
//! 3. The ID token is checked ([`Oidc::validate`]): its signature against the provider's published keys (JWKS,
//!    fetched again when a token names a key not yet known, so the provider's key rotation just works, but at
//!    most once a [`KEYS_REFETCH`]), the algorithm (only [`ALGORITHMS`], never `none` or a shared secret), `iss`,
//!    `aud` (and `azp` when there are several audiences), `exp` and `iat` within [`LEEWAY`] of this clock and
//!    `iat` no older than [`FRESH`], the `nonce` of this flow, and `email_verified` true.
//! 4. The one rule of this server: the token's email is the owner's ([`Owner`]).
//!
//! Every network request goes through [`Provider`], so that the tests play the provider without a network.
//! Why a sign-in was refused is told to the engine's standard error, never to the browser, and never with the
//! email address or a secret in it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, KeyAlgorithm, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use super::sessions::{digest, random_id, same_secret};
use super::{Client, Owner};

/// How long a started sign-in may take to come back.
pub const FLOW_LASTS: Duration = Duration::from_secs(10 * 60);
/// Sign-ins started and not yet finished, at most: beyond it the oldest is forgotten. A flow is a few hundred
/// bytes; this many in ten minutes is more than one address (or one IPv6 /64) may start, so pushing the owner's
/// own flow out takes many addresses.
pub const MOST_FLOWS: usize = 1024;
/// How far the provider's clock and this one may disagree.
pub const LEEWAY: Duration = Duration::from_secs(60);
/// An ID token issued longer ago than this is not the answer to a sign-in just made.
pub const FRESH: Duration = Duration::from_secs(10 * 60);
/// How long the discovery document and the keys are kept before they are fetched again.
pub const KEPT: Duration = Duration::from_secs(60 * 60);
/// The keys are fetched again for an unknown key id at most this often, so that tokens naming made-up keys
/// cannot make the engine hammer the provider.
pub const KEYS_REFETCH: Duration = Duration::from_secs(60);
/// The signature algorithms accepted: what Google signs with, and the elliptic-curve one other providers use.
pub const ALGORITHMS: [Algorithm; 2] = [Algorithm::RS256, Algorithm::ES256];
/// The largest answer read from the provider.
pub const LARGEST_ANSWER: usize = 1024 * 1024;

/// An answer from the provider.
#[derive(Clone, Debug)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The provider could not be reached; the reason is for the engine's log (it never holds a secret).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unreachable(pub String);

/// How the engine talks to the provider: blocking, over HTTPS only, no redirects followed.
pub trait Provider: Send + Sync {
    fn get(&self, url: &Url) -> Result<Answer, Unreachable>;
    fn post_form(&self, url: &Url, form: &[(&str, &str)]) -> Result<Answer, Unreachable>;
}

/// [`Provider`] over the network, through `ureq` and rustls, as the rest of the engine fetches.
pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::Rustls).build();
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(15)))
            .tls_config(tls)
            .build()
            .into();
        Http { agent }
    }
}

impl Http {
    fn read(url: &Url, response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Answer, Unreachable> {
        let mut response =
            response.map_err(|error| Unreachable(format!("{}: {error}", url.origin().ascii_serialization())))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(LARGEST_ANSWER as u64)
            .read_to_vec()
            .map_err(|error| Unreachable(format!("{}: {error}", url.origin().ascii_serialization())))?;
        Ok(Answer { status, body })
    }
}

impl Provider for Http {
    fn get(&self, url: &Url) -> Result<Answer, Unreachable> {
        Http::read(url, self.agent.get(url.as_str()).header("Accept", "application/json").call())
    }

    fn post_form(&self, url: &Url, form: &[(&str, &str)]) -> Result<Answer, Unreachable> {
        let sent = self.agent.post(url.as_str()).header("Accept", "application/json").send_form(form.iter().copied());
        Http::read(url, sent)
    }
}

/// Why a sign-in with the provider was refused. For the engine's log: never holds a secret or an email address.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OidcError {
    #[error("the provider could not be reached: {0}")]
    Unreachable(#[from] Unreachable),
    #[error("the provider's discovery document is not usable: {0}")]
    Discovery(&'static str),
    #[error("the provider's keys (JWKS) are not usable")]
    Keys,
    #[error("the provider said no: {0}")]
    ProviderRefused(String),
    #[error("the sign-in came back with a state that was never issued, or was used already")]
    UnknownState,
    #[error("the sign-in took longer than {} minutes to come back", FLOW_LASTS.as_secs() / 60)]
    FlowExpired,
    #[error("the sign-in came back to another browser than the one that started it")]
    OtherBrowser,
    #[error("the token endpoint refused the code (status {0})")]
    CodeRefused(u16),
    #[error("the token endpoint's answer has no ID token")]
    NoIdToken,
    #[error("the ID token is malformed")]
    Malformed,
    #[error("the ID token is signed with an algorithm that is not accepted")]
    Algorithm,
    #[error("the ID token names a key the provider does not publish")]
    UnknownKey,
    #[error("the ID token's signature is wrong")]
    Signature,
    #[error("the ID token is from another issuer")]
    Issuer,
    #[error("the ID token is meant for another client")]
    Audience,
    #[error("the ID token has expired")]
    Expired,
    #[error("the ID token was issued in the future or too long ago")]
    IssuedAt,
    #[error("the ID token's nonce is not this sign-in's")]
    Nonce,
    #[error("the provider has not verified the account's email address")]
    EmailNotVerified,
    #[error("an account that is not the owner's tried to sign in")]
    NotTheOwner,
    #[error("the operating system's random source could not be read")]
    NoRandomness,
}

/// What the provider's discovery document says that is needed here.
#[derive(Clone, Debug)]
struct Endpoints {
    authorization: Url,
    token: Url,
    keys: Url,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    code_challenge_methods_supported: Option<Vec<String>>,
}

/// An address the provider gave, which must be https.
fn https(text: &str, what: &'static str) -> Result<Url, OidcError> {
    let url = Url::parse(text).map_err(|_| OidcError::Discovery(what))?;
    if url.scheme() != "https" {
        return Err(OidcError::Discovery(what));
    }
    Ok(url)
}

/// The issuer as configured and as it appears in tokens: the same text, without a trailing slash.
fn bare(issuer: &str) -> &str {
    issuer.strip_suffix('/').unwrap_or(issuer)
}

/// A started sign-in, kept until it comes back.
struct Flow {
    nonce: String,
    verifier: String,
    /// The digest of the cookie value that ties it to the browser.
    binding: [u8; 32],
    started: Instant,
}

/// What [`Oidc::begin`] hands to the browser.
pub struct Begun {
    /// Where to send the browser: the provider's authorization endpoint with this flow's parameters.
    pub location: String,
    /// The value of the cookie that ties the flow to this browser.
    pub binding: String,
}

/// What the provider's redirect brought back (the query of `/auth/callback`).
#[derive(Deserialize)]
pub struct Callback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Something fetched from the provider, and when.
struct Fetched<T> {
    value: T,
    at: Instant,
}

/// The published keys, by key id, with when they were last asked for.
#[derive(Default)]
struct Keys {
    known: Vec<Jwk>,
    fetched: Option<Instant>,
}

/// The sign-in with one provider, for one client.
pub struct Oidc {
    client: Client,
    redirect: String,
    provider: Arc<dyn Provider>,
    endpoints: Mutex<Option<Fetched<Endpoints>>>,
    keys: Mutex<Keys>,
    flows: Mutex<HashMap<[u8; 32], Flow>>,
}

impl fmt::Debug for Oidc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Oidc").field("issuer", &self.client.issuer).finish_non_exhaustive()
    }
}

/// The claims of an ID token that are checked here; the registered ones are checked by `jsonwebtoken`.
#[derive(Deserialize)]
struct Claims {
    iat: u64,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<Value>,
    #[serde(default)]
    aud: Value,
    #[serde(default)]
    azp: Option<String>,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// The PKCE challenge of a verifier: base64url(SHA-256(verifier)), RFC 7636 section 4.2.
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

impl Oidc {
    pub fn new(client: Client, redirect: String, provider: Arc<dyn Provider>) -> Oidc {
        Oidc {
            client,
            redirect,
            provider,
            endpoints: Mutex::new(None),
            keys: Mutex::new(Keys::default()),
            flows: Mutex::new(HashMap::new()),
        }
    }

    /// The provider's name for the sign-in button: "Google" for Google, the issuer's host otherwise.
    pub fn name(&self) -> String {
        if bare(&self.client.issuer) == bare(super::GOOGLE) {
            return "Google".to_owned();
        }
        Url::parse(&self.client.issuer).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default()
    }

    fn endpoints(&self, now: Instant) -> Result<Endpoints, OidcError> {
        let kept = match &*self.endpoints.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
            Some(kept) if now.saturating_duration_since(kept.at) < KEPT => Some(kept.value.clone()),
            _ => None,
        };
        if let Some(endpoints) = kept {
            return Ok(endpoints);
        }
        let issuer = bare(&self.client.issuer);
        let address = https(&format!("{issuer}/.well-known/openid-configuration"), "the issuer is not https")?;
        let answer = self.provider.get(&address)?;
        if answer.status != 200 {
            return Err(OidcError::Discovery("it was not found"));
        }
        let found: Discovery = serde_json::from_slice(&answer.body).map_err(|_| OidcError::Discovery("not JSON"))?;
        // OpenID Connect Discovery 1.0 section 4.3: a document that names another issuer is not this provider's
        if bare(&found.issuer) != issuer {
            return Err(OidcError::Discovery("it names another issuer"));
        }
        if found.code_challenge_methods_supported.is_some_and(|methods| !methods.iter().any(|one| one == "S256")) {
            return Err(OidcError::Discovery("it does not offer PKCE with S256"));
        }
        let endpoints = Endpoints {
            authorization: https(&found.authorization_endpoint, "the authorization endpoint is not https")?,
            token: https(&found.token_endpoint, "the token endpoint is not https")?,
            keys: https(&found.jwks_uri, "the keys' address is not https")?,
        };
        *self.endpoints.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Fetched { value: endpoints.clone(), at: now });
        Ok(endpoints)
    }

    /// Start a sign-in: where to send the browser, and the cookie that ties the flow to it.
    pub fn begin(&self, now: Instant) -> Result<Begun, OidcError> {
        let endpoints = self.endpoints(now)?;
        let fresh = || random_id().map_err(|_| OidcError::NoRandomness);
        let (state, nonce, verifier, binding) = (fresh()?, fresh()?, fresh()?, fresh()?);
        let mut location = endpoints.authorization;
        location
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client.id)
            .append_pair("redirect_uri", &self.redirect)
            .append_pair("scope", "openid email")
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        let mut flows = self.flows.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        flows.retain(|_, flow| now.saturating_duration_since(flow.started) < FLOW_LASTS);
        while flows.len() >= MOST_FLOWS {
            let oldest = flows.iter().min_by_key(|(_, flow)| flow.started).map(|(key, _)| *key);
            match oldest {
                Some(key) => flows.remove(&key),
                None => break,
            };
        }
        flows.insert(digest(&state), Flow { nonce, verifier, binding: digest(&binding), started: now });
        Ok(Begun { location: location.to_string(), binding })
    }

    /// Finish a sign-in: the flow `state` names (taken, so it cannot be used again), the code exchanged, the ID
    /// token checked. `bindings` are the flow cookies the browser sent. Answers the owner's email address.
    pub fn finish<'a>(
        &self,
        callback: &Callback,
        bindings: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> Result<String, OidcError> {
        let Some(state) = callback.state.as_deref() else { return Err(OidcError::UnknownState) };
        let flow = self.flows.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&digest(state));
        let Some(flow) = flow else { return Err(OidcError::UnknownState) };
        if now.saturating_duration_since(flow.started) >= FLOW_LASTS {
            return Err(OidcError::FlowExpired);
        }
        if !bindings.into_iter().any(|binding| bool::from(digest(binding).ct_eq(&flow.binding))) {
            return Err(OidcError::OtherBrowser);
        }
        if let Some(error) = &callback.error {
            // the provider's error code is a short registered word; anything else is not repeated
            let word = if error.len() <= 40 && error.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                error.clone()
            } else {
                "an error".to_owned()
            };
            return Err(OidcError::ProviderRefused(word));
        }
        let Some(code) = callback.code.as_deref() else { return Err(OidcError::ProviderRefused("no code".to_owned())) };
        let endpoints = self.endpoints(now)?;
        let answer = self.provider.post_form(
            &endpoints.token,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect),
                ("client_id", &self.client.id),
                ("client_secret", self.client.secret.expose()),
                ("code_verifier", &flow.verifier),
            ],
        )?;
        if answer.status != 200 {
            return Err(OidcError::CodeRefused(answer.status));
        }
        #[derive(Deserialize)]
        struct Tokens {
            id_token: Option<String>,
        }
        let tokens: Tokens = serde_json::from_slice(&answer.body).map_err(|_| OidcError::NoIdToken)?;
        let id_token = tokens.id_token.ok_or(OidcError::NoIdToken)?;
        self.validate(&id_token, &flow.nonce, now)
    }

    /// The key a token's header names, fetching the provider's keys when it is not known (yet).
    fn key_for(&self, kid: Option<&str>, algorithm: Algorithm, now: Instant) -> Result<Jwk, OidcError> {
        let endpoints = self.endpoints(now)?;
        let mut keys = self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let pick = |known: &[Jwk]| -> Option<Jwk> {
            match kid {
                Some(kid) => known.iter().find(|key| key.common.key_id.as_deref() == Some(kid)).cloned(),
                // without a key id, only a provider that publishes one usable key can be understood
                None => {
                    let usable: Vec<&Jwk> = known.iter().filter(|key| fits(key, algorithm)).collect();
                    (usable.len() == 1).then(|| usable[0].clone())
                }
            }
        };
        let stale = keys.fetched.is_none_or(|at| now.saturating_duration_since(at) >= KEPT);
        if let (false, Some(key)) = (stale, pick(&keys.known)) {
            return Ok(key);
        }
        let may_fetch = keys.fetched.is_none_or(|at| now.saturating_duration_since(at) >= KEYS_REFETCH);
        if may_fetch {
            let fetched =
                self.provider.get(&endpoints.keys).map_err(OidcError::from).and_then(|answer| {
                    if answer.status == 200 { parse_keys(&answer.body) } else { Err(OidcError::Keys) }
                });
            match fetched {
                Ok(known) => {
                    keys.known = known;
                    keys.fetched = Some(now);
                }
                // the provider's keys could not be had now: the ones it published before still check its tokens
                Err(error) => return pick(&keys.known).ok_or(error),
            }
        }
        pick(&keys.known).ok_or(OidcError::UnknownKey)
    }

    /// Check an ID token as OpenID Connect Core 1.0 section 3.1.3.7 says, and answer its email address if it is
    /// the owner's.
    pub fn validate(&self, id_token: &str, nonce: &str, now: Instant) -> Result<String, OidcError> {
        let header = jsonwebtoken::decode_header(id_token).map_err(|_| OidcError::Malformed)?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(OidcError::Algorithm);
        }
        // extensions this engine does not understand must not be ignored (RFC 7515 section 4.1.11); keys named
        // inside the token itself are never used
        if header.crit.is_some() || header.jwk.is_some() || header.jku.is_some() || header.x5u.is_some() {
            return Err(OidcError::Malformed);
        }
        let jwk = self.key_for(header.kid.as_deref(), header.alg, now)?;
        if !fits(&jwk, header.alg) {
            return Err(OidcError::Algorithm);
        }
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| OidcError::Keys)?;

        let issuer = bare(&self.client.issuer).to_owned();
        let mut issuers = vec![issuer.clone()];
        // Google's own documentation: its ID tokens may say its issuer without the scheme
        if issuer == super::GOOGLE {
            issuers.push("accounts.google.com".to_owned());
        }
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&issuers);
        validation.set_audience(&[&self.client.id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.leeway = LEEWAY.as_secs();
        validation.validate_nbf = true; // a token that says it is not valid yet is not, when it says so
        let claims = jsonwebtoken::decode::<Claims>(id_token, &key, &validation).map_err(|error| {
            use jsonwebtoken::errors::ErrorKind;
            match error.kind() {
                ErrorKind::InvalidSignature => OidcError::Signature,
                ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => OidcError::Algorithm,
                ErrorKind::ExpiredSignature => OidcError::Expired,
                ErrorKind::ImmatureSignature => OidcError::IssuedAt,
                ErrorKind::InvalidIssuer => OidcError::Issuer,
                ErrorKind::InvalidAudience => OidcError::Audience,
                ErrorKind::MissingRequiredClaim(claim) if claim == "iss" => OidcError::Issuer,
                ErrorKind::MissingRequiredClaim(claim) if claim == "aud" => OidcError::Audience,
                ErrorKind::MissingRequiredClaim(_) => OidcError::Expired,
                _ => OidcError::Malformed,
            }
        })?;
        let claims = claims.claims;

        let unix = now_unix();
        if claims.iat > unix.saturating_add(LEEWAY.as_secs()) || claims.iat.saturating_add(FRESH.as_secs()) < unix {
            return Err(OidcError::IssuedAt);
        }
        // several audiences: the token must say it was issued to this client (azp); one: azp, if any, is it too
        let several = claims.aud.as_array().is_some_and(|audiences| audiences.len() > 1);
        match claims.azp.as_deref() {
            Some(azp) if azp != self.client.id => return Err(OidcError::Audience),
            None if several => return Err(OidcError::Audience),
            _ => {}
        }
        if !claims.nonce.as_deref().is_some_and(|sent| same_secret(sent, nonce)) {
            return Err(OidcError::Nonce);
        }
        let verified = matches!(&claims.email_verified, Some(Value::Bool(true)))
            || matches!(&claims.email_verified, Some(Value::String(text)) if text == "true");
        if !verified {
            return Err(OidcError::EmailNotVerified);
        }
        let email = claims.email.ok_or(OidcError::NotTheOwner)?;
        // Google: "Gmail addresses are not case sensitive", and no two of its accounts differ only in case
        if !self.client.owner.is(&email, issuer == super::GOOGLE) {
            return Err(OidcError::NotTheOwner);
        }
        Ok(email)
    }
}

/// Can `jwk` check a signature made with `algorithm`? Its type must be the algorithm's family, its `alg` (when
/// it says one) the algorithm itself, its use (when it says one) signatures.
fn fits(jwk: &Jwk, algorithm: Algorithm) -> bool {
    let family = match (&jwk.algorithm, algorithm) {
        (AlgorithmParameters::RSA(_), Algorithm::RS256) => true,
        (AlgorithmParameters::EllipticCurve(curve), Algorithm::ES256) => {
            curve.curve == jsonwebtoken::jwk::EllipticCurve::P256
        }
        _ => false,
    };
    let named = match jwk.common.key_algorithm {
        None => true,
        Some(KeyAlgorithm::RS256) => algorithm == Algorithm::RS256,
        Some(KeyAlgorithm::ES256) => algorithm == Algorithm::ES256,
        Some(_) => false,
    };
    let for_signatures = matches!(jwk.common.public_key_use, None | Some(PublicKeyUse::Signature));
    family && named && for_signatures
}

/// The keys of a JWKS document. A key of a kind this engine cannot read is skipped, not a reason to read none.
fn parse_keys(body: &[u8]) -> Result<Vec<Jwk>, OidcError> {
    #[derive(Deserialize)]
    struct Set {
        keys: Vec<Value>,
    }
    let set: Set = serde_json::from_slice(body).map_err(|_| OidcError::Keys)?;
    Ok(set.keys.into_iter().filter_map(|key| serde_json::from_value::<Jwk>(key).ok()).collect())
}

impl Owner {
    /// Is `claimed` (the token's `email`) the owner's address? See [`Owner`] for how they are compared;
    /// `any_case` compares the part before the `@` without regard to case too, as Google does for its accounts.
    pub fn is(&self, claimed: &str, any_case: bool) -> bool {
        let local = |text: &str| if any_case { text.to_lowercase() } else { text.to_owned() };
        Owner::parse(claimed).is_some_and(|claimed| {
            same_secret(&local(&claimed.local), &local(&self.local)) && same_secret(&claimed.domain, &self.domain)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    //! The provider played by the test: discovery, keys and token endpoint, with keys made at test time.

    use super::*;
    use jsonwebtoken::{EncodingKey, Header};
    use serde_json::json;
    use std::sync::OnceLock;

    pub const ISSUER: &str = "https://provider.test";
    pub const CLIENT: &str = "client-id.test";
    pub const OWNER: &str = "Owner.Name@Example.ORG";
    pub const REDIRECT: &str = "https://papers.test/auth/callback";

    /// A signing key of the provider, with its id.
    pub struct Signer {
        pub kid: String,
        pub algorithm: Algorithm,
        pub encoding: EncodingKey,
        pub public: Jwk,
    }

    impl Signer {
        pub fn sign(&self, claims: &Value) -> String {
            let mut header = Header::new(self.algorithm);
            header.kid = Some(self.kid.clone());
            jsonwebtoken::encode(&header, claims, &self.encoding).unwrap()
        }
    }

    fn rsa_der() -> Vec<u8> {
        use rsa::pkcs1::EncodeRsaPrivateKey;
        let key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        key.to_pkcs1_der().unwrap().as_bytes().to_vec()
    }

    /// RSA keys take a while to make: made once for all tests.
    fn rsa_keys() -> &'static [Vec<u8>; 2] {
        static KEYS: OnceLock<[Vec<u8>; 2]> = OnceLock::new();
        KEYS.get_or_init(|| [rsa_der(), rsa_der()])
    }

    pub fn rsa_signer(which: usize, kid: &str) -> Signer {
        let encoding = EncodingKey::from_rsa_der(&rsa_keys()[which]);
        let mut public = Jwk::from_encoding_key(&encoding, Algorithm::RS256).unwrap();
        public.common.key_id = Some(kid.to_owned());
        Signer { kid: kid.to_owned(), algorithm: Algorithm::RS256, encoding, public }
    }

    pub fn ec_signer(kid: &str) -> Signer {
        let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let encoding = EncodingKey::from_ec_der(&pair.serialize_der());
        let mut public = Jwk::from_encoding_key(&encoding, Algorithm::ES256).unwrap();
        public.common.key_id = Some(kid.to_owned());
        Signer { kid: kid.to_owned(), algorithm: Algorithm::ES256, encoding, public }
    }

    /// What the fake provider answers, changeable while a test runs.
    pub struct Script {
        pub discovery: Value,
        pub keys: Vec<Jwk>,
        /// The ID token the token endpoint answers with for the next code.
        pub id_token: Option<String>,
        pub token_status: u16,
        /// Every request made: method and address.
        pub asked: Vec<String>,
        /// The last form posted to the token endpoint.
        pub posted: Vec<(String, String)>,
    }

    #[derive(Clone)]
    pub struct Fake(pub Arc<Mutex<Script>>);

    impl Fake {
        pub fn new(signers: &[&Signer]) -> Fake {
            Fake(Arc::new(Mutex::new(Script {
                discovery: json!({
                    "issuer": ISSUER,
                    "authorization_endpoint": format!("{ISSUER}/authorize"),
                    "token_endpoint": format!("{ISSUER}/token"),
                    "jwks_uri": format!("{ISSUER}/keys"),
                    "code_challenge_methods_supported": ["plain", "S256"],
                }),
                keys: signers.iter().map(|signer| signer.public.clone()).collect(),
                id_token: None,
                token_status: 200,
                asked: Vec::new(),
                posted: Vec::new(),
            })))
        }

        pub fn script(&self) -> std::sync::MutexGuard<'_, Script> {
            self.0.lock().unwrap()
        }

        pub fn count(&self, what: &str) -> usize {
            self.script().asked.iter().filter(|asked| asked.ends_with(what)).count()
        }
    }

    impl Provider for Fake {
        fn get(&self, url: &Url) -> Result<Answer, Unreachable> {
            let mut script = self.script();
            script.asked.push(format!("GET {url}"));
            let body = match url.as_str() {
                "https://provider.test/.well-known/openid-configuration" => script.discovery.to_string(),
                "https://provider.test/keys" => json!({ "keys": script.keys }).to_string(),
                _ => return Ok(Answer { status: 404, body: Vec::new() }),
            };
            Ok(Answer { status: 200, body: body.into_bytes() })
        }

        fn post_form(&self, url: &Url, form: &[(&str, &str)]) -> Result<Answer, Unreachable> {
            let mut script = self.script();
            script.asked.push(format!("POST {url}"));
            script.posted = form.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect();
            if url.as_str() != "https://provider.test/token" {
                return Ok(Answer { status: 404, body: Vec::new() });
            }
            let body = json!({ "access_token": "unused", "token_type": "Bearer", "id_token": script.id_token });
            Ok(Answer { status: script.token_status, body: body.to_string().into_bytes() })
        }
    }

    pub fn client() -> Client {
        Client {
            issuer: ISSUER.to_owned(),
            id: CLIENT.to_owned(),
            secret: super::super::Secret("client-secret.test".to_owned()),
            owner: Owner::parse(OWNER).unwrap(),
        }
    }

    /// The claims of a token the provider would issue for the owner, for `nonce`.
    pub fn claims(nonce: &str) -> Value {
        let now = now_unix();
        json!({
            "iss": ISSUER, "aud": CLIENT, "azp": CLIENT, "sub": "1234567890",
            "email": "Owner.Name@example.org", "email_verified": true,
            "iat": now, "exp": now + 3600, "nonce": nonce,
        })
    }

    fn oidc(fake: &Fake) -> Oidc {
        Oidc::new(client(), REDIRECT.to_owned(), Arc::new(fake.clone()))
    }

    /// Start a flow; answers its state, nonce, verifier challenge and cookie value.
    fn started(oidc: &Oidc, now: Instant) -> (String, String, String, String) {
        let begun = oidc.begin(now).unwrap();
        let location = Url::parse(&begun.location).unwrap();
        let get = |name: &str| location.query_pairs().find(|(key, _)| key == name).unwrap().1.into_owned();
        (get("state"), get("nonce"), get("code_challenge"), begun.binding)
    }

    fn back(state: &str) -> Callback {
        Callback { code: Some("the-code".to_owned()), state: Some(state.to_owned()), error: None }
    }

    #[test]
    fn the_sign_in_goes_to_the_discovered_endpoint_with_pkce_state_and_nonce() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let oidc = oidc(&fake);
        let now = Instant::now();
        let begun = oidc.begin(now).unwrap();
        let location = Url::parse(&begun.location).unwrap();
        assert_eq!(location.origin().ascii_serialization(), ISSUER);
        assert_eq!(location.path(), "/authorize");
        let query: HashMap<String, String> = location.query_pairs().into_owned().collect();
        for (key, value) in [
            ("response_type", "code"),
            ("client_id", CLIENT),
            ("redirect_uri", REDIRECT),
            ("scope", "openid email"),
            ("code_challenge_method", "S256"),
        ] {
            assert_eq!(query[key], value, "{key}");
        }
        assert!(query["state"].len() >= 43 && query["nonce"].len() >= 43);
        assert!(!query.contains_key("login_hint"), "the owner's address must not be shown to whoever starts it");
        assert!(!begun.location.contains("client-secret"));
        // the next one has its own values
        let (state, nonce, _, _) = started(&oidc, now);
        assert_ne!((state.as_str(), nonce.as_str()), (query["state"].as_str(), query["nonce"].as_str()));
    }

    #[test]
    fn the_owner_signs_in_and_the_code_is_exchanged_with_the_verifier_of_the_challenge() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let oidc = oidc(&fake);
        let now = Instant::now();
        let (state, nonce, challenged, binding) = started(&oidc, now);
        fake.script().id_token = Some(signer.sign(&claims(&nonce)));
        let email = oidc.finish(&back(&state), [binding.as_str()], now).unwrap();
        assert_eq!(email, "Owner.Name@example.org");

        let posted: HashMap<String, String> = fake.script().posted.iter().cloned().collect();
        assert_eq!(posted["grant_type"], "authorization_code");
        assert_eq!(posted["code"], "the-code");
        assert_eq!(posted["redirect_uri"], REDIRECT);
        assert_eq!(posted["client_secret"], "client-secret.test");
        assert_eq!(challenge(&posted["code_verifier"]), challenged, "the verifier must match the challenge sent");
        assert!(posted["code_verifier"].len() >= 43);
        // base64url of SHA-256 without padding, as computed by another program
        assert_eq!(
            challenge("the-verifier-of-a-sign-in-made-by-this-test"),
            "T2jSTw4E620o1IBly0eWg2P0OvZkUeexTdmazMWhv_0"
        );
    }

    #[test]
    fn a_state_or_code_cannot_be_used_twice_nor_by_another_browser_nor_late() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let oidc = oidc(&fake);
        let now = Instant::now();
        let (state, nonce, _, binding) = started(&oidc, now);
        fake.script().id_token = Some(signer.sign(&claims(&nonce)));
        assert!(oidc.finish(&back(&state), [binding.as_str()], now).is_ok());
        assert_eq!(oidc.finish(&back(&state), [binding.as_str()], now), Err(OidcError::UnknownState));
        assert_eq!(oidc.finish(&back("made-up"), [binding.as_str()], now), Err(OidcError::UnknownState));

        let (state, _, _, _) = started(&oidc, now);
        assert_eq!(oidc.finish(&back(&state), ["another-browser"], now), Err(OidcError::OtherBrowser));
        assert_eq!(oidc.finish(&back(&state), [], now), Err(OidcError::UnknownState)); // and it is gone

        let (state, _, _, binding) = started(&oidc, now);
        assert_eq!(oidc.finish(&back(&state), [binding.as_str()], now + FLOW_LASTS), Err(OidcError::FlowExpired));

        let (state, _, _, binding) = started(&oidc, now);
        let refused = Callback { code: None, state: Some(state), error: Some("access_denied".to_owned()) };
        assert_eq!(
            oidc.finish(&refused, [binding.as_str()], now),
            Err(OidcError::ProviderRefused("access_denied".to_owned()))
        );

        let (state, nonce, _, binding) = started(&oidc, now);
        fake.script().id_token = Some(signer.sign(&claims(&nonce)));
        fake.script().token_status = 400; // a code the provider does not know (replayed at the provider)
        assert_eq!(oidc.finish(&back(&state), [binding.as_str()], now), Err(OidcError::CodeRefused(400)));
    }

    #[test]
    fn started_sign_ins_never_grow_beyond_their_limit() {
        let fake = Fake::new(&[]);
        let oidc = oidc(&fake);
        let now = Instant::now();
        for _ in 0..MOST_FLOWS + 10 {
            oidc.begin(now).unwrap();
        }
        assert_eq!(oidc.flows.lock().unwrap().len(), MOST_FLOWS);
        assert_eq!(fake.count("openid-configuration"), 1, "the discovery document is kept");
    }

    /// Validate a token with these claims changed, signed by `signer`.
    fn checked(oidc: &Oidc, signer: &Signer, change: impl FnOnce(&mut Value)) -> Result<String, OidcError> {
        let mut claims = claims("the-nonce");
        change(&mut claims);
        oidc.validate(&signer.sign(&claims), "the-nonce", Instant::now())
    }

    #[test]
    fn every_claim_that_is_not_right_is_a_refusal() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let oidc = oidc(&fake);
        let now = now_unix();
        assert!(checked(&oidc, &signer, |_| {}).is_ok());
        type Change = Box<dyn FnOnce(&mut Value)>;
        let cases: Vec<(Change, OidcError)> = vec![
            (Box::new(|claims| claims["aud"] = json!("another-client")), OidcError::Audience),
            (Box::new(|claims| claims["aud"] = json!(["another-client", "a-third"])), OidcError::Audience),
            // several audiences: azp must then say this client
            (
                Box::new(|claims| {
                    claims["aud"] = json!([CLIENT, "x"]);
                    claims.as_object_mut().unwrap().remove("azp");
                }),
                OidcError::Audience,
            ),
            (Box::new(|claims| claims["azp"] = json!("another-client")), OidcError::Audience),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("aud");
                }),
                OidcError::Audience,
            ),
            (Box::new(|claims| claims["iss"] = json!("https://evil.test")), OidcError::Issuer),
            (Box::new(|claims| claims["iss"] = json!("provider.test")), OidcError::Issuer),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("iss");
                }),
                OidcError::Issuer,
            ),
            (Box::new(|claims| claims["nonce"] = json!("another-nonce")), OidcError::Nonce),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("nonce");
                }),
                OidcError::Nonce,
            ),
            (Box::new(move |claims| claims["exp"] = json!(now - 120)), OidcError::Expired),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("exp");
                }),
                OidcError::Expired,
            ),
            (Box::new(move |claims| claims["iat"] = json!(now + 600)), OidcError::IssuedAt),
            (Box::new(move |claims| claims["iat"] = json!(now - FRESH.as_secs() - 120)), OidcError::IssuedAt),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("iat");
                }),
                OidcError::Malformed,
            ),
            (Box::new(|claims| claims["email_verified"] = json!(false)), OidcError::EmailNotVerified),
            (Box::new(|claims| claims["email_verified"] = json!("yes")), OidcError::EmailNotVerified),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("email_verified");
                }),
                OidcError::EmailNotVerified,
            ),
            (Box::new(|claims| claims["email"] = json!("someone.else@example.org")), OidcError::NotTheOwner),
            (Box::new(|claims| claims["email"] = json!("owner.name@example.org")), OidcError::NotTheOwner), // local part exact
            (Box::new(|claims| claims["email"] = json!("Owner.Name@example.org.evil.test")), OidcError::NotTheOwner),
            (
                Box::new(|claims| {
                    claims.as_object_mut().unwrap().remove("email");
                }),
                OidcError::NotTheOwner,
            ),
        ];
        for (index, (change, expected)) in cases.into_iter().enumerate() {
            assert_eq!(checked(&oidc, &signer, change), Err(expected), "case {index}");
        }
        // within the clock's leeway is fine; the domain in any case is the same domain
        assert!(checked(&oidc, &signer, move |claims| claims["exp"] = json!(now - 30)).is_ok());
        assert!(checked(&oidc, &signer, move |claims| claims["iat"] = json!(now + 30)).is_ok());
        assert!(checked(&oidc, &signer, |claims| claims["email"] = json!("Owner.Name@EXAMPLE.org")).is_ok());
        assert!(checked(&oidc, &signer, |claims| claims["email_verified"] = json!("true")).is_ok());
        assert!(checked(&oidc, &signer, |claims| claims["aud"] = json!([CLIENT, "another-client"])).is_ok());
    }

    #[test]
    fn a_signature_that_is_not_the_providers_or_an_algorithm_that_is_not_accepted_is_a_refusal() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let oidc = oidc(&fake);
        let good = signer.sign(&claims("the-nonce"));
        let now = Instant::now();

        // another key that claims to be k1
        let impostor = rsa_signer(1, "k1");
        assert_eq!(oidc.validate(&impostor.sign(&claims("the-nonce")), "the-nonce", now), Err(OidcError::Signature));
        // the claims changed after signing
        let mut parts: Vec<String> = good.split('.').map(str::to_owned).collect();
        let mut changed = claims("the-nonce");
        changed["email"] = json!("Owner.Name@example.org ");
        parts[1] = URL_SAFE_NO_PAD.encode(changed.to_string());
        assert_eq!(oidc.validate(&parts.join("."), "the-nonce", now), Err(OidcError::Signature));
        // no signature at all, or a shared secret made of the public key
        let none = format!("{}.{}.", URL_SAFE_NO_PAD.encode(r#"{"alg":"none","kid":"k1"}"#), parts[1]);
        assert_eq!(oidc.validate(&none, "the-nonce", now), Err(OidcError::Malformed));
        let hmac = jsonwebtoken::encode(
            &Header { kid: Some("k1".to_owned()), ..Header::new(Algorithm::HS256) },
            &claims("the-nonce"),
            &EncodingKey::from_secret(b"anything"),
        )
        .unwrap();
        assert_eq!(oidc.validate(&hmac, "the-nonce", now), Err(OidcError::Algorithm));
        // an RS256 header on an EC key
        let ec = ec_signer("e1");
        fake.script().keys.push(ec.public.clone());
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("e1".to_owned());
        let crossed = jsonwebtoken::encode(&header, &claims("the-nonce"), &signer.encoding).unwrap();
        assert!(oidc.validate(&crossed, "the-nonce", now + KEYS_REFETCH).is_err());
        // a key named inside the token
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("k1".to_owned());
        header.jwk = Some(impostor.public.clone());
        let embedded = jsonwebtoken::encode(&header, &claims("the-nonce"), &impostor.encoding).unwrap();
        assert_eq!(oidc.validate(&embedded, "the-nonce", now), Err(OidcError::Malformed));
        assert_eq!(oidc.validate("not.a.token", "the-nonce", now), Err(OidcError::Malformed));
        // and the right one, still
        assert!(oidc.validate(&good, "the-nonce", now).is_ok());
    }

    #[test]
    fn elliptic_curve_keys_are_accepted_as_well() {
        let signer = ec_signer("e1");
        let fake = Fake::new(&[&signer]);
        assert!(oidc(&fake).validate(&signer.sign(&claims("n")), "n", Instant::now()).is_ok());
    }

    #[test]
    fn a_rotated_key_is_fetched_once_and_made_up_key_ids_do_not_hammer_the_provider() {
        let old = rsa_signer(0, "k1");
        let new = rsa_signer(1, "k2");
        let fake = Fake::new(&[&old]);
        let oidc = oidc(&fake);
        let now = Instant::now();
        assert!(oidc.validate(&old.sign(&claims("n")), "n", now).is_ok());
        assert_eq!(fake.count("/keys"), 1);
        assert!(oidc.validate(&old.sign(&claims("n")), "n", now).is_ok());
        assert_eq!(fake.count("/keys"), 1, "known keys are kept");

        // the provider rotates: k2 appears, k1 is retired
        fake.script().keys = vec![new.public.clone()];
        let later = now + KEYS_REFETCH;
        assert!(oidc.validate(&new.sign(&claims("n")), "n", later).is_ok());
        assert_eq!(fake.count("/keys"), 2);
        assert_eq!(oidc.validate(&old.sign(&claims("n")), "n", later), Err(OidcError::UnknownKey));

        // tokens naming keys nobody published: no more than one fetch a minute
        let stranger = rsa_signer(1, "k-made-up");
        for _ in 0..20 {
            assert_eq!(oidc.validate(&stranger.sign(&claims("n")), "n", later), Err(OidcError::UnknownKey));
        }
        assert_eq!(fake.count("/keys"), 2);
        // and after the keys are kept for long enough, they are fetched again anyway
        assert!(oidc.validate(&new.sign(&claims("n")), "n", now + KEPT + KEYS_REFETCH).is_ok());
        assert_eq!(fake.count("/keys"), 3);
    }

    #[test]
    fn a_discovery_document_of_another_issuer_or_without_https_or_pkce_is_refused() {
        let signer = rsa_signer(0, "k1");
        for (field, value) in [
            ("issuer", json!("https://evil.test")),
            ("token_endpoint", json!("http://provider.test/token")),
            ("authorization_endpoint", json!("javascript:alert(1)")),
            ("jwks_uri", json!("http://provider.test/keys")),
            ("code_challenge_methods_supported", json!(["plain"])),
        ] {
            let fake = Fake::new(&[&signer]);
            fake.script().discovery[field] = value;
            assert!(matches!(oidc(&fake).begin(Instant::now()), Err(OidcError::Discovery(_))), "{field}");
        }
        // a key set with a kind of key this engine does not know still gives the keys it does
        let mixed = format!(r#"{{"keys":[{{"kty":"made-up"}},{}]}}"#, serde_json::to_string(&signer.public).unwrap());
        assert_eq!(parse_keys(mixed.as_bytes()).unwrap().len(), 1);
    }

    #[test]
    fn a_token_not_valid_yet_is_refused() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let mut early = claims("n");
        early["nbf"] = json!(now_unix() + 600);
        assert_eq!(oidc(&fake).validate(&signer.sign(&early), "n", Instant::now()), Err(OidcError::IssuedAt));
    }

    #[test]
    fn when_the_provider_does_not_answer_for_its_keys_the_ones_it_published_before_still_work() {
        let signer = rsa_signer(0, "k1");
        let fake = Fake::new(&[&signer]);
        let oidc = oidc(&fake);
        let now = Instant::now();
        assert!(oidc.validate(&signer.sign(&claims("n")), "n", now).is_ok());
        fake.script().discovery["jwks_uri"] = json!("https://provider.test/gone"); // not seen: the endpoints are kept
        *oidc.endpoints.lock().unwrap() = None;
        let later = now + KEPT + KEYS_REFETCH;
        assert!(oidc.validate(&signer.sign(&claims("n")), "n", later).is_ok());
        assert!(fake.count("/gone") >= 1, "the keys were asked for again");
    }

    /// Google as the provider: its discovery document and keys at its own addresses.
    struct Google(Vec<Jwk>);

    impl Provider for Google {
        fn get(&self, url: &Url) -> Result<Answer, Unreachable> {
            let body = match url.as_str() {
                "https://accounts.google.com/.well-known/openid-configuration" => json!({
                    "issuer": "https://accounts.google.com",
                    "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
                    "token_endpoint": "https://oauth2.googleapis.com/token",
                    "jwks_uri": "https://www.googleapis.com/oauth2/v3/certs",
                    "code_challenge_methods_supported": ["plain", "S256"],
                }),
                "https://www.googleapis.com/oauth2/v3/certs" => json!({ "keys": self.0 }),
                _ => return Ok(Answer { status: 404, body: Vec::new() }),
            };
            Ok(Answer { status: 200, body: body.to_string().into_bytes() })
        }

        fn post_form(&self, _: &Url, _: &[(&str, &str)]) -> Result<Answer, Unreachable> {
            Ok(Answer { status: 404, body: Vec::new() })
        }
    }

    #[test]
    fn the_sign_in_button_says_google_for_google_and_the_issuers_host_otherwise() {
        let named = |issuer: &str| {
            let client = Client { issuer: issuer.to_owned(), ..client() };
            Oidc::new(client, REDIRECT.to_owned(), Arc::new(Fake::new(&[]))).name()
        };
        assert_eq!(named("https://accounts.google.com"), "Google");
        assert_eq!(named("https://accounts.google.com/"), "Google");
        assert_eq!(named(ISSUER), "provider.test");
        assert_eq!(named("https://login.example.org/realms/papers"), "login.example.org");
        assert_eq!(named("not an address"), "");
    }

    #[test]
    fn googles_tokens_may_name_the_issuer_without_its_scheme_and_its_addresses_are_compared_in_any_case() {
        let signer = rsa_signer(0, "g1");
        let client = Client { issuer: super::super::GOOGLE.to_owned(), ..client() };
        let google = Oidc::new(client, REDIRECT.to_owned(), Arc::new(Google(vec![signer.public.clone()])));
        let now = Instant::now();
        let with = |issuer: &str, email: &str| {
            let mut claims = claims("n");
            claims["iss"] = json!(issuer);
            claims["email"] = json!(email);
            google.validate(&signer.sign(&claims), "n", now)
        };
        assert_eq!(with("https://accounts.google.com", "Owner.Name@example.org"), Ok("Owner.Name@example.org".into()));
        assert_eq!(with("accounts.google.com", "Owner.Name@example.org"), Ok("Owner.Name@example.org".into()));
        // Gmail addresses are not case sensitive: the owner in another case is the owner, as Google sends it
        assert_eq!(with("accounts.google.com", "owner.name@EXAMPLE.org"), Ok("owner.name@EXAMPLE.org".into()));
        assert_eq!(with("https://accounts.google.com", "ownername@example.org"), Err(OidcError::NotTheOwner));
        for other in ["google.com", "http://accounts.google.com", "https://accounts.google.com.evil.test"] {
            assert_eq!(with(other, "Owner.Name@example.org"), Err(OidcError::Issuer), "{other}");
        }
        // and the discovered authorization endpoint is Google's
        let begun = google.begin(now).unwrap();
        assert!(begun.location.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"), "{}", begun.location);
    }

    #[test]
    fn the_owners_address_is_compared_exactly_but_for_the_case_of_its_domain() {
        let owner = Owner::parse(" Owner.Name@Example.ORG ").unwrap();
        assert!(owner.is("Owner.Name@example.org", false));
        for other in
            ["owner.name@example.org", "OwnerName@example.org", "Owner.Name@example.org.", "Owner.Name", "@example.org"]
        {
            assert!(!owner.is(other, false), "{other}");
        }
        // for Google, whose addresses are not case sensitive, the part before the @ in any case too
        assert!(owner.is("owner.name@example.org", true));
        assert!(!owner.is("ownername@example.org", true));
        for bad in ["", "no-at-sign", "two@@example.org", "a b@example.org", "a@", "@b"] {
            assert!(Owner::parse(bad).is_none(), "{bad}");
        }
    }
}
