//! The language model: questions to a provider's API over HTTPS, answers as JSON, every call priced and recorded.
//!
//! The provider is a choice ([`Provider`], with one row per provider in `provider.rs`): the Claude API, OpenAI,
//! Gemini, a dozen hosted providers that speak OpenAI's Chat Completions, and any OpenAI-compatible server the user
//! points it at (Ollama or LM Studio on this computer, a gateway). A [`Client`] is made for one provider's
//! [`Endpoint`], and callers ask it the same [`Ask`] whichever it is: each provider's request and answer shapes are
//! in a file of their own (`anthropic.rs`, `responses.rs`, `gemini.rs`, `chat.rs`). Where a provider cannot do
//! something the features rely on, the client says so and does what it can instead, never silently:
//!
//! | it cannot                       | so                                                                       |
//! |---------------------------------|--------------------------------------------------------------------------|
//! | hold an answer to a schema      | the schema is asked for in the question, the answer checked here (`schema.rs`), asked once more, then [`NoAnswer::NotJson`] |
//! | cache the shared prefix         | [`Estimate::caching`] says so, and the prefix is priced in full for every question |
//! | count tokens                    | they are estimated from characters, marked [`Counting::Assumed`]           |
//! | report its usage                | the call is counted at its worst case (an answer of `max_tokens`): hidden thinking has no characters |
//! | publish a price                 | the cost is "not known" (`None`), never 0, and a budget in dollars is refused |
//!
//! Whatever model answers, quotes it gives are evidence only once found in the paper (`crate::evidence`): a weaker
//! model's answers pass through the same check.
//!
//! The rest of this describes the Claude API, the first provider and the one whose behaviour is unchanged by the
//! others. There is no official Rust SDK, so this is the Messages API over plain HTTP (`ureq`, as the sources use),
//! as the API's own reference describes it for raw HTTP: `POST /v1/messages` with `x-api-key`, `anthropic-version`
//! and a JSON body. What it does, and why:
//!
//! - **Models** are listed once, in [`MODELS`], with what the interface shows for each; [`DEFAULT_MODEL`] is the
//!   reference's default, Claude Opus 5.5.
//! - **Thinking** is adaptive on every model offered (`thinking: {type: "adaptive"}`); how hard the model thinks
//!   is the request's [`Effort`]. Claude Opus 5.5 and Claude Haiku 5.5 default to `medium` effort when none is
//!   sent, so it is always sent.
//! - **Answers are JSON** matching a schema, through structured outputs (`output_config.format`), since every
//!   feature needs an answer a program can read; or plain text when a feature asks for prose.
//! - **The long shared prefix is cached**: a paper's text or a community's papers, sent many times with different
//!   questions, goes first and is marked `cache_control: ephemeral`, as is the system prompt, and the question
//!   comes after the last mark. Whether it worked is in each answer's usage (`cache_read_input_tokens`), which is
//!   recorded with every call.
//! - **A refusal, or an answer cut off at `max_tokens`, is told before the content is read**: neither is an
//!   answer. Where the API can hand a declined request to another model in the same call (server-side fallbacks),
//!   it is asked to.
//! - **Errors are of two kinds**, as in the Python backend: [`Unavailable`] (no key, a refused key, an unknown
//!   model, the spend limit: stop asking) and [`NoAnswer`] (a rate limit, an overloaded or failing server, a
//!   refusal: the next request may work). A rate limit is waited out as its `retry-after` says, a few times.
//! - **Every call is recorded** for the ledger ([`Recorder`], `usage::Call`) with what it was for and its tokens
//!   by how they are billed, cache reads and writes apart; a call is recorded whether its answer is usable or
//!   not, since it was paid for either way.
//! - **A budget** ([`Budget`]) is checked before every call: a call whose worst case would take the batch past
//!   what its caller is willing to spend is not started. The worst case of a request sent with fallbacks is the
//!   model asked and the dearest fallback after it ([`worst_of`]), each at its worst, since a refusal part way is
//!   billed too; what is held is a [`Hold`], given back however the call ends unless it is settled.
//!   [`estimate_for`] says what a batch is likely to cost before it is started, and at most the same worst case.
//! - **The key** is read from `ANTHROPIC_API_KEY` only, the address from `ANTHROPIC_BASE_URL` when it is set (and
//!   each other provider's from the variables its row names). Neither is ever logged, shown in a `Debug`, put in an
//!   error, or returned by a route ([`Providers`] tells only whether a key is set).
//!
//! Every test runs against a fake [`Transport`] serving answers shaped as the API's reference documents them: no
//! test makes a real call, which would cost money.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::library::Library;
use crate::usage::{Call, Tokens, Usd, price_of};

mod anthropic;
mod chat;
mod gemini;
pub mod provider;
mod providers;
mod responses;
mod schema;

use anthropic::{Block, Message, Usage, failure};
pub use provider::{
    About, Address, Answers, Caching, Capabilities, EffortControl, Key, ModelIds, Offered, Provider, TokenCount,
    variables,
};
use provider::{Api, effort_for};
pub use providers::{AddressState, Availability, KeyState, Listed, ListedModel, Place, Providers};
pub use schema::strict_ready;

/// The version of the API this speaks (`anthropic-version`).
pub const API_VERSION: &str = "2026-10-21";
/// Where the key is read from, and nowhere else.
pub const KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";
/// Where requests go when it is set (a proxy, a gateway); Anthropic's own endpoint when not.
pub const BASE_VARIABLE: &str = "ANTHROPIC_BASE_URL";
const DEFAULT_BASE: &str = "https://api.anthropic.com";
/// The beta that lets the API hand a declined request to another model in the same call.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// How long one request may take: a long prompt and a long answer take minutes.
const PATIENCE: Duration = Duration::from_secs(10 * 60);
/// Times a request that gave nothing usable for a passing reason (rate limit, overload) is tried again.
const RETRIES: u32 = 2;
/// A `retry-after` longer than this is not waited for: the request gives up instead.
const LONGEST_WAIT: Duration = Duration::from_secs(90);
/// Most bytes of an answer read: an answer is at most 128K tokens of text.
const LARGEST_ANSWER: u64 = 16 * 1024 * 1024;
/// Characters per token assumed when tokens are estimated from characters rather than counted. Low for English
/// prose, so that what is estimated from it errs on the high side; [`Client::calibrate`] measures the real one.
pub const CHARS_PER_TOKEN: f64 = 3.0;

/// Whether the API can hand a request this model declines to another model within the same call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// `fallbacks: "default"`: the API picks the model by why it declined.
    ServerSide,
    None,
}

/// A model the interface offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: &'static str,
    /// As shown: "Claude Opus 5.5".
    pub label: &'static str,
    /// A few words beside it.
    pub note: &'static str,
    pub fallback: Fallback,
}

/// The models offered, the default first: the one list of them (the settings' choices come from it).
pub const MODELS: [Model; 4] = [
    Model { id: "claude-opus-5-5", label: "Claude Opus 5.5", note: "the default", fallback: Fallback::ServerSide },
    Model {
        id: "claude-sonnet-5-5",
        label: "Claude Sonnet 5.5",
        note: "faster, about half the cost",
        fallback: Fallback::ServerSide,
    },
    Model {
        id: "claude-haiku-5-5",
        label: "Claude Haiku 5.5",
        note: "cheapest, least thorough",
        fallback: Fallback::None,
    },
    Model {
        id: "claude-fable-5-2",
        label: "Claude Fable 5.2",
        note: "most capable, about twice the cost",
        fallback: Fallback::ServerSide,
    },
];

/// The model used when none is chosen.
pub const DEFAULT_MODEL: &str = MODELS[0].id;

/// The model offered under `id`.
pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.id == id)
}

/// The ids of the models offered, for the settings' choices.
pub fn model_ids() -> Vec<String> {
    MODELS.iter().map(|model| model.id.to_owned()).collect()
}

/// The models the Claude API may hand a declined request to under `fallbacks: "default"`: the permitted fallback
/// targets, as the API's reference lists them (which one is picked, by the reason for declining, is not published).
/// A request sent with fallbacks may be answered, and billed, by any of them, so it is held and estimated at the
/// dearest of these and the model asked.
pub const FALLBACK_TARGETS: [&str; 2] = ["claude-opus-4-8", "claude-opus-5"];

/// Does a request to `model` of `provider` go with `fallbacks`?
fn falls_back(provider: Provider, model: &str) -> bool {
    provider == Provider::Anthropic && self::model(model).is_some_and(|model| model.fallback == Fallback::ServerSide)
}

/// The price keys of every model a request to `model` of `provider` may be billed by: the model asked first, then
/// the fallback targets when the request goes with fallbacks ([`worst_of`] says how they add up). One of them
/// without a price makes the call unpriced (held to a budget in tokens), never guessed.
pub fn answered_by(provider: Provider, model: &str) -> Vec<String> {
    let mut models = vec![price_key(provider, model)];
    if falls_back(provider, model) {
        models.extend(FALLBACK_TARGETS.iter().map(|target| price_key(provider, target)));
    }
    models
}

/// The most one call of worst case `worst` can take, when it may be billed by `models` ([`answered_by`]): the
/// model asked, and, when there are others, one of them after it. A declined request is handed on within the
/// same call, and the reference says that a refusal part way through is billed at the normal rates, so the worst
/// is the model asked at its worst plus the dearest fallback at its worst: tokens of every kind, and dollars, or
/// the price key of a model whose price is not known.
pub fn worst_of<S: AsRef<str>>(models: &[S], worst: &Tokens) -> (u64, Result<Usd, String>) {
    let Some((asked, fallbacks)) = models.split_first() else {
        return (worst.all(), Err(String::new()));
    };
    let price = |model: &S| price_of(model.as_ref(), worst).ok_or_else(|| model.as_ref().to_owned());
    let attempts = if fallbacks.is_empty() { 1 } else { 2 };
    let usd = price(asked).and_then(|first| {
        fallbacks.iter().map(price).try_fold(first, |sum, cost| cost.map(|cost| sum.max(first + cost)))
    });
    (worst.all().saturating_mul(attempts), usd)
}

/// The API key. It is shown to nothing: its `Debug` says only that there is one.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// Only for a [`Transport`], which must send it as the `x-api-key` header and nowhere else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiKey(hidden)")
    }
}

/// How a [`Transport`] sends the key: each provider reads it from a header of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyHeader {
    /// `x-api-key` (the Claude API).
    XApiKey,
    /// `Authorization: Bearer` (OpenAI and everything that speaks its APIs).
    Bearer,
    /// `x-goog-api-key` (Gemini).
    GoogApiKey,
    /// No key at all: a server on this computer that needs none.
    NotSent,
}

/// Where requests go, with what key, for which provider.
#[derive(Clone)]
pub struct Endpoint {
    key: ApiKey,
    base: Url,
    provider: Provider,
    /// How JSON answers are had: the provider's, unless [`provider::ANSWERS_VARIABLE`] says otherwise.
    answers: Answers,
}

/// `text` as an address a key may be sent to: https, or plain http only to this computer.
fn safe_base(text: &str) -> Option<Url> {
    let base = Url::parse(text).ok()?;
    // the key goes in a header: never over plain HTTP to another computer
    (base.scheme() == "https" || (base.scheme() == "http" && on_loopback(&base))).then_some(base)
}

fn on_loopback(base: &Url) -> bool {
    matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // the address may carry credentials of a proxy: neither it nor the key is shown
        formatter.write_str("Endpoint(hidden)")
    }
}

impl Endpoint {
    /// From the process's environment.
    pub fn from_environment() -> Result<Self, Unavailable> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// From whatever `lookup` says the variables are (the tests' way, and a caller's that reads them elsewhere).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, Unavailable> {
        let given = |name: &str| lookup(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        let key = given(KEY_VARIABLE).ok_or(Unavailable::NoKey)?;
        let base = given(BASE_VARIABLE).unwrap_or_else(|| DEFAULT_BASE.to_owned());
        let base = safe_base(&base).ok_or(Unavailable::BadEndpoint)?;
        Ok(Self { key: ApiKey(key), base, provider: Provider::Anthropic, answers: Answers::Enforced })
    }

    /// The endpoint of `provider`, from whatever `lookup` says the variables of its row are: its key (required, or
    /// optional for a server on this computer) and its address. The Claude API's is [`Endpoint::from_lookup`]'s.
    pub fn for_provider(provider: Provider, lookup: impl Fn(&str) -> Option<String>) -> Result<Self, Unavailable> {
        if provider == Provider::Anthropic {
            return Self::from_lookup(lookup);
        }
        let about = provider.about();
        let given = |name: &str| lookup(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        let key = match about.key {
            Key::Required(variable) => given(variable).ok_or(Setup::KeyNotSet(variable))?,
            Key::Optional(variable) => given(variable).unwrap_or_default(),
        };
        let base = match about.address {
            Address::Fixed(default) => default.to_owned(),
            Address::Overridable { variable, default } => given(variable).unwrap_or_else(|| default.to_owned()),
            Address::Required(variable) => given(variable).ok_or(Setup::AddressNotSet(variable))?,
        };
        let base = safe_base(&base).ok_or(Setup::NotHttps(about.address.variable().unwrap_or(about.id)))?;
        let answers = match (provider, given(provider::ANSWERS_VARIABLE).as_deref()) {
            (Provider::Compatible, Some("prompt")) => Answers::Prompted,
            _ => about.capabilities.answers,
        };
        Ok(Self { key: ApiKey(key), base, provider, answers })
    }

    /// Is the endpoint the provider's own, or another the user set?
    pub fn is_default(&self) -> bool {
        let own = match self.provider.about().address {
            Address::Fixed(default) | Address::Overridable { default, .. } => default,
            Address::Required(_) => return false,
        };
        self.base.as_str().trim_end_matches('/') == own
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// What the provider can do, as this endpoint reaches it.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities { answers: self.answers, ..self.provider.about().capabilities }
    }

    /// The name of the computer requests go to, with its port when it names one: what the settings show of the
    /// address (never its path, which may carry a gateway's credentials, nor anything else).
    pub fn host(&self) -> Option<String> {
        let host = self.base.host_str()?;
        Some(match self.base.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        })
    }

    /// Is the model run on this computer (a server on loopback), so that it costs nothing in API fees?
    pub fn on_this_computer(&self) -> bool {
        self.provider == Provider::Compatible && on_loopback(&self.base)
    }

    fn key_header(&self) -> KeyHeader {
        if self.key.0.is_empty() {
            return KeyHeader::NotSent;
        }
        match self.provider.about().api {
            Api::Messages => KeyHeader::XApiKey,
            Api::Responses | Api::ChatCompletions(_) => KeyHeader::Bearer,
            Api::GenerateContent => KeyHeader::GoogApiKey,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.as_str().trim_end_matches('/'))
    }
}

/// How hard the model thinks before it answers.
/// Ordered from least to most, so that a feature's cap on it is a `min`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The form an answer must take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Form<'a> {
    /// JSON matching this schema (every object `additionalProperties: false`, as structured outputs require).
    Json(&'a Value),
    Text,
}

/// One question to the model.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    /// What it is for, as the ledger shows it: "questions about a paper".
    pub purpose: &'a str,
    pub model: &'a str,
    /// The instructions: the same for every question of a feature, so cached with the prefix.
    pub system: &'a str,
    /// The long part sent many times with different questions (a paper's text, a community's papers): cached.
    /// Must be byte for byte the same each time for the cache to be read.
    pub shared: Option<&'a str>,
    /// What changes from one call to the next; after the cached part.
    pub question: &'a str,
    pub form: Form<'a>,
    pub effort: Effort,
    /// The most the answer may take, thinking included.
    pub max_tokens: u32,
}

impl Ask<'_> {
    fn characters(&self) -> (u64, u64) {
        let count = |text: &str| u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        (count(self.system) + self.shared.map_or(0, count), count(self.question))
    }
}

/// An answer as it came back from the transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    /// The `retry-after` header, as sent.
    pub retry_after: Option<String>,
    /// The `x-should-retry` header, as sent.
    pub should_retry: Option<String>,
    pub body: Vec<u8>,
}

/// Why no answer came back at all. No message: the transport's own could name the address.
///
/// The first two are before the request was taken in (nothing can have been billed); [`TransportError::Lost`]
/// is after it was sent, when the provider may have answered and billed it: it is not sent again, which could
/// pay twice, and the call's outcome is left as unknown (`library::asking`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// No connection could be made (the address, the network, TLS): nothing was sent.
    #[error("the language model's API could not be reached")]
    Unreachable,
    /// The time to connect or to send the request ran out: it was not taken in.
    #[error("the language model's API took too long to answer")]
    TimedOut,
    /// The request was sent, and then the connection was lost or the answer took too long or could not be read:
    /// it may have been answered and billed.
    #[error("the language model's API stopped answering part way: the request may have been answered, and billed")]
    Lost,
    /// Not sent: the work was asked to stop while the request waited for its turn (after a rate limit).
    #[error("asking was stopped")]
    Cancelled,
}

/// The one way this reaches the network. An implementation sends `key` as the `x-api-key` header and nowhere
/// else, logs nothing of the request, and gives up after `patience`.
pub trait Transport: Send + Sync {
    fn post(
        &self,
        url: &str,
        key: &ApiKey,
        headers: &[(&str, &str)],
        body: &[u8],
        patience: Duration,
    ) -> Result<Reply, TransportError>;

    /// [`Transport::post`] with the key sent as the provider reads it (`sent_as`), and in no other place. What
    /// every request goes through. A transport that was written for the Claude API alone keeps working for it: by
    /// default this is `post`, which sends the key as `x-api-key`.
    fn post_keyed(
        &self,
        url: &str,
        key: &ApiKey,
        sent_as: KeyHeader,
        headers: &[(&str, &str)],
        body: &[u8],
        patience: Duration,
    ) -> Result<Reply, TransportError> {
        let _ = sent_as;
        self.post(url, key, headers, body, patience)
    }

    /// A `GET` (a server's list of its models), the key sent as with [`Transport::post_keyed`]. A transport without
    /// it reaches nothing.
    fn get(&self, url: &str, key: &ApiKey, sent_as: KeyHeader, patience: Duration) -> Result<Reply, TransportError> {
        let _ = (url, key, sent_as, patience);
        Err(TransportError::Unreachable)
    }

    /// Wait before trying again; the tests' transport only notes it.
    fn wait(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// [`Transport`] over HTTPS, through `ureq` and rustls.
pub struct Https {
    agent: ureq::Agent,
}

impl Default for Https {
    fn default() -> Self {
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::Rustls).build();
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(crate::sources::USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            .tls_config(tls)
            .build()
            .into();
        Self { agent }
    }
}

impl Transport for Https {
    fn post(
        &self,
        url: &str,
        key: &ApiKey,
        headers: &[(&str, &str)],
        body: &[u8],
        patience: Duration,
    ) -> Result<Reply, TransportError> {
        self.post_keyed(url, key, KeyHeader::XApiKey, headers, body, patience)
    }

    fn post_keyed(
        &self,
        url: &str,
        key: &ApiKey,
        sent_as: KeyHeader,
        headers: &[(&str, &str)],
        body: &[u8],
        patience: Duration,
    ) -> Result<Reply, TransportError> {
        let mut request = self.agent.post(url).config().timeout_global(Some(patience)).build();
        request = keyed(request, key, sent_as).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        reply_of(request.send(body).map_err(transport_failed)?)
    }

    fn get(&self, url: &str, key: &ApiKey, sent_as: KeyHeader, patience: Duration) -> Result<Reply, TransportError> {
        let request = self.agent.get(url).config().timeout_global(Some(patience)).build();
        reply_of(keyed(request, key, sent_as).call().map_err(transport_failed)?)
    }
}

/// What a failure of `ureq` means for the request: not sent, or sent and maybe billed. Only what certainly comes
/// before the request is taken in (resolving, connecting, TLS, sending) counts as not sent; anything else (an
/// I/O error part way, a timeout waiting for the answer, an answer that cannot be read) is [`TransportError::Lost`].
fn transport_failed(error: ureq::Error) -> TransportError {
    use std::io::ErrorKind;
    use ureq::Timeout;
    match error {
        ureq::Error::Timeout(Timeout::Resolve | Timeout::Connect | Timeout::SendRequest | Timeout::SendBody) => {
            TransportError::TimedOut
        }
        ureq::Error::Timeout(_) => TransportError::Lost,
        ureq::Error::Io(io)
            if matches!(
                io.kind(),
                ErrorKind::ConnectionRefused
                    | ErrorKind::NetworkUnreachable
                    | ErrorKind::HostUnreachable
                    | ErrorKind::AddrNotAvailable
                    | ErrorKind::NetworkDown
            ) =>
        {
            TransportError::Unreachable
        }
        ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::BadUri(_)
        | ureq::Error::Http(_)
        | ureq::Error::InvalidProxyUrl
        | ureq::Error::ConnectProxyFailed(_)
        | ureq::Error::RequireHttpsOnly(_)
        | ureq::Error::TlsRequired
        | ureq::Error::Tls(_)
        | ureq::Error::Pem(_) => TransportError::Unreachable,
        _ => TransportError::Lost,
    }
}

/// The request with the key in the one header `sent_as` names.
fn keyed<B>(request: ureq::RequestBuilder<B>, key: &ApiKey, sent_as: KeyHeader) -> ureq::RequestBuilder<B> {
    match sent_as {
        KeyHeader::XApiKey => request.header("x-api-key", key.expose()),
        KeyHeader::Bearer => request.header("authorization", format!("Bearer {}", key.expose())),
        KeyHeader::GoogApiKey => request.header("x-goog-api-key", key.expose()),
        KeyHeader::NotSent => request,
    }
}

/// The answer as a [`Reply`]. A wait asked for in milliseconds (`retry-after-ms`, which some OpenAI-compatible
/// servers send instead) is given in seconds, as `retry-after` is.
fn reply_of(mut response: ureq::http::Response<ureq::Body>) -> Result<Reply, TransportError> {
    let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
    let in_millis = || {
        header("retry-after-ms")
            .and_then(|millis| millis.trim().parse::<f64>().ok())
            .map(|ms| (ms / 1000.0).to_string())
    };
    let retry_after = header("retry-after").or_else(in_millis);
    let should_retry = header("x-should-retry");
    let status = response.status().as_u16();
    // the status is in: the request was taken in, so an answer that cannot be read may still have been billed
    let body =
        response.body_mut().with_config().limit(LARGEST_ANSWER).read_to_vec().map_err(|_| TransportError::Lost)?;
    Ok(Reply { status, retry_after, should_retry, body })
}

/// Where each call is recorded for the ledger.
pub trait Recorder: Send + Sync {
    fn record(&self, call: Call);

    /// Keep an answer just paid for in `library` and record the `calls` that paid for it. The library's own ledger
    /// does both in one transaction ([`Library::keep_paid`]); any other recorder keeps, then records.
    fn keep_paid(
        &self,
        library: &Library,
        slot: &crate::library::Slot,
        model: &str,
        prompt: u32,
        content: &str,
        calls: Vec<Call>,
    ) -> Result<(), crate::library::LibraryError> {
        let kept = library.keep_paid(slot, model, prompt, content, &[]);
        for call in calls {
            self.record(call);
        }
        kept
    }

    /// Record the `calls` a question in `slot` took that gave no answer to keep, and remove the intent written in
    /// `library` before it was sent (`library::asking`): the ledger now knows what it cost. The library's own
    /// ledger does both in one transaction ([`Library::record_paid`]).
    fn record_paid(
        &self,
        library: &Library,
        slot: &crate::library::Slot,
        calls: Vec<Call>,
    ) -> Result<(), crate::library::LibraryError> {
        for call in calls {
            self.record(call);
        }
        library.not_billed(slot).map(drop)
    }
}

/// A call as the library's ledger keeps it: whole millionths of a dollar, counts that fit its columns.
fn ledger_call(call: &Call) -> crate::library::ledger::Call<'_> {
    let signed = |count: u64| i64::try_from(count).unwrap_or(i64::MAX);
    crate::library::ledger::Call {
        model: &call.model,
        purpose: &call.purpose,
        input_tokens: signed(call.tokens.input),
        output_tokens: signed(call.tokens.output),
        cache_read_tokens: signed(call.tokens.cache_read),
        cache_write_tokens: signed(call.tokens.cache_written()),
        price_micros: call.cost().map(|cost| signed(cost.picodollars().div_ceil(1_000_000))),
    }
}

/// The calls of a session, in memory.
impl Recorder for Mutex<Vec<Call>> {
    fn record(&self, call: Call) {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(call);
    }
}

/// The library's ledger, which the interface's totals are summed from. A call that cannot be written is told on
/// standard error (never with its prompt) rather than lost silently.
impl Recorder for Library {
    fn record(&self, call: Call) {
        if let Err(error) = self.record_call(&ledger_call(&call)) {
            eprintln!("constellate: a call to {} for {} could not be recorded: {error}", call.model, call.purpose);
        }
    }

    /// The answer and its cost in one transaction of this library, which is the one the answer is kept in.
    fn keep_paid(
        &self,
        _library: &Library,
        slot: &crate::library::Slot,
        model: &str,
        prompt: u32,
        content: &str,
        calls: Vec<Call>,
    ) -> Result<(), crate::library::LibraryError> {
        let ledger: Vec<_> = calls.iter().map(ledger_call).collect();
        let kept = self.keep_paid(slot, model, prompt, content, &ledger);
        if kept.is_err() {
            // the answer could not be kept: its cost is still recorded, as a call paid for either way, and with it
            // the intent goes, since the ledger then says what it cost
            let recorded = Library::record_paid(self, slot, &ledger).is_ok();
            drop(ledger);
            if !recorded {
                for call in calls {
                    self.record(call);
                }
            }
        }
        kept
    }

    fn record_paid(
        &self,
        _library: &Library,
        slot: &crate::library::Slot,
        calls: Vec<Call>,
    ) -> Result<(), crate::library::LibraryError> {
        let ledger: Vec<_> = calls.iter().map(ledger_call).collect();
        Library::record_paid(self, slot, &ledger)
    }
}

/// The provider cannot be used at all: asking again will not help until something is changed. The variants without
/// a variable named are the Claude API's own words (`ANTHROPIC_*`); [`Setup`] names the variable of any provider.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unavailable {
    #[error("No API key is set (ANTHROPIC_API_KEY)")]
    NoKey,
    #[error("ANTHROPIC_BASE_URL is not an https address")]
    BadEndpoint,
    #[error("The API key was refused")]
    KeyRefused,
    #[error("This API key is not allowed to do this")]
    NotAllowed,
    #[error("The provider's account has a billing problem")]
    Billing,
    #[error("The provider's account has reached its spend limit")]
    SpendLimit,
    #[error("The model {0} is not available to this account")]
    UnknownModel(String),
    #[error("ANTHROPIC_BASE_URL does not lead to the Claude API")]
    NoSuchEndpoint,
    /// What is missing or wrong in how a provider other than the Claude API is set up.
    #[error(transparent)]
    Setup(#[from] Setup),
    /// The settings that say which provider to ask cannot be read (a damaged or hand-edited file): no provider is
    /// asked rather than one guessed.
    #[error(
        "The settings file (settings.json in the data folder) cannot be read, so which provider to ask is not known: \
         fix the file or remove it"
    )]
    SettingsUnreadable,
}

/// What is missing or wrong in a provider's variables, naming the variable (never its value).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Setup {
    #[error("No API key is set ({0})")]
    KeyNotSet(&'static str),
    #[error("No address is set ({0}): the address of an OpenAI-compatible API, such as http://localhost:11434/v1")]
    AddressNotSet(&'static str),
    #[error("{0} is not an https address (plain http is allowed only to this computer)")]
    NotHttps(&'static str),
    #[error("{0} does not lead to an API the engine can speak")]
    NoSuchEndpoint(&'static str),
    #[error("This provider cannot count tokens: they are estimated from characters")]
    CannotCount,
}

/// This request gave nothing usable; the next one may work.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoAnswer {
    #[error("The language model's API is limiting requests; try again later")]
    RateLimited { retry_after: Option<Duration> },
    #[error("The language model's API is overloaded; try again later")]
    Overloaded,
    #[error("The language model's API failed ({0})")]
    ServerError(u16),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("The request is too large for the language model's API")]
    TooLarge,
    #[error("The language model's API refused the request: {0}")]
    Rejected(String),
    #[error("The model declined to answer")]
    Refused { category: Option<String> },
    #[error("The answer was cut off before it was finished")]
    Cut,
    #[error("The answer was empty")]
    Empty,
    #[error("The answer was not the JSON asked for: {0}")]
    NotJson(String),
    #[error("The language model's API answered in a way this does not understand")]
    Unexpected,
}

/// A call not started, because it could take the batch past its budget.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OverBudget {
    #[error("This would take {needed} tokens more than the {limit} allowed ({spent} spent)")]
    Tokens { limit: u64, spent: u64, needed: u64 },
    #[error("This could cost more than the budget allows")]
    Dollars { limit: Usd, spent: Usd, needed: Usd },
    #[error("The price of {0} is not known, so a budget in dollars cannot be kept: set a budget in tokens instead")]
    Unpriced(String),
}

/// Why [`Client::ask`] gave no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AskError {
    #[error(transparent)]
    Unavailable(#[from] Unavailable),
    #[error(transparent)]
    NoAnswer(#[from] NoAnswer),
    #[error(transparent)]
    OverBudget(#[from] OverBudget),
}

/// The most a batch may spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// Tokens of every kind, prompt and answer.
    Tokens(u64),
    /// US dollars, at the prices built into the engine.
    Dollars(Usd),
}

#[derive(Default)]
struct Spending {
    tokens: u64,
    usd: Usd,
    /// What calls under way may still take at worst.
    held_tokens: u64,
    held_usd: Usd,
}

/// What a batch of calls may spend, shared by its calls (from any thread). A call holds its worst case before it
/// starts ([`Budget::reserve`]) and is refused if that would pass the limit; when it ends, what it really took
/// replaces what it held ([`Hold::settle`]), or, when nothing was billed, what it held is given back.
pub struct Budget {
    limit: Limit,
    spending: Mutex<Spending>,
}

/// A call's worst case, held against a budget while it is under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    tokens: u64,
    usd: Usd,
}

/// A call's worst case held against a [`Budget`] while the call is under way: what [`Budget::reserve`] hands back.
///
/// It cannot be forgotten. It is either settled at what the call took ([`Hold::settle`], which consumes it), given
/// back ([`Hold::release`], the same as dropping it), or dropped on any other way out: an early return, an error, a
/// panic unwinding through the call. Each happens exactly once, so a call that ends in any way never leaves what it
/// held on the budget, where it would refuse every later call of the batch.
#[must_use = "a hold dropped at once gives back what it held: settle it with what the call took"]
pub struct Hold<'b> {
    budget: &'b Budget,
    /// `None` once settled.
    held: Option<Held>,
}

impl fmt::Debug for Hold<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Hold").field("held", &self.held).finish()
    }
}

impl Hold<'_> {
    /// The call took `took` and cost `cost` (`None` when its price is not known): that replaces what was held.
    pub fn settle(mut self, took: &Tokens, cost: Option<Usd>) {
        if let Some(held) = self.held.take() {
            self.budget.settle(held, took, cost);
        }
    }

    /// Nothing was billed: what was held is given back.
    pub fn release(mut self) {
        if let Some(held) = self.held.take() {
            self.budget.give_back(held);
        }
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            self.budget.give_back(held);
        }
    }
}

impl fmt::Debug for Budget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (tokens, usd) = self.spent();
        formatter.debug_struct("Budget").field("limit", &self.limit).field("spent", &(tokens, usd)).finish()
    }
}

impl Budget {
    pub fn new(limit: Limit) -> Self {
        Self { limit, spending: Mutex::new(Spending::default()) }
    }

    /// What it is held to.
    pub fn limit(&self) -> Limit {
        self.limit
    }

    /// What the batch's calls have taken so far: tokens, and dollars (of priced calls).
    pub fn spent(&self) -> (u64, Usd) {
        let spending = self.lock();
        (spending.tokens, spending.usd)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Spending> {
        self.spending.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Tokens spent and held, read together.
    #[cfg(test)]
    fn taken(&self) -> (u64, u64) {
        let spending = self.lock();
        (spending.tokens, spending.held_tokens)
    }

    /// What calls under way hold at present: tokens, and dollars. Nothing once every call has ended.
    pub fn held(&self) -> (u64, Usd) {
        let spending = self.lock();
        (spending.held_tokens, spending.held_usd)
    }

    /// Hold the worst case of a call, `worst`, that may be billed by `models` (price keys: the model asked, then
    /// every model the provider may hand the request to, [`answered_by`]), counted as [`worst_of`] says: refused
    /// when that would take spent and held together past the limit. A budget in dollars refuses a call any of whose
    /// models has no known price, rather than guess what it would cost.
    pub fn reserve<S: AsRef<str>>(&self, models: &[S], worst: &Tokens) -> Result<Hold<'_>, OverBudget> {
        let (most_tokens, most_usd) = worst_of(models, worst);
        let mut spending = self.lock();
        let held = match self.limit {
            Limit::Tokens(limit) => {
                let needed = most_tokens;
                let taken = spending.tokens.saturating_add(spending.held_tokens);
                if taken.saturating_add(needed) > limit {
                    return Err(OverBudget::Tokens { limit, spent: spending.tokens, needed });
                }
                Held { tokens: needed, usd: Usd::ZERO }
            }
            Limit::Dollars(limit) => {
                let needed = most_usd.map_err(OverBudget::Unpriced)?;
                if spending.usd + spending.held_usd + needed > limit {
                    return Err(OverBudget::Dollars { limit, spent: spending.usd, needed });
                }
                Held { tokens: 0, usd: needed }
            }
        };
        spending.held_tokens = spending.held_tokens.saturating_add(held.tokens);
        spending.held_usd =
            Usd::from_picodollars(spending.held_usd.picodollars().saturating_add(held.usd.picodollars()));
        Ok(Hold { budget: self, held: Some(held) })
    }

    fn unhold(spending: &mut Spending, held: Held) {
        spending.held_tokens = spending.held_tokens.saturating_sub(held.tokens);
        spending.held_usd =
            Usd::from_picodollars(spending.held_usd.picodollars().saturating_sub(held.usd.picodollars()));
    }

    fn give_back(&self, held: Held) {
        Self::unhold(&mut self.lock(), held);
    }

    fn settle(&self, held: Held, took: &Tokens, cost: Option<Usd>) {
        let mut spending = self.lock();
        Self::unhold(&mut spending, held);
        spending.tokens = spending.tokens.saturating_add(took.all());
        // a call whose price is not known is counted at what it held, so that a budget in dollars still sees it
        spending.usd += cost.unwrap_or(held.usd);
    }
}

/// How tokens are reckoned when they have not been counted: from characters, at a ratio assumed or measured.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Counting {
    /// [`CHARS_PER_TOKEN`], assumed: an estimate that errs high.
    Assumed { chars_per_token: f64 },
    /// Measured on one request with the API's token-counting endpoint, and applied to the rest.
    Measured { chars_per_token: f64 },
}

impl Counting {
    pub const ASSUMED: Self = Self::Assumed { chars_per_token: CHARS_PER_TOKEN };

    fn chars_per_token(self) -> f64 {
        match self {
            Self::Assumed { chars_per_token } | Self::Measured { chars_per_token } => chars_per_token.max(0.5),
        }
    }

    fn tokens(self, chars: u64) -> u64 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let tokens = (chars as f64 / self.chars_per_token()).ceil() as u64;
        tokens
    }
}

/// What a call is planned to be, for an estimate before any is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The system prompt and the shared part, in characters: cached.
    pub shared_chars: u64,
    pub question_chars: u64,
    /// Questions asked over the same shared part: the first writes it to the cache, the rest read it.
    pub questions: u32,
    /// The answer's likely length, thinking included.
    pub answer_tokens: u32,
    /// The most an answer may take.
    pub max_tokens: u32,
}

/// What asking would cost, before anything is sent: what the interface shows as "about N dollars" (or, for a
/// model whose price is not known, as so many tokens at most). Every feature's estimate is one of these
/// (`AskingCost` in the contracts), so that each says the same things: which provider and model, how tokens were
/// reckoned and the prefix counted, and the worst case in tokens a budget can be held to when dollars cannot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Estimate {
    pub provider: Provider,
    pub model: String,
    pub calls: u64,
    /// Likely tokens, by how they are billed.
    pub tokens: Tokens,
    /// Likely cost; `None` when the model's price is not known (or it runs on this computer).
    pub usd: Option<Usd>,
    /// If every answer took `max_tokens` and nothing was read from the cache.
    pub most_usd: Option<Usd>,
    /// Tokens of every kind in that worst case: what a budget in tokens is held to when there is no price.
    pub most_tokens: u64,
    /// How the prompt's tokens were reckoned: it is an estimate either way.
    pub counting: Counting,
    /// How the shared prefix was counted: written once and read after, or, for a provider that reports no caching
    /// ([`Caching::NotReported`]), paid for in full by every question.
    pub caching: Caching,
    /// The model runs on this computer (a server on loopback): it costs no API fee, and its cost is not in
    /// dollars.
    pub on_this_computer: bool,
}

/// The most characters [`Client::ask`] adds to a question it asks once more, after an answer that did not follow
/// the schema: what was wrong with it, and the request to answer again. Counted in an estimate's worst case.
const RETRY_CHARS: u64 = 600;

/// Where a model's calls are sent, as far as what they cost goes: the provider (whose prices they are billed at),
/// whether the model runs on this computer (which costs no API fee, whatever its id), and how JSON answers are had
/// there (where the schema is written into the question, it is paid for as part of it, and an answer that does not
/// follow it is asked for once more). What an estimate is reckoned by, so that it counts what is really sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pricing {
    pub provider: Provider,
    pub on_this_computer: bool,
    pub answers: Answers,
}

impl Pricing {
    /// How `endpoint` is priced.
    pub fn of(endpoint: &Endpoint) -> Self {
        Self {
            provider: endpoint.provider(),
            on_this_computer: endpoint.on_this_computer(),
            answers: endpoint.capabilities().answers,
        }
    }

    /// How `provider` is priced when its endpoint is not set up (yet): as its row says.
    pub fn listed(provider: Provider) -> Self {
        Self { provider, on_this_computer: false, answers: provider.about().capabilities.answers }
    }

    /// The characters of `ask`'s question as it is sent here: with the schema written into it where the provider
    /// cannot be held to one.
    pub fn question_chars(self, ask: &Ask<'_>) -> u64 {
        let count = |text: &str| u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        match (self.provider, ask.form) {
            (Provider::Anthropic, _) | (_, Form::Text) => count(ask.question),
            (_, Form::Json(schema)) => count(&schema::question(ask.question, schema, self.answers, None)),
        }
    }

    /// What `planned` calls to `model` are likely to cost here, and at most ([`estimate_for`] the provider). A
    /// model on this computer has no dollar figure; where answers are not held to the schema by the provider, the
    /// worst case counts every question asked twice, as the client does when an answer does not follow it.
    pub fn estimate(self, model: &str, planned: &[Planned], counting: Counting) -> Estimate {
        let asked_again = self.provider != Provider::Anthropic && self.answers != Answers::Enforced;
        let estimate = reckon(self.provider, model, planned, counting, asked_again);
        if self.on_this_computer {
            Estimate { usd: None, most_usd: None, on_this_computer: true, ..estimate }
        } else {
            estimate
        }
    }
}

/// What `planned` calls to `model` of `provider` are likely to cost, and at most, at the prices of that provider
/// ([`price_key`]), its prompt caching counted as that provider caches ([`Capabilities::caching`]): marked or
/// written at a price of its own, the first question writes the prefix and the rest read it; cached by itself at no
/// extra price, the first pays for it in full and the rest read it; not reported, every question pays for it in
/// full. The cache is counted as it works when the questions over one shared part are asked one after the other
/// within a few minutes. The most is always every answer at `max_tokens` and nothing read. A feature estimates
/// through [`Pricing::estimate`], which also counts what the provider's way of answering in JSON adds.
pub fn estimate_for(provider: Provider, model: &str, planned: &[Planned], counting: Counting) -> Estimate {
    reckon(provider, model, planned, counting, false)
}

/// [`estimate_for`], with each question's worst case counted twice when `asked_again`.
fn reckon(provider: Provider, model: &str, planned: &[Planned], counting: Counting, asked_again: bool) -> Estimate {
    let priced = price_key(provider, model);
    // the worst case at each model the request may be answered by (a server-side fallback): the most is the
    // dearest of them, so that "at most" is what the budget holds
    let answering = answered_by(provider, model);
    let caching = provider.about().capabilities.caching;
    let mut likely = Tokens::default();
    let mut likely_usd = Some(Usd::ZERO);
    let mut most_usd = Some(Usd::ZERO);
    let mut most_tokens = 0u64;
    let mut calls = 0u64;
    let add = |total: &mut Option<Usd>, tokens: &Tokens| {
        *total = total.and_then(|sum| price_of(&priced, tokens).map(|cost| sum + cost));
    };
    // each call as a budget holds it ([`worst_of`]): unpriced when any model that may bill it has no price
    let add_most = |total: &mut Option<Usd>, most_tokens: &mut u64, tokens: &Tokens| {
        let (all, usd) = worst_of(&answering, tokens);
        *total = total.and_then(|sum| usd.ok().map(|cost| sum + cost));
        *most_tokens = most_tokens.saturating_add(all);
    };
    for plan in planned {
        let shared = counting.tokens(plan.shared_chars);
        let question = counting.tokens(plan.question_chars);
        for asked in 0..plan.questions {
            let first = asked == 0;
            let output = u64::from(plan.answer_tokens);
            let (call, worst) = match caching {
                Caching::Marked | Caching::AutomaticWritesBilled => {
                    let call = Tokens {
                        input: question,
                        output,
                        cache_read: if first { 0 } else { shared },
                        cache_write_5m: if first { shared } else { 0 },
                        cache_write_1h: 0,
                    };
                    let worst =
                        Tokens { output: u64::from(plan.max_tokens), cache_read: 0, cache_write_5m: shared, ..call };
                    (call, worst)
                }
                Caching::Automatic => {
                    let call = Tokens {
                        input: if first { question + shared } else { question },
                        output,
                        cache_read: if first { 0 } else { shared },
                        ..Tokens::default()
                    };
                    let worst = Tokens { input: question + shared, output: u64::from(plan.max_tokens), ..call };
                    (call, Tokens { cache_read: 0, ..worst })
                }
                Caching::NotReported => {
                    let call = Tokens { input: question + shared, output, ..Tokens::default() };
                    (call, Tokens { output: u64::from(plan.max_tokens), ..call })
                }
            };
            add(&mut likely_usd, &call);
            likely = likely + call;
            add_most(&mut most_usd, &mut most_tokens, &worst);
            if asked_again {
                // the second attempt: the same prompt with what was wrong added after the question
                let again = Tokens { input: worst.input + counting.tokens(RETRY_CHARS), ..worst };
                add_most(&mut most_usd, &mut most_tokens, &again);
            }
            calls += 1;
        }
    }
    Estimate {
        provider,
        model: model.to_owned(),
        calls,
        tokens: likely,
        usd: likely_usd,
        most_usd,
        most_tokens,
        counting,
        caching,
        on_this_computer: false,
    }
}

/// The id a model's price is kept under in the prices file, and its calls in the ledger: the Claude API's models by
/// their own id (as they always were), every other provider's as `provider:id`, since two providers may sell the
/// same model under the same id at different prices.
pub fn price_key(provider: Provider, model: &str) -> String {
    match provider {
        Provider::Anthropic => model.to_owned(),
        other => format!("{}:{model}", other.id()),
    }
}

/// What a model's price is, as the interface is told it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Price {
    /// US dollars per million tokens, as published; reading from the cache where that has a price of its own.
    Known { input: f64, output: f64, cache_read: Option<f64> },
    /// Not published, or not known here: shown as "cost not known", never as nothing.
    NotKnown,
    /// A server on this computer: there is no fee for the API.
    ThisComputer,
}

/// The price of `model` at `provider`, as the interface shows it: a server on this computer costs no fee.
pub fn listed_price(provider: Provider, model: &str, on_this_computer: bool) -> Price {
    if on_this_computer {
        return Price::ThisComputer;
    }
    let per_million = |picodollars: u64| {
        #[allow(clippy::cast_precision_loss)]
        let dollars = picodollars as f64 / 1e6;
        dollars
    };
    match crate::usage::model_price(&price_key(provider, model)) {
        Some(price) => Price::Known {
            input: per_million(price.rate.input),
            output: per_million(price.rate.output),
            cache_read: price.rate.cache.map(|cache| per_million(cache.read)),
        },
        None => Price::NotKnown,
    }
}

/// An answer, with what it took.
#[derive(Clone, Debug, PartialEq)]
pub struct Answered<T> {
    pub answer: T,
    /// The model that answered, as the API named it (another one when a fallback answered).
    pub model: String,
    pub tokens: Tokens,
    /// `None` when the model's price is not known.
    pub cost: Option<Usd>,
}

/// An [`Ask`] as it is sent to a provider other than the Claude API: the question as worded for it, and the effort
/// the model has nearest to the one asked for (none for a model without one).
pub(super) struct Asked<'a> {
    pub(super) ask: Ask<'a>,
    pub(super) question: String,
    pub(super) effort: Option<Effort>,
    pub(super) answers: Answers,
}

/// A request in a provider's shapes: where under the endpoint it goes, and its body.
pub(super) struct Request {
    pub(super) path: String,
    pub(super) body: Value,
}

/// An answer as read in a provider's shapes, before anything of it is used.
pub(super) struct Read {
    /// The model that answered, as the provider named it.
    pub(super) model: Option<String>,
    /// What it took, by how it is billed; `None` when the server did not say.
    pub(super) tokens: Option<Tokens>,
    pub(super) outcome: Outcome,
}

/// What an answer is, told before its content is read.
pub(super) enum Outcome {
    Text(String),
    /// The model or the provider's filters declined, for this reason when one was given.
    Refused(Option<String>),
    /// Cut off at the most tokens allowed.
    Cut,
    /// The provider said, in a successful reply, that it failed.
    Failed(AskError),
}

/// The models an OpenAI-compatible server says it has (`GET <address>/models`), for the user to pick from rather
/// than type: what a server on this computer has downloaded. Only the compatible endpoint is asked; listing models
/// costs nothing, and nothing is recorded.
pub fn discover(transport: &dyn Transport, endpoint: &Endpoint) -> Result<Vec<String>, AskError> {
    if endpoint.provider != Provider::Compatible {
        return Ok(endpoint.provider.about().models.iter().map(|model| model.id.to_owned()).collect());
    }
    let reply = transport
        .get(&endpoint.url("/models"), &endpoint.key, endpoint.key_header(), Duration::from_secs(15))
        .map_err(NoAnswer::from)?;
    if !(200..300).contains(&reply.status) {
        return Err(chat::openai_style_failure(&reply, "", Provider::Compatible));
    }
    chat::read_models(&reply.body)
        .ok_or_else(|| Unavailable::Setup(Setup::NoSuchEndpoint("CONSTELLATE_MODEL_BASE_URL")).into())
}

/// Whether `answered` is `asked` itself, or `asked` with a date or version after a hyphen (`-2026-11-01`,
/// `-20261101`, `-001`): digits and hyphens only, so that `gpt-6.2-sol-pro` is not taken for `gpt-6.2-sol`.
fn dated_of(answered: &str, asked: &str) -> bool {
    match answered.strip_prefix(asked) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit() || b == b'-')),
        None => false,
    }
}

/// How long a reply's `retry-after` header says to wait. A header that is negative, not a number, or absurd is no
/// wait at all (and must not panic); a long one is kept just past [`LONGEST_WAIT`], so that it is not waited for.
fn retry_after_header(reply: &Reply) -> Option<Duration> {
    reply
        .retry_after
        .as_deref()
        .and_then(|seconds| seconds.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds.min(LONGEST_WAIT.as_secs_f64() + 1.0)).ok())
}

/// The most characters of a provider's own error message shown ([`NoAnswer::Rejected`]).
const MESSAGE_CHARS: usize = 300;
/// The most read of it before it is [`scrubbed`]: more than is shown, so that a key that runs past what is shown
/// is still seen whole, and hidden, before the message is cut.
const RAW_MESSAGE_CHARS: usize = 4_000;
/// What stands in for words of a message that are not shown.
const HIDDEN: &str = "[hidden]";
/// The shapes of keys the repository's hooks refuse to commit, one regular expression a line: what a message from a
/// provider (or a gateway in front of it) must not echo to the interface either.
const KEY_SHAPES_TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.githooks/secrets.txt"));
static KEY_SHAPES: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
    KEY_SHAPES_TEXT
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| regex::Regex::new(line).ok())
        .collect()
});
/// Where a message that echoes the request starts to carry its headers: it is cut there.
const HEADER_NAMES: [&str; 6] = ["authorization", "x-api-key", "x-goog-api-key", "bearer ", "cookie", "proxy-"];

/// A provider's error message made safe to show: control characters made spaces; the endpoint's own key, anything
/// shaped like a key ([`KEY_SHAPES`]) and any long unbroken run of token characters hidden; cut where it starts to
/// echo request headers ([`HEADER_NAMES`]); and at most [`MESSAGE_CHARS`] characters. An arbitrary gateway's
/// message is shown, so it is never trusted to leave a secret out by itself.
fn scrubbed(message: &str, key: &ApiKey) -> String {
    let mut text: String = message.chars().map(|letter| if letter.is_control() { ' ' } else { letter }).collect();
    if !key.0.is_empty() {
        text = text.replace(&key.0, HIDDEN);
    }
    for shape in KEY_SHAPES.iter() {
        // a shape may begin with the character before the key (`(^|[^A-Za-z0-9_-])sk-...`): that one stays
        text = shape
            .replace_all(&text, |found: &regex::Captures<'_>| {
                let before: String = found[0].chars().take_while(|letter| !letter.is_ascii_alphanumeric()).collect();
                format!("{before}{HIDDEN}")
            })
            .into_owned();
    }
    // ASCII lower-casing keeps every byte where it was, so a place found in it is a place in `text`
    let lower = text.to_ascii_lowercase();
    if let Some(at) = HEADER_NAMES.iter().filter_map(|name| lower.find(name)).min() {
        text.truncate(at);
        text.push_str("[the rest is not shown]");
    }
    let token = |letter: char| letter.is_ascii_alphanumeric() || "-_+/=.".contains(letter);
    let words: Vec<String> =
        text.split(' ')
            .map(|word| {
                if word.chars().count() >= 32 && word.chars().all(token) { HIDDEN.to_owned() } else { word.to_owned() }
            })
            .collect();
    let joined = words.join(" ");
    let mut shown: String = joined.trim().chars().take(MESSAGE_CHARS).collect();
    if joined.trim().chars().count() > MESSAGE_CHARS {
        shown.push('…');
    }
    shown
}

/// Is this worth asking again, and after how long?
fn retry_after(error: &AskError, reply_says: Option<&str>, attempt: u32) -> Option<Duration> {
    if reply_says == Some("false") {
        return None;
    }
    let backoff = Duration::from_secs(2u64.saturating_pow(attempt));
    match error {
        AskError::NoAnswer(NoAnswer::RateLimited { retry_after }) => {
            let wait = retry_after.unwrap_or(backoff);
            (wait <= LONGEST_WAIT).then_some(wait)
        }
        // stopped; or sent and perhaps answered and billed, which a second request could pay for twice
        AskError::NoAnswer(NoAnswer::Transport(TransportError::Cancelled | TransportError::Lost)) => None,
        AskError::NoAnswer(NoAnswer::Overloaded | NoAnswer::ServerError(_) | NoAnswer::Transport(_)) => Some(backoff),
        _ => None,
    }
}

/// A client of the Claude API: the endpoint, the way to it, and where its calls are recorded.
pub struct Client<'a> {
    transport: &'a dyn Transport,
    endpoint: Endpoint,
    recorder: &'a dyn Recorder,
    counting: Mutex<Counting>,
    /// Set once the API said it cannot be used: no request is sent after that.
    stopped: Mutex<Option<Unavailable>>,
}

impl<'a> Client<'a> {
    pub fn new(transport: &'a dyn Transport, endpoint: Endpoint, recorder: &'a dyn Recorder) -> Self {
        Self { transport, endpoint, recorder, counting: Mutex::new(Counting::ASSUMED), stopped: Mutex::new(None) }
    }

    /// How prompt tokens are reckoned before a call: assumed, or measured by [`Client::calibrate`].
    pub fn counting(&self) -> Counting {
        *self.counting.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn headers(&self, model: &str, fallback: bool) -> Vec<(&'static str, &'static str)> {
        let mut headers = vec![("anthropic-version", API_VERSION)];
        if fallback && self::model(model).is_some_and(|model| model.fallback == Fallback::ServerSide) {
            headers.push(("anthropic-beta", FALLBACK_BETA));
        }
        headers
    }

    fn stopped(&self) -> Result<(), Unavailable> {
        match &*self.stopped.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            Some(why) => Err(why.clone()),
            None => Ok(()),
        }
    }

    fn stop(&self, error: &AskError) {
        if let AskError::Unavailable(why) = error {
            // another model may still be available: only what is true of every request stops them all
            if !matches!(why, Unavailable::UnknownModel(_)) {
                *self.stopped.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why.clone());
            }
        }
    }

    /// Send `body` to `path`, trying again as a passing failure allows.
    fn send(&self, path: &str, model: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Reply, AskError> {
        self.stopped()?;
        let url = self.endpoint.url(path);
        let (key, sent_as) = (&self.endpoint.key, self.endpoint.key_header());
        let mut attempt = 0;
        loop {
            let (error, says) = match self.transport.post_keyed(&url, key, sent_as, headers, body, PATIENCE) {
                Ok(reply) if (200..300).contains(&reply.status) => return Ok(reply),
                Ok(reply) => (self.failure(&reply, model), reply.should_retry),
                Err(error) => (NoAnswer::from(error).into(), None),
            };
            self.stop(&error);
            match retry_after(&error, says.as_deref(), attempt).filter(|_| attempt < RETRIES) {
                Some(wait) => {
                    self.transport.wait(wait);
                    attempt += 1;
                }
                None => return Err(error),
            }
        }
    }

    /// What a request that failed with `reply.status` means, in the words of the provider's API.
    fn failure(&self, reply: &Reply, model: &str) -> AskError {
        let provider = self.endpoint.provider;
        let error = match provider.about().api {
            Api::Messages => failure(reply, model),
            Api::Responses => responses::failure(reply, model, provider),
            Api::GenerateContent => gemini::failure(reply, model),
            Api::ChatCompletions(_) => chat::openai_style_failure(reply, model, provider),
        };
        self.scrubbed(error)
    }

    /// `error` with the provider's own words in it made safe to show ([`scrubbed`]).
    fn scrubbed(&self, error: AskError) -> AskError {
        match error {
            AskError::NoAnswer(NoAnswer::Rejected(message)) => {
                NoAnswer::Rejected(scrubbed(&message, &self.endpoint.key)).into()
            }
            other => other,
        }
    }

    /// The worst a call can take: its prompt (estimated from characters) all written to the cache, and an answer
    /// of `max_tokens`.
    fn worst(&self, ask: &Ask<'_>) -> Tokens {
        let counting = self.counting();
        let (shared, question) = ask.characters();
        Tokens {
            input: counting.tokens(question),
            output: u64::from(ask.max_tokens),
            cache_write_5m: counting.tokens(shared),
            ..Tokens::default()
        }
    }

    /// Record what the answer says it took, by the model that took it.
    fn record(&self, purpose: &str, asked: &str, message: &Message) -> (String, Tokens, Option<Usd>) {
        let usage = message.usage.clone();
        let answered = message.model.clone().unwrap_or_else(|| asked.to_owned());
        let parts: Vec<Usage> = usage
            .as_ref()
            .and_then(|usage| usage.iterations.clone())
            .map(|parts| parts.into_iter().collect())
            .unwrap_or_default();
        let parts: Vec<(String, Tokens)> = if parts.is_empty() {
            vec![(answered.clone(), usage.map(|usage| usage.tokens()).unwrap_or_default())]
        } else {
            parts
                .into_iter()
                .map(|part| (part.model.clone().unwrap_or_else(|| answered.clone()), part.tokens()))
                .collect()
        };
        let mut total = Tokens::default();
        let mut cost = Some(Usd::ZERO);
        for (model, tokens) in parts {
            let call = Call { model, purpose: purpose.to_owned(), tokens };
            cost = cost.and_then(|sum| call.cost().map(|one| sum + one));
            total = total + tokens;
            self.recorder.record(call);
        }
        (answered, total, cost)
    }

    /// Ask, and get the answer's text: JSON when the form is [`Form::Json`]. Refused before anything is sent when
    /// the call's worst case would take `budget` past its limit.
    ///
    /// From a provider that does not guarantee an answer follows the schema, the JSON is checked against it here,
    /// and asked for once more, with what was wrong, when it does not: then [`NoAnswer::NotJson`]. The answer is the
    /// JSON alone (without a fence or words around it). Each attempt is recorded and held against `budget`.
    pub fn ask(&self, ask: &Ask<'_>, budget: &Budget) -> Result<Answered<String>, AskError> {
        if self.endpoint.provider != Provider::Anthropic {
            return self.ask_provider(ask, budget);
        }
        self.stopped()?;
        // held at the dearest model that may answer: a fallback may cost more than the model asked
        let answering = answered_by(Provider::Anthropic, ask.model);
        let answering: Vec<&str> = answering.iter().map(String::as_str).collect();
        let held = budget.reserve(&answering, &self.worst(ask))?;
        let mut body = ask.body();
        body["max_tokens"] = json!(ask.max_tokens);
        if falls_back(Provider::Anthropic, ask.model) {
            body["fallbacks"] = json!("default");
        }
        let headers = self.headers(ask.model, true);
        let sent = self.send("/v1/messages", ask.model, &headers, body.to_string().as_bytes());
        let reply = match sent {
            Ok(reply) => reply,
            Err(error) => {
                held.release(); // a failed request is not billed
                return Err(error);
            }
        };
        let Ok(message) = serde_json::from_slice::<Message>(&reply.body) else {
            // an answer was given and so probably billed, but says nothing of what it took: its worst case is
            // recorded and counted, rather than nothing
            let worst = self.worst(ask);
            let call = Call { model: ask.model.to_owned(), purpose: ask.purpose.to_owned(), tokens: worst };
            held.settle(&worst, call.cost());
            self.recorder.record(call);
            return Err(NoAnswer::Unexpected.into());
        };
        // paid for whether or not the answer is usable: recorded before it is looked at
        let (answered, tokens, cost) = match message.usage {
            Some(_) => self.record(ask.purpose, ask.model, &message),
            // an answer that says nothing of what it took (a gateway that drops the usage) is counted at its worst
            // case, as one in no shape known is, not as nothing: its thinking cannot be counted from its text
            None => {
                let worst = self.worst(ask);
                let model = message.model.clone().unwrap_or_else(|| ask.model.to_owned());
                let call = Call { model: model.clone(), purpose: ask.purpose.to_owned(), tokens: worst };
                let cost = call.cost();
                self.recorder.record(call);
                (model, worst, cost)
            }
        };
        held.settle(&tokens, cost);
        match message.stop_reason.as_deref() {
            Some("refusal") => {
                let category = message.stop_details.and_then(|details| details.category);
                return Err(NoAnswer::Refused { category }.into());
            }
            // the answer ran into the model's context window rather than `max_tokens`: cut off all the same
            Some("max_tokens" | "model_context_window_exceeded") => return Err(NoAnswer::Cut.into()),
            _ => {}
        }
        let text: String = message
            .content
            .into_iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(text),
                Block::Other => None,
            })
            .collect::<String>()
            .trim()
            .to_owned();
        if text.is_empty() {
            return Err(NoAnswer::Empty.into());
        }
        Ok(Answered { answer: text, model: answered, tokens, cost })
    }

    /// Ask for JSON and read it as `T`.
    pub fn ask_json<T: DeserializeOwned>(&self, ask: &Ask<'_>, budget: &Budget) -> Result<Answered<T>, AskError> {
        let Answered { answer, model, tokens, cost } = self.ask(ask, budget)?;
        let answer = serde_json::from_str(&answer).map_err(|error| NoAnswer::NotJson(error.to_string()))?;
        Ok(Answered { answer, model, tokens, cost })
    }

    /// What is sent for `ask` to a provider other than the Claude API: the question with the schema written into
    /// it where the provider cannot be held to one, and on a second attempt what was wrong with the first answer.
    fn asked<'b>(&self, ask: &Ask<'b>, wrong: Option<&str>) -> Asked<'b> {
        let capabilities = self.endpoint.capabilities();
        let question = match ask.form {
            Form::Json(schema) => schema::question(ask.question, schema, capabilities.answers, wrong),
            Form::Text => ask.question.to_owned(),
        };
        let effort = match capabilities.effort {
            EffortControl::ByModel => {
                self.endpoint.provider.about().model(ask.model).and_then(|model| effort_for(ask.effort, model.efforts))
            }
            EffortControl::NotSent => None,
        };
        Asked { ask: *ask, question, effort, answers: capabilities.answers }
    }

    /// The request for `asked`, in the provider's shapes.
    fn request(&self, asked: &Asked<'_>) -> Request {
        match self.endpoint.provider.about().api {
            // the Claude API's requests are made by `ask` itself, as they always were
            Api::Messages => Request { path: "/v1/messages".to_owned(), body: asked.ask.body() },
            Api::Responses => responses::request(asked),
            Api::GenerateContent => gemini::request(asked),
            Api::ChatCompletions(chat) => chat::request(asked, chat),
        }
    }

    /// What an answer says, in the provider's shapes; `None` when it is not shaped as one.
    fn read(&self, body: &[u8], model: &str) -> Option<Read> {
        let provider = self.endpoint.provider;
        match provider.about().api {
            Api::Messages => None,
            Api::Responses => responses::read(body),
            Api::GenerateContent => gemini::read(body),
            Api::ChatCompletions(_) => chat::read(body, model, provider),
        }
    }

    /// The worst `asked` can take. The prefix is counted as written to the cache where writing it has a price of
    /// its own, and as plain input everywhere else (where an unpriced cache would make the worst case unpriced).
    fn worst_asked(&self, asked: &Asked<'_>) -> Tokens {
        let counting = self.counting();
        let chars = |text: &str| u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        // the prefix counted as one text, as an estimate counts it: two parts rounded up apart would be a token
        // more than the estimate's worst case, and a budget of exactly that worst case would refuse the call
        let prefix = counting.tokens(chars(asked.ask.system) + asked.ask.shared.map_or(0, chars));
        let question = counting.tokens(chars(&asked.question));
        let output = u64::from(asked.ask.max_tokens);
        match self.endpoint.provider.about().capabilities.caching {
            Caching::Marked | Caching::AutomaticWritesBilled => {
                Tokens { input: question, output, cache_write_5m: prefix, ..Tokens::default() }
            }
            Caching::Automatic | Caching::NotReported => {
                Tokens { input: question + prefix, output, ..Tokens::default() }
            }
        }
    }

    /// The most [`Client::ask`] can spend on `ask`, every attempt counted as the budget holds it: tokens of every
    /// kind, and US dollars (`None` when a model that may bill it has no known price). What is written down before
    /// the call is sent (`library::asking`), as what it may have cost should its outcome be lost.
    pub fn most(&self, ask: &Ask<'_>) -> (u64, Option<Usd>) {
        let provider = self.endpoint.provider;
        if provider == Provider::Anthropic {
            let (tokens, usd) = worst_of(&answered_by(provider, ask.model), &self.worst(ask));
            return (tokens, usd.ok());
        }
        let priced = [price_key(provider, ask.model)];
        let first = self.worst_asked(&self.asked(ask, None));
        let (mut tokens, mut usd) = worst_of(&priced, &first);
        let asked_again =
            matches!(ask.form, Form::Json(_)) && self.endpoint.capabilities().answers != Answers::Enforced;
        if asked_again {
            // an answer that did not follow the schema is asked for once more, with what was wrong after the question
            let again = Tokens { input: first.input + self.counting().tokens(RETRY_CHARS), ..first };
            let (more_tokens, more_usd) = worst_of(&priced, &again);
            tokens = tokens.saturating_add(more_tokens);
            usd = usd.and_then(|usd| more_usd.map(|more| usd + more));
        }
        (tokens, usd.ok())
    }

    /// [`Client::ask`] for any provider but the Claude API.
    fn ask_provider(&self, ask: &Ask<'_>, budget: &Budget) -> Result<Answered<String>, AskError> {
        let provider = self.endpoint.provider;
        // a model id goes into a URL path (Gemini) and the ledger: only a plain one is sent anywhere
        if !provider::plain_id(ask.model) {
            return Err(Unavailable::UnknownModel(ask.model.to_owned()).into());
        }
        let priced = price_key(provider, ask.model);
        let mut wrong: Option<String> = None;
        loop {
            self.stopped()?;
            let asked = self.asked(ask, wrong.as_deref());
            let worst = self.worst_asked(&asked);
            let held = match (budget.reserve(&[priced.as_str()], &worst), &wrong) {
                (Ok(held), _) => held,
                // the first attempt was paid for: this is an answer that was not the JSON asked for, not a call
                // that was never started, which a batch would start again from the beginning (and pay for again)
                (Err(_), Some(why)) => return Err(NoAnswer::NotJson(why.clone()).into()),
                (Err(over), None) => return Err(over.into()),
            };
            let Request { path, body } = self.request(&asked);
            let reply = match self.send(&path, ask.model, &[], body.to_string().as_bytes()) {
                Ok(reply) => reply,
                Err(error) => {
                    held.release(); // a failed request is not billed
                    return Err(error);
                }
            };
            let Some(read) = self.read(&reply.body, ask.model) else {
                // answered, and so probably billed, but in no shape this knows: its worst case is recorded
                let call = Call { model: priced.clone(), purpose: ask.purpose.to_owned(), tokens: worst };
                held.settle(&worst, call.cost());
                self.recorder.record(call);
                return Err(NoAnswer::Unexpected.into());
            };
            let answered = read.model.clone().unwrap_or_else(|| ask.model.to_owned());
            // a server that reports no usage (some local ones) is counted at the worst case the request allows, as
            // the Claude API's answer without usage is: the answer's characters would leave out hidden thinking,
            // which a budget in tokens must count
            let tokens = read.tokens.unwrap_or(worst);
            // a dated id of the model asked for (gpt-6.2-sol-2026-11-01) has its price; another model has its own
            let recorded = if dated_of(&answered, ask.model) { ask.model } else { answered.as_str() };
            let call = Call { model: price_key(provider, recorded), purpose: ask.purpose.to_owned(), tokens };
            let cost = call.cost();
            // paid for whether or not the answer is usable: recorded before it is looked at
            held.settle(&tokens, cost);
            self.recorder.record(call);
            let text = match read.outcome {
                Outcome::Text(text) => text,
                Outcome::Refused(category) => return Err(NoAnswer::Refused { category }.into()),
                Outcome::Cut => return Err(NoAnswer::Cut.into()),
                Outcome::Failed(error) => {
                    self.stop(&error);
                    return Err(self.scrubbed(error));
                }
            };
            if text.is_empty() {
                return Err(NoAnswer::Empty.into());
            }
            let Form::Json(schema) = ask.form else {
                return Ok(Answered { answer: text, model: answered, tokens, cost });
            };
            let why = match schema::json_in(&text) {
                Some((json, value)) => match schema::check(&value, schema) {
                    Ok(()) => return Ok(Answered { answer: json, model: answered, tokens, cost }),
                    Err(why) => why,
                },
                None => "it is not JSON".to_owned(),
            };
            if wrong.is_some() {
                return Err(NoAnswer::NotJson(why).into());
            }
            wrong = Some(why);
        }
    }

    /// The prompt tokens of `ask`, counted by the API's token-counting endpoint (no answer is written). A provider
    /// with no such endpoint ([`TokenCount::Characters`]) says so: [`Setup::CannotCount`].
    pub fn count_tokens(&self, ask: &Ask<'_>) -> Result<u64, AskError> {
        #[derive(Deserialize)]
        struct Counted {
            input_tokens: u64,
        }
        if self.endpoint.provider != Provider::Anthropic {
            if self.endpoint.capabilities().counting == TokenCount::Characters {
                return Err(Unavailable::Setup(Setup::CannotCount).into());
            }
            let asked = Asked { effort: None, ..self.asked(ask, None) };
            let gemini = self.endpoint.provider.about().api == Api::GenerateContent;
            let Request { path, body } =
                if gemini { gemini::count_request(&asked) } else { responses::count_request(&asked) };
            let reply = self.send(&path, ask.model, &[], body.to_string().as_bytes())?;
            let counted = if gemini { gemini::read_count(&reply.body) } else { responses::read_count(&reply.body) };
            return counted.ok_or_else(|| NoAnswer::Unexpected.into());
        }
        let headers = self.headers(ask.model, false);
        let reply = self.send("/v1/messages/count_tokens", ask.model, &headers, ask.body().to_string().as_bytes())?;
        let counted: Counted = serde_json::from_slice(&reply.body).map_err(|_| NoAnswer::Unexpected)?;
        Ok(counted.input_tokens)
    }

    /// Measure how many characters a token is for prompts like `ask`, with one token count, and reckon later
    /// prompts with it: what makes [`estimate_for`] and the budget's worst cases closer to what calls will cost.
    ///
    /// A provider that cannot count tokens is not asked: the counting stays [`Counting::Assumed`], which says that
    /// what is reckoned with it is an estimate from characters.
    pub fn calibrate(&self, ask: &Ask<'_>) -> Result<Counting, AskError> {
        if self.endpoint.capabilities().counting == TokenCount::Characters {
            return Ok(self.counting());
        }
        let tokens = self.count_tokens(ask)?.max(1);
        let (shared, question) = ask.characters();
        #[allow(clippy::cast_precision_loss)]
        let measured = Counting::Measured { chars_per_token: (shared + question) as f64 / tokens as f64 };
        *self.counting.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = measured;
        Ok(measured)
    }
}

/// Answers served in order, for the tests: no network, and what was asked is kept.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::VecDeque;

    use super::*;

    /// A request as the fake transport saw it; the key only as whether it was given.
    #[derive(Clone, Debug)]
    pub struct Sent {
        pub url: String,
        pub headers: Vec<(String, String)>,
        pub body: Value,
        pub key_given: bool,
        /// The header the key was sent in.
        pub key_as: KeyHeader,
    }

    #[derive(Default)]
    pub struct Fake {
        replies: Mutex<VecDeque<Result<Reply, TransportError>>>,
        pub sent: Mutex<Vec<Sent>>,
        pub waited: Mutex<Vec<Duration>>,
    }

    pub fn ok(body: &Value) -> Reply {
        Reply { status: 200, retry_after: None, should_retry: None, body: body.to_string().into_bytes() }
    }

    pub fn error(status: u16, kind: &str, message: &str) -> Reply {
        let body = json!({"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_011CSHoEeqs5C35K2UUqR7Fy"});
        Reply { status, retry_after: None, should_retry: None, body: body.to_string().into_bytes() }
    }

    /// An answer as the Messages API documents it: a (summarised-away) thinking block, then the text.
    pub fn message(model: &str, text: &str, stop_reason: &str, usage: Value) -> Value {
        json!({
            "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [
                {"type": "thinking", "thinking": "", "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pk"},
                {"type": "text", "text": text}
            ],
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "stop_details": null,
            "usage": usage
        })
    }

    impl Fake {
        pub fn new(replies: Vec<Result<Reply, TransportError>>) -> Self {
            Self { replies: Mutex::new(replies.into()), ..Self::default() }
        }

        pub fn sent(&self) -> Vec<Sent> {
            self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
        }
    }

    /// An answer as OpenAI's Responses reference documents it: a reasoning item, then the message.
    pub fn response(model: &str, text: &str, usage: Value) -> Value {
        json!({
            "id": "resp_67ccd2bed1ec8190b14f964abc0542670bb6a6b452d3795b",
            "object": "response",
            "status": "completed",
            "model": model,
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": []},
                {"type": "message", "id": "msg_1", "status": "completed", "role": "assistant",
                 "content": [{"type": "output_text", "text": text, "annotations": []}]}
            ],
            "usage": usage
        })
    }

    /// An answer as Chat Completions documents it.
    pub fn completion(model: &str, text: &str, usage: Value) -> Value {
        json!({
            "id": "chatcmpl-B9MBs8CjcvOU2jLn4n570S5qMJKcT",
            "object": "chat.completion",
            "model": model,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text, "refusal": null}, "finish_reason": "stop"}],
            "usage": usage
        })
    }

    /// An answer as Gemini's `generateContent` documents it.
    pub fn generated(model: &str, text: &str, usage: Value) -> Value {
        json!({
            "candidates": [{"content": {"role": "model", "parts": [{"text": text}]}, "finishReason": "STOP", "index": 0}],
            "usageMetadata": usage,
            "modelVersion": model,
            "responseId": "mAitaLmkHPPlz7IPvtfUqQ4"
        })
    }

    /// An endpoint of `provider` with a key made up for the tests (and, for the compatible one, a server on this
    /// computer).
    pub fn endpoint(provider: Provider) -> Endpoint {
        Endpoint::for_provider(provider, |name| match name {
            "CONSTELLATE_MODEL_BASE_URL" => Some("http://localhost:11434/v1".to_owned()),
            name if name.ends_with("_BASE_URL") => None,
            _ => Some("test-key-not-real".to_owned()),
        })
        .unwrap_or_else(|why| panic!("{why}"))
    }

    impl Fake {
        fn next(
            &self,
            url: &str,
            sent_as: KeyHeader,
            key: &ApiKey,
            headers: &[(&str, &str)],
            body: &[u8],
        ) -> Result<Reply, TransportError> {
            self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Sent {
                url: url.to_owned(),
                headers: headers.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect(),
                body: serde_json::from_slice(body).unwrap_or(Value::Null),
                key_given: !key.expose().is_empty(),
                key_as: sent_as,
            });
            self.replies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Err(TransportError::Unreachable))
        }
    }

    impl Transport for Fake {
        fn post(
            &self,
            url: &str,
            key: &ApiKey,
            headers: &[(&str, &str)],
            body: &[u8],
            _patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.next(url, KeyHeader::XApiKey, key, headers, body)
        }

        fn post_keyed(
            &self,
            url: &str,
            key: &ApiKey,
            sent_as: KeyHeader,
            headers: &[(&str, &str)],
            body: &[u8],
            _patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.next(url, sent_as, key, headers, body)
        }

        fn get(
            &self,
            url: &str,
            key: &ApiKey,
            sent_as: KeyHeader,
            _patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.next(url, sent_as, key, &[], b"")
        }

        fn wait(&self, duration: Duration) {
            self.waited.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(duration);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::fake::{Fake, error, message, ok};
    use super::*;
    use crate::usage::model_price;

    const KEY: &str = "test-key-not-real";

    fn endpoint() -> Endpoint {
        Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| KEY.to_owned())).unwrap()
    }

    fn schema() -> Value {
        json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"], "additionalProperties": false})
    }

    fn ask<'a>(schema: &'a Value, shared: Option<&'a str>) -> Ask<'a> {
        Ask {
            purpose: "questions about a paper",
            model: "claude-opus-5-5",
            system: "Answer from the paper only.",
            shared,
            question: "What does the paper claim?",
            form: Form::Json(schema),
            effort: Effort::Low,
            max_tokens: 2_000,
        }
    }

    fn usage(input: u64, output: u64, read: u64, written: u64) -> Value {
        json!({
            "input_tokens": input,
            "output_tokens": output,
            "cache_read_input_tokens": read,
            "cache_creation_input_tokens": written,
            "cache_creation": {"ephemeral_5m_input_tokens": written, "ephemeral_1h_input_tokens": 0}
        })
    }

    fn plenty() -> Budget {
        Budget::new(Limit::Dollars(Usd::from_picodollars(10_000_000_000_000)))
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct Said {
        answer: String,
    }

    #[test]
    fn a_question_is_sent_as_the_reference_documents_and_its_json_answer_read() {
        let fake = Fake::new(vec![Ok(ok(&message(
            "claude-opus-5-5",
            r#"{"answer": "it merges"}"#,
            "end_turn",
            usage(30, 12, 0, 5_000),
        )))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let answered: Answered<Said> = client.ask_json(&ask(&schema, Some("The paper's text.")), &plenty()).unwrap();
        assert_eq!(answered.answer, Said { answer: "it merges".to_owned() });

        let sent = &fake.sent()[0];
        assert_eq!(sent.url, "https://api.anthropic.com/v1/messages");
        assert!(sent.key_given);
        assert!(sent.headers.contains(&("anthropic-version".to_owned(), "2026-10-21".to_owned())));
        assert!(sent.headers.contains(&("anthropic-beta".to_owned(), "server-side-fallback-2026-07-01".to_owned())));
        let body = &sent.body;
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["max_tokens"], 2_000);
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["output_config"]["format"], json!({"type": "json_schema", "schema": schema}));
        assert_eq!(body["fallbacks"], "default");
        assert!(body.get("temperature").is_none());
        // the shared part first and cached, the question after the last mark
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["text"], "The paper's text.");
        assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(content[1], json!({"type": "text", "text": "What does the paper claim?"}));
        assert_eq!(body["system"][0]["cache_control"], json!({"type": "ephemeral"}));
        assert!(!sent.body.to_string().contains(KEY), "the key is only ever a header");
    }

    #[test]
    fn cache_reads_and_writes_are_read_from_the_usage_and_recorded_with_the_purpose() {
        let fake = Fake::new(vec![
            Ok(ok(&message("claude-opus-5-5", r#"{"answer": "a"}"#, "end_turn", usage(30, 10, 0, 40_000)))),
            Ok(ok(&message("claude-opus-5-5", r#"{"answer": "b"}"#, "end_turn", usage(25, 10, 40_000, 0)))),
        ]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let first = client.ask(&ask(&schema, Some("text")), &plenty()).unwrap();
        let second = client.ask(&ask(&schema, Some("text")), &plenty()).unwrap();
        assert_eq!((first.tokens.cache_write_5m, first.tokens.cache_read), (40_000, 0));
        assert_eq!((second.tokens.cache_write_5m, second.tokens.cache_read), (0, 40_000), "the second read the cache");
        // 25 * $4 + 10 * $20 + 40000 * $0.20 per million tokens
        assert_eq!(second.cost, Some(Usd::from_picodollars((100 + 200 + 8_000) * 1_000_000)));
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].purpose, "questions about a paper");
        assert_eq!(calls[1].tokens.cache_read, 40_000);
    }

    #[test]
    fn a_refusal_and_a_cut_off_answer_are_no_answer_but_are_still_recorded() {
        let mut refused = message("claude-opus-5-5", "", "refusal", usage(30, 0, 0, 0));
        refused["stop_details"] = json!({"type": "refusal", "category": "cyber", "explanation": "declined"});
        let fake = Fake::new(vec![
            Ok(ok(&refused)),
            Ok(ok(&message("claude-opus-5-5", r#"{"answer": "half"#, "max_tokens", usage(30, 2_000, 0, 0)))),
        ]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        assert_eq!(
            client.ask(&ask(&schema, None), &plenty()),
            Err(AskError::NoAnswer(NoAnswer::Refused { category: Some("cyber".to_owned()) }))
        );
        assert_eq!(client.ask(&ask(&schema, None), &plenty()), Err(AskError::NoAnswer(NoAnswer::Cut)));
        assert_eq!(calls.into_inner().unwrap().len(), 2, "both were paid for");
    }

    #[test]
    fn an_answer_without_its_usage_is_counted_at_its_worst_case_not_as_free() {
        let mut answer = message("claude-opus-5-5", r#"{"answer": "x"}"#, "end_turn", Value::Null);
        answer.as_object_mut().unwrap().remove("usage");
        let fake = Fake::new(vec![Ok(ok(&answer))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let budget = Budget::new(Limit::Tokens(100_000));
        let answered = client.ask(&ask(&schema, Some("The paper's text.")), &budget).unwrap();
        assert_eq!(answered.answer, r#"{"answer": "x"}"#);
        assert_eq!(answered.tokens.output, 2_000, "max_tokens: the thinking it took is not known");
        assert_eq!(budget.spent().0, answered.tokens.all());
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls[0].tokens, answered.tokens);
        assert!(calls[0].cost().is_some_and(|cost| cost > Usd::ZERO));
    }

    #[test]
    fn prose_cut_off_by_the_context_window_is_not_shown_as_a_whole_answer() {
        let cut = message(
            "claude-opus-5-5",
            "The paper shows that merging",
            "model_context_window_exceeded",
            usage(30, 9, 0, 0),
        );
        let fake = Fake::new(vec![Ok(ok(&cut))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let prose = Ask { form: Form::Text, ..ask(&schema, None) };
        assert_eq!(client.ask(&prose, &plenty()), Err(AskError::NoAnswer(NoAnswer::Cut)));
    }

    #[test]
    fn a_providers_error_message_is_shown_without_keys_headers_or_more_than_its_length() {
        // made at run time, so that no key-shaped text is ever written in the repository
        let own = format!("{}{}", "own-", "q".repeat(24));
        let shaped = format!("sk-{}", "Z9".repeat(20));
        let key = ApiKey(own.clone());
        let message = format!("invalid request for {own}, see {shaped} and\nnext\tline");
        let said = scrubbed(&message, &key);
        assert!(!said.contains(&own) && !said.contains(&shaped), "{said}");
        assert!(said.contains("invalid request for [hidden]") && said.contains("next line"), "{said}");
        let echoed = "bad request: headers were Authorization: Bearer whatever x-api-key: whatever";
        assert_eq!(scrubbed(echoed, &key), "bad request: headers were [the rest is not shown]");
        let opaque = format!("token {} refused", "a1B2".repeat(10));
        assert_eq!(scrubbed(&opaque, &key), "token [hidden] refused", "a long opaque run is taken for a token");
        let long = "word ".repeat(200);
        let cut = scrubbed(&long, &key);
        assert!(cut.chars().count() <= MESSAGE_CHARS + 1 && cut.ends_with('…'), "{cut}");
        // through the client: what the API refused, with the key it echoed hidden
        let fake = Fake::new(vec![Ok(error(400, "invalid_request_error", &format!("bad key {shaped}")))]);
        let calls = Mutex::new(Vec::new());
        let schema = schema();
        let refused = Client::new(&fake, endpoint(), &calls).ask(&ask(&schema, None), &plenty()).unwrap_err();
        assert_eq!(refused, NoAnswer::Rejected("bad key [hidden]".to_owned()).into());
    }

    #[test]
    fn a_fallback_to_a_dearer_model_is_held_and_estimated_at_its_price_and_never_passes_the_budget() {
        let schema = schema();
        let asked = ask(&schema, Some("The paper's text."));
        let calls = Mutex::new(Vec::new());
        let nothing = Fake::new(Vec::new());
        let worst = Client::new(&nothing, endpoint(), &calls).worst(&asked);
        // the model asked's worst case alone, as was held before: room for it, not for a fallback after it
        let alone = Budget::new(Limit::Dollars(price_of("claude-opus-5-5", &worst).unwrap()));
        let refused = Client::new(&nothing, endpoint(), &calls).ask(&asked, &alone);
        assert!(matches!(refused, Err(AskError::OverBudget(OverBudget::Dollars { .. }))), "{refused:?}");
        assert!(nothing.sent().is_empty(), "refused before anything was sent");

        // the estimate's "at most" is what is held: the model asked and the dearer of its fallbacks, at worst
        let (shared, question) = asked.characters();
        let planned = Planned {
            shared_chars: shared,
            question_chars: question,
            questions: 1,
            answer_tokens: 100,
            max_tokens: 2_000,
        };
        let estimate = estimate_for(Provider::Anthropic, "claude-opus-5-5", &[planned], Counting::ASSUMED);
        let opus_5 = price_of("claude-opus-5", &worst).unwrap();
        assert!(opus_5 > price_of("claude-opus-5-5", &worst).unwrap(), "the fallback is the dearer");
        assert_eq!(estimate.most_usd, Some(price_of("claude-opus-5-5", &worst).unwrap() + opus_5));
        assert_eq!(estimate.most_tokens, 2 * worst.all());

        // the API answers as Claude Opus 5 after the model asked declined part way: both at their worst
        let part = |model: &str, kind: &str| {
            json!({"type": kind, "model": model, "input_tokens": worst.input, "output_tokens": worst.output,
                   "cache_creation_input_tokens": worst.cache_write_5m})
        };
        let mut answered = message("claude-opus-5", r#"{"answer": "x"}"#, "end_turn", usage(1, 1, 0, 0));
        answered["usage"]["iterations"] =
            json!([part("claude-opus-5-5", "message"), part("claude-opus-5", "fallback_message")]);
        let fake = Fake::new(vec![Ok(ok(&answered))]);
        let budget = Budget::new(Limit::Dollars(estimate.most_usd.unwrap()));
        let reply = Client::new(&fake, endpoint(), &calls).ask(&asked, &budget).unwrap();
        assert_eq!(reply.model, "claude-opus-5");
        assert!(budget.spent().1 <= estimate.most_usd.unwrap(), "{:?} spent", budget.spent());
        assert_eq!(budget.held(), (0, Usd::ZERO));
    }

    #[test]
    fn a_model_asked_without_fallbacks_is_held_and_estimated_once() {
        assert_eq!(answered_by(Provider::Anthropic, "claude-haiku-5-5"), ["claude-haiku-5-5"]);
        assert_eq!(answered_by(Provider::OpenAi, "gpt-6.2-sol"), ["openai:gpt-6.2-sol"]);
        assert_eq!(answered_by(Provider::Anthropic, "claude-opus-5-5").len(), 1 + FALLBACK_TARGETS.len());
        let worst = Tokens { input: 100, output: 100, ..Tokens::default() };
        assert_eq!(worst_of(&["claude-haiku-5-5"], &worst), (200, Ok(price_of("claude-haiku-5-5", &worst).unwrap())));
        // a fallback whose price is not known makes the call unpriced, never guessed
        assert_eq!(worst_of(&["claude-opus-5-5", "claude-unknown-9"], &worst).1, Err("claude-unknown-9".to_owned()));
        for target in FALLBACK_TARGETS {
            assert!(
                model_price(target).is_some(),
                "{target} has a price, so a call that may fall back to it is priced"
            );
        }
    }

    #[test]
    fn holds_reserved_settled_and_dropped_from_many_threads_never_pass_the_limit_and_all_come_back() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        const LIMIT: u64 = 10_000;
        let tokens = |input: u64| Tokens { input, ..Tokens::default() };
        for seed in 0..16u64 {
            let budget = Budget::new(Limit::Tokens(LIMIT));
            std::thread::scope(|scope| {
                for thread in 0..6u64 {
                    let budget = &budget;
                    scope.spawn(move || {
                        let mut random = StdRng::seed_from_u64(seed * 1_000 + thread);
                        let mut holds: Vec<(Hold<'_>, u64)> = Vec::new();
                        for _ in 0..400 {
                            match random.gen_range(0..4) {
                                0 | 1 => {
                                    let worst = random.gen_range(1..3_000);
                                    if let Ok(hold) = budget.reserve(&["claude-opus-5-5"], &tokens(worst)) {
                                        holds.push((hold, worst));
                                    }
                                }
                                2 if !holds.is_empty() => {
                                    let (hold, worst) = holds.swap_remove(random.gen_range(0..holds.len()));
                                    let took = random.gen_range(0..=worst);
                                    hold.settle(&tokens(took), None);
                                }
                                3 if !holds.is_empty() => {
                                    drop(holds.swap_remove(random.gen_range(0..holds.len())));
                                }
                                _ => {}
                            }
                            let (spent, held) = budget.taken();
                            assert!(spent + held <= LIMIT, "spent {spent} and held {held} of {LIMIT}");
                        }
                        // whatever is still held is dropped here, as a thread that ends would
                    });
                }
            });
            assert_eq!(budget.held(), (0, Usd::ZERO), "seed {seed}: every hold came back");
            assert!(budget.spent().0 <= LIMIT);
        }
    }

    #[test]
    fn a_fallback_s_tokens_are_recorded_by_each_model_that_worked_on_the_answer() {
        let mut answered = message("claude-opus-4-8", r#"{"answer": "x"}"#, "end_turn", usage(30, 40, 0, 0));
        answered["usage"]["iterations"] = json!([
            {"type": "message", "model": "claude-opus-5-5", "input_tokens": 30, "output_tokens": 5},
            {"type": "fallback_message", "model": "claude-opus-4-8", "input_tokens": 30, "output_tokens": 35}
        ]);
        let fake = Fake::new(vec![Ok(ok(&answered))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let reply = client.ask(&ask(&schema, None), &plenty()).unwrap();
        assert_eq!(reply.model, "claude-opus-4-8");
        let calls = calls.into_inner().unwrap();
        assert_eq!(
            calls.iter().map(|call| call.model.as_str()).collect::<Vec<_>>(),
            ["claude-opus-5-5", "claude-opus-4-8"]
        );
        assert_eq!(reply.tokens.output, 40);
    }

    #[test]
    fn a_rate_limit_is_waited_out_as_retry_after_says_then_answered() {
        let mut limited =
            error(429, "rate_limit_error", "Number of request tokens has exceeded your per-minute rate limit");
        limited.retry_after = Some("7".to_owned());
        let fake = Fake::new(vec![
            Ok(limited),
            Ok(error(529, "overloaded_error", "Overloaded")),
            Ok(ok(&message("claude-opus-5-5", r#"{"answer": "x"}"#, "end_turn", usage(1, 1, 0, 0)))),
        ]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        assert!(client.ask(&ask(&schema, None), &plenty()).is_ok());
        assert_eq!(*fake.waited.lock().unwrap(), [Duration::from_secs(7), Duration::from_secs(2)]);
    }

    #[test]
    fn errors_are_told_apart_into_stop_asking_and_try_again() {
        let cases: Vec<(Reply, AskError)> = vec![
            (error(401, "authentication_error", "invalid x-api-key"), Unavailable::KeyRefused.into()),
            (error(403, "permission_error", "not allowed"), Unavailable::NotAllowed.into()),
            (
                error(404, "not_found_error", "model: claude-opus-9"),
                Unavailable::UnknownModel("claude-opus-5-5".to_owned()).into(),
            ),
            (error(413, "request_too_large", "too large"), NoAnswer::TooLarge.into()),
            (
                error(400, "invalid_request_error", "messages: roles must alternate"),
                NoAnswer::Rejected("messages: roles must alternate".to_owned()).into(),
            ),
        ];
        for (reply, expected) in cases {
            let fake = Fake::new(vec![Ok(reply)]);
            let calls = Mutex::new(Vec::new());
            let client = Client::new(&fake, endpoint(), &calls);
            let schema = schema();
            assert_eq!(client.ask(&ask(&schema, None), &plenty()), Err(expected.clone()));
            assert_eq!(fake.sent().len(), 1, "{expected:?} is not tried again");
        }
        let mut spent = error(429, "rate_limit_error", "spend limit");
        spent.body = json!({"type": "error", "error": {"type": "rate_limit_error", "message": "limit", "details": {"error_code": "enforced_spend_limit_reached"}}}).to_string().into_bytes();
        assert_eq!(failure(&spent, "m"), AskError::Unavailable(Unavailable::SpendLimit));
        for header in ["-1", "NaN", "inf", "1e30", "soon"] {
            let mut odd = error(429, "rate_limit_error", "slow down");
            odd.retry_after = Some(header.to_owned());
            let AskError::NoAnswer(NoAnswer::RateLimited { retry_after }) = failure(&odd, "m") else { panic!() };
            assert!(retry_after.is_none_or(|wait| wait > LONGEST_WAIT), "{header}: never waited for, never a panic");
        }
        let fake = Fake::new(vec![
            Ok(error(500, "api_error", "x")),
            Ok(error(500, "api_error", "x")),
            Ok(error(500, "api_error", "x")),
        ]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        assert_eq!(client.ask(&ask(&schema, None), &plenty()), Err(NoAnswer::ServerError(500).into()));
        assert_eq!(fake.sent().len(), 3, "tried, and tried again twice");
    }

    #[test]
    fn once_the_api_cannot_be_used_nothing_more_is_sent() {
        let fake = Fake::new(vec![Ok(error(401, "authentication_error", "invalid x-api-key"))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let _ = client.ask(&ask(&schema, None), &plenty());
        assert_eq!(client.ask(&ask(&schema, None), &plenty()), Err(Unavailable::KeyRefused.into()));
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn the_key_comes_from_the_environment_only_and_is_never_shown() {
        assert_eq!(Endpoint::from_lookup(|_| None).unwrap_err(), Unavailable::NoKey);
        assert_eq!(Endpoint::from_lookup(|_| Some("  ".to_owned())).unwrap_err(), Unavailable::NoKey);
        let endpoint = endpoint();
        assert!(endpoint.is_default());
        assert!(!format!("{endpoint:?} {:?}", endpoint.key).contains(KEY));
        let proxy = Endpoint::from_lookup(|name| {
            Some(if name == KEY_VARIABLE { KEY } else { "https://gateway.example.org/claude/" }.to_owned())
        });
        assert_eq!(proxy.unwrap().url("/v1/messages"), "https://gateway.example.org/claude/v1/messages");
        let plain = Endpoint::from_lookup(|name| {
            Some(if name == KEY_VARIABLE { KEY } else { "http://gateway.example.org" }.to_owned())
        });
        assert_eq!(plain.unwrap_err(), Unavailable::BadEndpoint, "a key is never sent over plain HTTP");
        let local = Endpoint::from_lookup(|name| {
            Some(if name == KEY_VARIABLE { KEY } else { "http://127.0.0.1:8080" }.to_owned())
        });
        assert!(local.is_ok());
        for error in [Unavailable::BadEndpoint, Unavailable::NoSuchEndpoint, Unavailable::KeyRefused] {
            assert!(!error.to_string().contains("example.org") && !error.to_string().contains(KEY));
        }
    }

    #[test]
    fn a_call_that_could_pass_the_budget_is_not_started() {
        let fake = Fake::new(vec![Ok(ok(&message(
            "claude-opus-5-5",
            r#"{"answer": "x"}"#,
            "end_turn",
            usage(100, 100, 0, 0),
        )))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        // 2000 answer tokens at $20 per million is $0.04: a budget of one cent cannot take it
        let tight = Budget::new(Limit::Dollars(Usd::from_picodollars(10_000_000_000)));
        assert!(matches!(
            client.ask(&ask(&schema, None), &tight),
            Err(AskError::OverBudget(OverBudget::Dollars { .. }))
        ));
        assert!(fake.sent().is_empty(), "refused before anything was sent");
        let tokens = Budget::new(Limit::Tokens(1_000));
        assert!(matches!(
            client.ask(&ask(&schema, None), &tokens),
            Err(AskError::OverBudget(OverBudget::Tokens { .. }))
        ));
        // what is spent is what the call took, not what it might have
        let enough = Budget::new(Limit::Tokens(10_000));
        client.ask(&ask(&schema, None), &enough).unwrap();
        assert_eq!(enough.spent().0, 200);
        let unpriced = Ask { model: "claude-unknown-1", ..ask(&schema, None) };
        assert_eq!(client.ask(&unpriced, &plenty()), Err(OverBudget::Unpriced("claude-unknown-1".to_owned()).into()));
    }

    #[test]
    fn a_batch_is_estimated_before_it_starts_with_the_cache_counted() {
        // 739 papers of 40,000 characters, three questions each
        let plan =
            Planned { shared_chars: 40_000, question_chars: 300, questions: 3, answer_tokens: 800, max_tokens: 4_000 };
        let estimated = estimate_for(
            Provider::Anthropic,
            "claude-opus-5-5",
            &vec![plan; 739],
            Counting::Measured { chars_per_token: 4.0 },
        );
        assert_eq!(estimated.calls, 739 * 3);
        assert_eq!(estimated.tokens.cache_write_5m, 739 * 10_000);
        assert_eq!(estimated.tokens.cache_read, 739 * 2 * 10_000);
        let likely = estimated.usd.unwrap();
        let most = estimated.most_usd.unwrap();
        assert!(likely < most);
        // per paper: 10000 written at $5, 20000 read at $0.20, 225 question tokens at $4, 2400 answer tokens at $20
        let per_paper = 10_000 * 5_000_000 + 20_000 * 200_000 + 225 * 4_000_000 + 2_400 * 20_000_000;
        assert_eq!(likely, Usd::from_picodollars(739 * per_paper));
        assert_eq!(estimate_for(Provider::Anthropic, "claude-unknown-1", &[plan], Counting::ASSUMED).usd, None);
        assert_eq!(Counting::ASSUMED, Counting::Assumed { chars_per_token: CHARS_PER_TOKEN });
    }

    #[test]
    fn tokens_are_counted_with_the_api_s_endpoint_and_estimates_follow() {
        let fake = Fake::new(vec![Ok(ok(&json!({"input_tokens": 2_000})))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(), &calls);
        let schema = schema();
        let text = "x".repeat(8_000 - 27 - 26);
        let measured = client.calibrate(&ask(&schema, Some(&text))).unwrap();
        assert_eq!(measured, Counting::Measured { chars_per_token: 4.0 });
        assert_eq!(client.counting(), measured);
        let sent = &fake.sent()[0];
        assert_eq!(sent.url, "https://api.anthropic.com/v1/messages/count_tokens");
        assert!(sent.body.get("max_tokens").is_none());
        assert!(sent.body.get("fallbacks").is_none());
        assert!(calls.into_inner().unwrap().is_empty(), "counting writes no answer and is not a call in the ledger");
    }

    #[test]
    fn the_models_offered_are_listed_once_with_the_default_first() {
        assert_eq!(DEFAULT_MODEL, "claude-opus-5-5");
        assert_eq!(model_ids(), ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-2"]);
        for offered in MODELS {
            assert!(model_price(offered.id).is_some(), "{} has a price", offered.id);
        }
        assert_eq!(model("claude-haiku-5-5").unwrap().fallback, Fallback::None);
    }
}

/// The client with every other provider: the same [`Ask`], each provider's own shapes on the wire.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod every_provider {
    use super::fake::{Fake, completion, endpoint, generated, ok, response};
    use super::*;

    const KEY: &str = "test-key-not-real";

    fn schema() -> Value {
        json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"], "additionalProperties": false})
    }

    fn ask<'a>(model: &'a str, schema: &'a Value) -> Ask<'a> {
        Ask {
            purpose: "questions about a paper",
            model,
            system: "Answer from the paper only.",
            shared: Some("The paper's text."),
            question: "What does the paper claim?",
            form: Form::Json(schema),
            effort: Effort::Max,
            max_tokens: 2_000,
        }
    }

    /// In tokens: most of these models have no price, and a budget in dollars would refuse them.
    fn plenty() -> Budget {
        Budget::new(Limit::Tokens(10_000_000))
    }

    fn chat_usage(prompt: u64, output: u64, cached: u64) -> Value {
        json!({"prompt_tokens": prompt, "completion_tokens": output, "total_tokens": prompt + output,
            "prompt_tokens_details": {"cached_tokens": cached}})
    }

    #[test]
    fn each_provider_is_sent_its_own_shapes_with_the_key_in_its_own_header() {
        let schema = schema();
        let cases: [(Provider, &str, Value, &str, KeyHeader); 4] = [
            (
                Provider::OpenAi,
                "gpt-6.2-sol",
                response("gpt-6.2-sol", r#"{"answer": "it merges"}"#, json!({"input_tokens": 40, "output_tokens": 9})),
                "https://api.openai.com/v1/responses",
                KeyHeader::Bearer,
            ),
            (
                Provider::Gemini,
                "gemini-4-carbon",
                generated(
                    "gemini-4-carbon",
                    r#"{"answer": "it merges"}"#,
                    json!({"promptTokenCount": 40, "candidatesTokenCount": 9}),
                ),
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-4-carbon:generateContent",
                KeyHeader::GoogApiKey,
            ),
            (
                Provider::Groq,
                "openai/gpt-oss-120b",
                completion("openai/gpt-oss-120b", r#"{"answer": "it merges"}"#, chat_usage(40, 9, 0)),
                "https://api.groq.com/openai/v1/chat/completions",
                KeyHeader::Bearer,
            ),
            (
                Provider::Compatible,
                "qwen3:32b",
                completion("qwen3:32b", r#"{"answer": "it merges"}"#, chat_usage(40, 9, 0)),
                "http://localhost:11434/v1/chat/completions",
                KeyHeader::Bearer,
            ),
        ];
        for (provider, model, answer, url, header) in cases {
            let fake = Fake::new(vec![Ok(ok(&answer))]);
            let calls = Mutex::new(Vec::new());
            let client = Client::new(&fake, endpoint(provider), &calls);
            let answered: Answered<Value> = client.ask_json(&ask(model, &schema), &plenty()).unwrap();
            assert_eq!(answered.answer, json!({"answer": "it merges"}), "{provider:?}");
            let sent = &fake.sent()[0];
            assert_eq!((sent.url.as_str(), sent.key_as), (url, header), "{provider:?}");
            assert!(sent.headers.is_empty(), "no Claude API headers go to {provider:?}");
            assert!(!sent.body.to_string().contains(KEY), "the key is only ever a header");
            let recorded = calls.into_inner().unwrap();
            assert_eq!(recorded[0].model, price_key(provider, model), "recorded under its provider");
            assert_eq!((recorded[0].tokens.input, recorded[0].tokens.output), (40, 9));
        }
    }

    #[test]
    fn an_effort_is_sent_as_the_model_has_it_and_not_at_all_where_it_has_none() {
        let schema = schema();
        let fake = Fake::new(vec![
            Ok(ok(&generated("gemini-4-carbon", r#"{"answer": "x"}"#, json!({})))),
            Ok(ok(&completion("mistral-large-4-0", r#"{"answer": "x"}"#, chat_usage(1, 1, 0)))),
            Ok(ok(&response("gpt-7-nova", r#"{"answer": "x"}"#, json!({})))),
        ]);
        let calls = Mutex::new(Vec::new());
        // Gemini 4 Carbon has low, medium and high: max is sent as high
        let gemini = Client::new(&fake, endpoint(Provider::Gemini), &calls);
        gemini.ask(&ask("gemini-4-carbon", &schema), &plenty()).unwrap();
        let mistral = Client::new(&fake, endpoint(Provider::Mistral), &calls);
        mistral.ask(&ask("mistral-large-4-0", &schema), &plenty()).unwrap();
        // a model id typed rather than listed: its efforts are not known, so none is sent
        let openai = Client::new(&fake, endpoint(Provider::OpenAi), &calls);
        openai.ask(&ask("gpt-7-nova", &schema), &plenty()).unwrap();
        let sent = fake.sent();
        assert_eq!(sent[0].body["generationConfig"]["thinkingConfig"], json!({"thinkingLevel": "high"}));
        assert!(sent[1].body.get("reasoning_effort").is_none());
        assert!(sent[2].body.get("reasoning").is_none());
    }

    #[test]
    fn json_a_provider_cannot_be_held_to_is_checked_and_asked_for_once_more() {
        let schema = schema();
        // MiniMax takes no schema: it is written into the question, and the answer checked here
        let fake = Fake::new(vec![
            Ok(ok(&completion("MiniMax-M3", "Sure! The paper claims that merging works.", chat_usage(50, 10, 0)))),
            Ok(ok(&completion("MiniMax-M3", "```json\n{\"answer\": \"merging works\"}\n```", chat_usage(60, 10, 0)))),
        ]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::MiniMax), &calls);
        let answered = client.ask(&ask("MiniMax-M3", &schema), &plenty()).unwrap();
        assert_eq!(answered.answer, r#"{"answer": "merging works"}"#, "the JSON alone, out of its fence");
        let sent = fake.sent();
        assert!(sent[0].body.get("response_format").is_none());
        let first = sent[0].body["messages"][1]["content"].as_str().unwrap();
        assert!(first.starts_with("The paper's text.\n\nWhat does the paper claim?") && first.contains("JSON Schema"));
        let second = sent[1].body["messages"][1]["content"].as_str().unwrap();
        assert!(second.contains("could not be used: it is not JSON"), "{second}");
        assert_eq!(calls.into_inner().unwrap().len(), 2, "both attempts were paid for");

        // a schema requested but not followed, twice: not JSON, after exactly one more try
        let fake = Fake::new(vec![
            Ok(ok(&completion("qwen3:32b", r#"{"answer": 3}"#, chat_usage(5, 5, 0)))),
            Ok(ok(&completion("qwen3:32b", r#"{"reply": "x"}"#, chat_usage(5, 5, 0)))),
            Ok(ok(&completion("qwen3:32b", r#"{"answer": "x"}"#, chat_usage(5, 5, 0)))),
        ]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::Compatible), &calls);
        let error = client.ask(&ask("qwen3:32b", &schema), &plenty()).unwrap_err();
        assert!(
            matches!(error, AskError::NoAnswer(NoAnswer::NotJson(ref why)) if why.contains("lacks \"answer\"")),
            "{error:?}"
        );
        assert_eq!(fake.sent().len(), 2);
        assert!(fake.sent()[0].body["messages"][1]["content"].as_str().unwrap().contains("What does the paper claim?"));
    }

    #[test]
    fn an_answer_not_in_json_whose_second_try_the_budget_cannot_take_is_not_json_not_a_call_never_started() {
        let schema = schema();
        let fake = Fake::new(vec![Ok(ok(&completion("MiniMax-M3", "Sure, it merges.", chat_usage(50, 10, 0))))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::MiniMax), &calls);
        let ask = ask("MiniMax-M3", &schema);
        // room for the first attempt's worst case, not for a second one after what the first took
        let first = client.worst_asked(&client.asked(&ask, None)).all();
        let budget = Budget::new(Limit::Tokens(first + 10));
        let error = client.ask(&ask, &budget).unwrap_err();
        // a batch starts a call refused by the budget again once room is made: this one was paid for already
        assert_eq!(error, NoAnswer::NotJson("it is not JSON".to_owned()).into());
        assert_eq!(fake.sent().len(), 1);
        assert_eq!(calls.into_inner().unwrap().len(), 1, "the first attempt is recorded");
        assert_eq!(budget.spent().0, 60);
    }

    #[test]
    fn a_compatible_server_can_be_told_to_take_json_in_the_prompt() {
        let endpoint = Endpoint::for_provider(Provider::Compatible, |name| match name {
            "CONSTELLATE_MODEL_BASE_URL" => Some("http://127.0.0.1:1234/v1".to_owned()),
            "CONSTELLATE_MODEL_JSON" => Some("prompt".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(endpoint.capabilities().answers, Answers::Prompted);
        let fake = Fake::new(vec![Ok(ok(&completion("local", r#"{"answer": "x"}"#, chat_usage(1, 1, 0))))]);
        let calls = Mutex::new(Vec::new());
        let schema = schema();
        Client::new(&fake, endpoint, &calls).ask(&ask("local", &schema), &plenty()).unwrap();
        let sent = &fake.sent()[0];
        assert!(sent.body.get("response_format").is_none());
        assert_eq!(sent.key_as, KeyHeader::NotSent, "no key is set, so none is sent");
    }

    #[test]
    fn a_server_that_reports_no_usage_is_counted_at_its_worst_case_and_its_cost_is_not_known() {
        let schema = schema();
        let mut answer = completion("qwen3:32b", r#"{"answer": "it merges"}"#, Value::Null);
        answer.as_object_mut().unwrap().remove("usage");
        let fake = Fake::new(vec![Ok(ok(&answer))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::Compatible), &calls);
        let budget = Budget::new(Limit::Tokens(100_000));
        let answered = client.ask(&ask("qwen3:32b", &schema), &budget).unwrap();
        assert!(answered.tokens.input > 0, "{:?}", answered.tokens);
        let max_tokens = u64::from(ask("qwen3:32b", &schema).max_tokens);
        assert_eq!(answered.tokens.output, max_tokens, "hidden thinking may have taken all the answer was allowed");
        assert_eq!(answered.cost, None, "not known, which is not free");
        assert_eq!(budget.spent().0, answered.tokens.all());
    }

    #[test]
    fn an_unpriced_model_is_kept_to_a_budget_in_tokens_and_refused_one_in_dollars() {
        let schema = schema();
        let fake = Fake::new(vec![Ok(ok(&completion("qwen3:32b", r#"{"answer": "x"}"#, chat_usage(100, 100, 0))))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::Compatible), &calls);
        let dollars = Budget::new(Limit::Dollars(Usd::from_picodollars(10_000_000_000_000)));
        let refused = client.ask(&ask("qwen3:32b", &schema), &dollars).unwrap_err();
        assert_eq!(refused, OverBudget::Unpriced("compatible:qwen3:32b".to_owned()).into());
        assert!(refused.to_string().contains("set a budget in tokens instead"));
        assert!(fake.sent().is_empty(), "refused before anything was sent");
        assert!(matches!(
            client.ask(&ask("qwen3:32b", &schema), &Budget::new(Limit::Tokens(1_000))),
            Err(AskError::OverBudget(OverBudget::Tokens { .. }))
        ));
        let enough = Budget::new(Limit::Tokens(10_000));
        client.ask(&ask("qwen3:32b", &schema), &enough).unwrap();
        assert_eq!(enough.spent().0, 200);
    }

    #[test]
    fn a_priced_model_of_another_provider_is_kept_to_a_budget_in_dollars() {
        let schema = schema();
        let fake = Fake::new(vec![]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::OpenAi), &calls);
        // 2000 answer tokens of GPT-6.2 Astra at $40 per million are $0.08: a budget of one cent cannot take it
        let tight = Budget::new(Limit::Dollars(Usd::from_picodollars(10_000_000_000)));
        assert!(matches!(
            client.ask(&ask("gpt-6.2-astra", &schema), &tight),
            Err(AskError::OverBudget(OverBudget::Dollars { .. }))
        ));
        assert!(fake.sent().is_empty());
    }

    #[test]
    fn a_batch_is_estimated_as_each_provider_caches_and_says_how() {
        let plan =
            Planned { shared_chars: 40_000, question_chars: 300, questions: 3, answer_tokens: 800, max_tokens: 4_000 };
        let counting = Counting::Measured { chars_per_token: 4.0 };
        // nothing cached: every question pays for the whole prefix, and the estimate says so
        let local = estimate_for(Provider::Compatible, "qwen3:32b", &[plan], counting);
        assert_eq!(local.caching, Caching::NotReported);
        assert_eq!((local.tokens.input, local.tokens.cache_read), (3 * (10_000 + 75), 0));
        assert_eq!(local.usd, None, "a model of the user's own server has no price");
        // cached by the provider at no extra price: the first pays in full, the others read
        let gemini = estimate_for(Provider::Gemini, "gemini-4-carbon", &[plan], counting);
        assert_eq!(gemini.caching, Caching::Automatic);
        assert_eq!((gemini.tokens.input, gemini.tokens.cache_read), (10_000 + 3 * 75, 2 * 10_000));
        assert!(gemini.usd.unwrap() < gemini.most_usd.unwrap());
        // writing billed: as the Claude API's
        let openai = estimate_for(Provider::OpenAi, "gpt-6.2-sol", &[plan], counting);
        assert_eq!((openai.tokens.cache_write_5m, openai.tokens.cache_read), (10_000, 2 * 10_000));
        // 10000 written at $2.50, 20000 read at $0.10, 225 question tokens at $2, 2400 answer tokens at $10
        let expected = 10_000 * 2_500_000 + 20_000 * 100_000 + 225 * 2_000_000 + 2_400 * 10_000_000;
        assert_eq!(openai.usd, Some(Usd::from_picodollars(expected)));
        assert_eq!(estimate_for(Provider::Anthropic, "claude-opus-5-5", &[plan], counting).caching, Caching::Marked);
    }

    #[test]
    fn an_estimate_counts_what_the_providers_way_of_answering_in_json_adds_and_no_fee_on_this_computer() {
        let plan =
            Planned { shared_chars: 40_000, question_chars: 300, questions: 3, answer_tokens: 800, max_tokens: 4_000 };
        let counting = Counting::ASSUMED;
        let held = Pricing::listed(Provider::OpenAi).estimate("gpt-6.2-sol", &[plan], counting);
        assert_eq!(held, estimate_for(Provider::OpenAi, "gpt-6.2-sol", &[plan], counting), "held to the schema");
        // not held to it: an answer that does not follow it is asked for once more, which the worst case counts
        let prompted = Pricing::listed(Provider::Meta);
        let once = estimate_for(Provider::Meta, "muse-spark-1.3", &[plan], counting);
        let twice = prompted.estimate("muse-spark-1.3", &[plan], counting);
        assert_eq!(twice.tokens, once.tokens, "the likely cost is one answer each");
        assert!(twice.most_tokens > 2 * once.most_tokens, "{} {}", twice.most_tokens, once.most_tokens);
        // the schema written into the question is part of what is sent
        let schema = json!({"type": "object", "properties": {"said": {"type": "string"}}});
        let ask = Ask {
            purpose: "a test",
            model: "muse-spark-1.3",
            system: "",
            shared: None,
            question: "What?",
            form: Form::Json(&schema),
            effort: Effort::Low,
            max_tokens: 10,
        };
        assert!(prompted.question_chars(&ask) > 5 + schema.to_string().chars().count() as u64);
        assert_eq!(Pricing::listed(Provider::Anthropic).question_chars(&ask), 5);
        // on this computer: no dollars, said so, and the worst case in tokens all the same
        let here = Pricing { on_this_computer: true, ..Pricing::listed(Provider::Compatible) };
        let local = here.estimate("qwen3:32b", &[plan], counting);
        assert_eq!((local.usd, local.most_usd, local.on_this_computer), (None, None, true));
        assert!(local.most_tokens > local.tokens.all());
    }

    #[test]
    fn tokens_are_counted_where_the_provider_can_and_estimated_where_it_cannot() {
        let schema = schema();
        let fake = Fake::new(vec![
            Ok(ok(&json!({"object": "response.input_tokens", "input_tokens": 12}))),
            Ok(ok(&json!({"totalTokens": 12}))),
        ]);
        let calls = Mutex::new(Vec::new());
        let openai = Client::new(&fake, endpoint(Provider::OpenAi), &calls);
        assert!(matches!(openai.calibrate(&ask("gpt-6.2-sol", &schema)).unwrap(), Counting::Measured { .. }));
        let gemini = Client::new(&fake, endpoint(Provider::Gemini), &calls);
        assert_eq!(gemini.count_tokens(&ask("gemini-4-carbon", &schema)).unwrap(), 12);
        let sent = fake.sent();
        assert_eq!(sent[0].url, "https://api.openai.com/v1/responses/input_tokens");
        assert!(sent[1].url.ends_with("/v1beta/models/gemini-4-carbon:countTokens"));

        let local = Client::new(&fake, endpoint(Provider::Compatible), &calls);
        assert_eq!(local.calibrate(&ask("qwen3:32b", &schema)).unwrap(), Counting::ASSUMED, "an estimate, and said so");
        assert_eq!(local.count_tokens(&ask("qwen3:32b", &schema)), Err(Unavailable::Setup(Setup::CannotCount).into()));
        assert_eq!(fake.sent().len(), 2, "nothing was sent to the provider that cannot count");
        assert!(calls.into_inner().unwrap().is_empty());
    }

    #[test]
    fn errors_of_any_provider_stop_the_asking_or_wait_as_the_claude_api_s_do() {
        let schema = schema();
        let refused = Reply {
            status: 401,
            retry_after: None,
            should_retry: None,
            body: json!({"error": {"message": "Incorrect API key provided", "type": "invalid_request_error", "code": "invalid_api_key"}})
                .to_string()
                .into_bytes(),
        };
        let fake = Fake::new(vec![Ok(refused)]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::OpenAi), &calls);
        assert_eq!(client.ask(&ask("gpt-6.2-sol", &schema), &plenty()), Err(Unavailable::KeyRefused.into()));
        assert_eq!(client.ask(&ask("gpt-6.2-sol", &schema), &plenty()), Err(Unavailable::KeyRefused.into()));
        assert_eq!(fake.sent().len(), 1, "nothing more is sent once the key was refused");

        let limited = Reply {
            status: 429,
            retry_after: Some("4".to_owned()),
            should_retry: None,
            body: json!({"error": {"message": "Rate limit reached", "code": "rate_limit_exceeded"}})
                .to_string()
                .into_bytes(),
        };
        let fake = Fake::new(vec![
            Ok(limited),
            Ok(ok(&completion("openai/gpt-oss-120b", r#"{"answer": "x"}"#, chat_usage(1, 1, 0)))),
        ]);
        let client = Client::new(&fake, endpoint(Provider::Groq), &calls);
        assert!(client.ask(&ask("openai/gpt-oss-120b", &schema), &plenty()).is_ok());
        assert_eq!(*fake.waited.lock().unwrap(), [Duration::from_secs(4)]);

        let refusal = json!({"model": "m", "choices": [{"message": {"content": null, "refusal": "No."}, "finish_reason": "stop"}],
            "usage": chat_usage(3, 1, 0)});
        let fake = Fake::new(vec![Ok(ok(&refusal))]);
        let calls = Mutex::new(Vec::new());
        let client = Client::new(&fake, endpoint(Provider::Mistral), &calls);
        assert_eq!(
            client.ask(&ask("mistral-large-4-0", &schema), &plenty()),
            Err(NoAnswer::Refused { category: None }.into())
        );
        assert_eq!(calls.into_inner().unwrap().len(), 1, "a refusal was paid for");
    }

    #[test]
    fn every_provider_s_keys_and_addresses_come_from_its_variables_and_are_never_shown() {
        const SECRET: &str = "sk-test-abcdefghijklmnop-not-real";
        for provider in Provider::ALL {
            let about = provider.about();
            let missing = Endpoint::for_provider(provider, |_| None);
            if about.key == Key::Required(about.key.variable()) {
                let error = missing.unwrap_err().to_string();
                assert!(error.contains(about.key.variable()), "{provider:?}: {error}");
            }
            let set = Endpoint::for_provider(provider, |name| match name {
                "CONSTELLATE_MODEL_BASE_URL" => Some("https://gateway.example.org/v1".to_owned()),
                name if name.ends_with("_BASE_URL") => None,
                _ => Some(SECRET.to_owned()),
            })
            .unwrap();
            assert!(!format!("{set:?}").contains(SECRET));
            assert!(!set.on_this_computer());
        }
        let plain = Endpoint::for_provider(Provider::OpenAi, |name| match name {
            "OPENAI_BASE_URL" => Some("http://gateway.example.org/v1".to_owned()),
            _ => Some(SECRET.to_owned()),
        });
        let error = plain.unwrap_err();
        assert_eq!(error, Setup::NotHttps("OPENAI_BASE_URL").into(), "a key is never sent over plain HTTP");
        assert!(!error.to_string().contains("example.org") && !error.to_string().contains(SECRET));
        let proxy = Endpoint::for_provider(Provider::OpenAi, |name| match name {
            "OPENAI_BASE_URL" => Some("https://gateway.example.org/openai/v1/".to_owned()),
            _ => Some(SECRET.to_owned()),
        })
        .unwrap();
        assert_eq!(proxy.url("/responses"), "https://gateway.example.org/openai/v1/responses");
        assert!(!proxy.is_default());
        assert!(endpoint(Provider::Gemini).is_default());
    }

    #[test]
    fn a_dated_id_of_the_model_asked_for_has_its_price_and_another_model_its_own() {
        assert!(dated_of("gpt-6.2-sol", "gpt-6.2-sol"));
        assert!(dated_of("gpt-6.2-sol-2026-11-01", "gpt-6.2-sol"));
        assert!(dated_of("grok-4.7-0309", "grok-4.7"));
        assert!(!dated_of("gpt-6.2-sol-pro", "gpt-6.2-sol"));
        assert!(!dated_of("gpt-6.2-solar", "gpt-6.2-sol"));
        assert!(!dated_of("gpt-6.2-sol-", "gpt-6.2-sol"));
    }

    #[test]
    fn a_compatible_server_s_models_are_read_from_its_list() {
        let listed = json!({"object": "list", "data": [{"id": "qwen3:32b"}, {"id": "llama3.3:70b"}]});
        let fake = Fake::new(vec![Ok(ok(&listed))]);
        assert_eq!(discover(&fake, &endpoint(Provider::Compatible)).unwrap(), ["qwen3:32b", "llama3.3:70b"]);
        assert_eq!(fake.sent()[0].url, "http://localhost:11434/v1/models");
        let empty = Fake::new(vec![]);
        let listed = discover(&empty, &endpoint(Provider::OpenAi)).unwrap();
        assert_eq!(listed[0], "gpt-6.2-sol", "a hosted provider's are the ones listed, and nothing is asked");
        assert!(empty.sent().is_empty());
    }
}
