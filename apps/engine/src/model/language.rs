//! The language model: questions to the Claude API over HTTPS, answers as JSON, every call priced and recorded.
//!
//! There is no official Rust SDK, so this is the Messages API over plain HTTP (`ureq`, as the sources use), as the
//! API's own reference describes it for raw HTTP: `POST /v1/messages` with `x-api-key`, `anthropic-version` and a
//! JSON body. What it does, and why:
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
//!   what its caller is willing to spend is not started. [`estimate`] says what a batch is likely to cost before
//!   it is started.
//! - **The key** is read from `ANTHROPIC_API_KEY` only, the address from `ANTHROPIC_BASE_URL` when it is set.
//!   Neither is ever logged, shown in a `Debug`, or put in an error.
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

/// Where requests go and with what key.
#[derive(Clone)]
pub struct Endpoint {
    key: ApiKey,
    base: Url,
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
        let base = Url::parse(&base).map_err(|_| Unavailable::BadEndpoint)?;
        let local = matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        // the key goes in a header: never over plain HTTP to another computer
        if !(base.scheme() == "https" || (base.scheme() == "http" && local)) {
            return Err(Unavailable::BadEndpoint);
        }
        Ok(Self { key: ApiKey(key), base })
    }

    /// Is the endpoint Anthropic's own, or another the user set?
    pub fn is_default(&self) -> bool {
        self.base.as_str().trim_end_matches('/') == DEFAULT_BASE
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.as_str().trim_end_matches('/'))
    }
}

/// How hard the model thinks before it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    /// The messages and settings of the request, without what only `messages` takes.
    fn body(&self) -> Value {
        let cached = json!({"type": "ephemeral"});
        let mut content = Vec::new();
        if let Some(shared) = self.shared {
            content.push(json!({"type": "text", "text": shared, "cache_control": cached}));
        }
        content.push(json!({"type": "text", "text": self.question}));
        let mut output = json!({"effort": self.effort.as_str()});
        if let Form::Json(schema) = self.form {
            output["format"] = json!({"type": "json_schema", "schema": schema});
        }
        json!({
            "model": self.model,
            "system": [{"type": "text", "text": self.system, "cache_control": cached}],
            "messages": [{"role": "user", "content": content}],
            "thinking": {"type": "adaptive"},
            "output_config": output,
        })
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("the Claude API could not be reached")]
    Unreachable,
    #[error("the Claude API took too long to answer")]
    TimedOut,
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
        let failed = |error: ureq::Error| match error {
            ureq::Error::Timeout(_) => TransportError::TimedOut,
            _ => TransportError::Unreachable,
        };
        let mut request = self.agent.post(url).config().timeout_global(Some(patience)).build();
        request = request.header("x-api-key", key.expose()).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut response = request.send(body).map_err(failed)?;
        let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
        let (retry_after, should_retry) = (header("retry-after"), header("x-should-retry"));
        let body = response.body_mut().with_config().limit(LARGEST_ANSWER).read_to_vec().map_err(failed)?;
        Ok(Reply { status: response.status().as_u16(), retry_after, should_retry, body })
    }
}

/// Where each call is recorded for the ledger.
pub trait Recorder: Send + Sync {
    fn record(&self, call: Call);
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
        let signed = |count: u64| i64::try_from(count).unwrap_or(i64::MAX);
        // the ledger keeps whole millionths of a dollar
        let micros = call.cost().map(|cost| signed(cost.picodollars().div_ceil(1_000_000)));
        let kept = self.record_call(&crate::library::ledger::Call {
            model: &call.model,
            purpose: &call.purpose,
            input_tokens: signed(call.tokens.input),
            output_tokens: signed(call.tokens.output),
            cache_read_tokens: signed(call.tokens.cache_read),
            cache_write_tokens: signed(call.tokens.cache_written()),
            price_micros: micros,
        });
        if let Err(error) = kept {
            eprintln!("constellate: a call to {} for {} could not be recorded: {error}", call.model, call.purpose);
        }
    }
}

/// The Claude API cannot be used at all: asking again will not help until something is changed.
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
    #[error("The Claude API account has a billing problem")]
    Billing,
    #[error("The Claude API account has reached its spend limit")]
    SpendLimit,
    #[error("The model {0} is not available to this account")]
    UnknownModel(String),
    #[error("ANTHROPIC_BASE_URL does not lead to the Claude API")]
    NoSuchEndpoint,
}

/// This request gave nothing usable; the next one may work.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoAnswer {
    #[error("The Claude API is limiting requests; try again later")]
    RateLimited { retry_after: Option<Duration> },
    #[error("The Claude API is overloaded; try again later")]
    Overloaded,
    #[error("The Claude API failed ({0})")]
    ServerError(u16),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("The request is too large for the Claude API")]
    TooLarge,
    #[error("The Claude API refused the request: {0}")]
    Rejected(String),
    #[error("The model declined to answer")]
    Refused { category: Option<String> },
    #[error("The answer was cut off before it was finished")]
    Cut,
    #[error("The answer was empty")]
    Empty,
    #[error("The answer was not the JSON asked for: {0}")]
    NotJson(String),
    #[error("The Claude API answered in a way this does not understand")]
    Unexpected,
}

/// A call not started, because it could take the batch past its budget.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OverBudget {
    #[error("This would take {needed} tokens more than the {limit} allowed ({spent} spent)")]
    Tokens { limit: u64, spent: u64, needed: u64 },
    #[error("This could cost more than the budget allows")]
    Dollars { limit: Usd, spent: Usd, needed: Usd },
    #[error("The price of {0} is not known, so a budget in dollars cannot be kept")]
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
/// starts and is refused if that would pass the limit; when it ends, what it really took replaces what it held.
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

impl Budget {
    pub fn new(limit: Limit) -> Self {
        Self { limit, spending: Mutex::new(Spending::default()) }
    }

    /// What the batch's calls have taken so far: tokens, and dollars (of priced calls).
    pub fn spent(&self) -> (u64, Usd) {
        let spending = self.lock();
        (spending.tokens, spending.usd)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Spending> {
        self.spending.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn hold(&self, model: &str, worst: &Tokens) -> Result<Held, OverBudget> {
        let mut spending = self.lock();
        let held = match self.limit {
            Limit::Tokens(limit) => {
                let needed = worst.all();
                let taken = spending.tokens.saturating_add(spending.held_tokens);
                if taken.saturating_add(needed) > limit {
                    return Err(OverBudget::Tokens { limit, spent: spending.tokens, needed });
                }
                Held { tokens: needed, usd: Usd::ZERO }
            }
            Limit::Dollars(limit) => {
                let needed = price_of(model, worst).ok_or_else(|| OverBudget::Unpriced(model.to_owned()))?;
                if spending.usd + spending.held_usd + needed > limit {
                    return Err(OverBudget::Dollars { limit, spent: spending.usd, needed });
                }
                Held { tokens: 0, usd: needed }
            }
        };
        spending.held_tokens = spending.held_tokens.saturating_add(held.tokens);
        spending.held_usd =
            Usd::from_picodollars(spending.held_usd.picodollars().saturating_add(held.usd.picodollars()));
        Ok(held)
    }

    fn settle(&self, held: Held, took: &Tokens, cost: Option<Usd>) {
        let mut spending = self.lock();
        spending.held_tokens = spending.held_tokens.saturating_sub(held.tokens);
        spending.held_usd =
            Usd::from_picodollars(spending.held_usd.picodollars().saturating_sub(held.usd.picodollars()));
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

/// What a batch is likely to cost, before it is started: what the interface shows as "about N dollars".
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Estimate {
    pub model: String,
    pub calls: u64,
    /// Likely tokens, by how they are billed.
    pub tokens: Tokens,
    /// Likely cost; `None` when the model's price is not known.
    pub usd: Option<Usd>,
    /// If every answer took `max_tokens` and nothing was read from the cache.
    pub most_usd: Option<Usd>,
    /// How the prompt's tokens were reckoned: it is an estimate either way.
    pub counting: Counting,
}

/// What `planned` calls to `model` are likely to cost, and at most. Prompt caching is counted as it works when
/// the questions over one shared part are asked one after the other within five minutes: the first writes the
/// cache, the others read it.
pub fn estimate(model: &str, planned: &[Planned], counting: Counting) -> Estimate {
    let mut likely = Tokens::default();
    let mut likely_usd = Some(Usd::ZERO);
    let mut most_usd = Some(Usd::ZERO);
    let mut calls = 0u64;
    let add = |total: &mut Option<Usd>, tokens: &Tokens| {
        *total = total.and_then(|sum| price_of(model, tokens).map(|cost| sum + cost));
    };
    for plan in planned {
        let shared = counting.tokens(plan.shared_chars);
        let question = counting.tokens(plan.question_chars);
        for asked in 0..plan.questions {
            let call = Tokens {
                input: question,
                output: u64::from(plan.answer_tokens),
                cache_read: if asked == 0 { 0 } else { shared },
                cache_write_5m: if asked == 0 { shared } else { 0 },
                cache_write_1h: 0,
            };
            add(&mut likely_usd, &call);
            likely = likely + call;
            let worst = Tokens { output: u64::from(plan.max_tokens), cache_read: 0, cache_write_5m: shared, ..call };
            add(&mut most_usd, &worst);
            calls += 1;
        }
    }
    Estimate { model: model.to_owned(), calls, tokens: likely, usd: likely_usd, most_usd, counting }
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

/// An answer as the API documents it, only what is read of it.
#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    stop_reason: Option<String>,
    stop_details: Option<StopDetails>,
    #[serde(default)]
    content: Vec<Block>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct StopDetails {
    category: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Block {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Clone, Deserialize)]
struct Usage {
    model: Option<String>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
    /// One per model that worked on the answer, when a fallback did.
    iterations: Option<Vec<Usage>>,
}

#[derive(Clone, Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

impl Usage {
    fn tokens(&self) -> Tokens {
        let (write_5m, write_1h) = match &self.cache_creation {
            Some(by_lifetime) => (by_lifetime.ephemeral_5m_input_tokens, by_lifetime.ephemeral_1h_input_tokens),
            // without the breakdown every write is taken as a five-minute one, the lifetime asked for here
            None => (self.cache_creation_input_tokens.unwrap_or(0), 0),
        };
        Tokens {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_input_tokens.unwrap_or(0),
            cache_write_5m: write_5m,
            cache_write_1h: write_1h,
        }
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<String>,
    details: Option<ErrorCode>,
}

#[derive(Deserialize)]
struct ErrorCode {
    error_code: Option<String>,
}

/// What a request that failed with `reply.status` means.
fn failure(reply: &Reply, model: &str) -> AskError {
    let detail = serde_json::from_slice::<ErrorBody>(&reply.body).ok().and_then(|body| body.error);
    let message = detail.as_ref().and_then(|detail| detail.message.clone()).unwrap_or_default();
    let kind = detail.as_ref().and_then(|detail| detail.kind.clone()).unwrap_or_default();
    let code = detail.and_then(|detail| detail.details).and_then(|details| details.error_code);
    let retry_after = reply
        .retry_after
        .as_deref()
        .and_then(|seconds| seconds.trim().parse::<f64>().ok())
        // a header that is negative, not a number, or absurd is no wait at all (and must not panic)
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds.min(LONGEST_WAIT.as_secs_f64() + 1.0)).ok());
    match reply.status {
        401 => Unavailable::KeyRefused.into(),
        402 => Unavailable::Billing.into(),
        403 => Unavailable::NotAllowed.into(),
        404 if message.starts_with("model:") => Unavailable::UnknownModel(model.to_owned()).into(),
        404 => Unavailable::NoSuchEndpoint.into(),
        413 => NoAnswer::TooLarge.into(),
        429 if code.as_deref() == Some("enforced_spend_limit_reached") => Unavailable::SpendLimit.into(),
        429 => NoAnswer::RateLimited { retry_after }.into(),
        529 => NoAnswer::Overloaded.into(),
        _ if kind == "overloaded_error" => NoAnswer::Overloaded.into(),
        status if status >= 500 => NoAnswer::ServerError(status).into(),
        // what the request got wrong, in the API's words (it never repeats the key), shortened
        _ => NoAnswer::Rejected(message.chars().take(300).collect()).into(),
    }
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
        let mut attempt = 0;
        loop {
            let (error, says) = match self.transport.post(&url, &self.endpoint.key, headers, body, PATIENCE) {
                Ok(reply) if (200..300).contains(&reply.status) => return Ok(reply),
                Ok(reply) => (failure(&reply, model), reply.should_retry),
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
    pub fn ask(&self, ask: &Ask<'_>, budget: &Budget) -> Result<Answered<String>, AskError> {
        self.stopped()?;
        let held = budget.hold(ask.model, &self.worst(ask))?;
        let mut body = ask.body();
        body["max_tokens"] = json!(ask.max_tokens);
        if model(ask.model).is_some_and(|model| model.fallback == Fallback::ServerSide) {
            body["fallbacks"] = json!("default");
        }
        let headers = self.headers(ask.model, true);
        let sent = self.send("/v1/messages", ask.model, &headers, body.to_string().as_bytes());
        let reply = match sent {
            Ok(reply) => reply,
            Err(error) => {
                budget.settle(held, &Tokens::default(), Some(Usd::ZERO)); // a failed request is not billed
                return Err(error);
            }
        };
        let Ok(message) = serde_json::from_slice::<Message>(&reply.body) else {
            // an answer was given and so probably billed, but says nothing of what it took: its worst case is
            // recorded and counted, rather than nothing
            let worst = self.worst(ask);
            let call = Call { model: ask.model.to_owned(), purpose: ask.purpose.to_owned(), tokens: worst };
            budget.settle(held, &worst, call.cost());
            self.recorder.record(call);
            return Err(NoAnswer::Unexpected.into());
        };
        // paid for whether or not the answer is usable: recorded before it is looked at
        let (answered, tokens, cost) = self.record(ask.purpose, ask.model, &message);
        budget.settle(held, &tokens, cost);
        match message.stop_reason.as_deref() {
            Some("refusal") => {
                let category = message.stop_details.and_then(|details| details.category);
                return Err(NoAnswer::Refused { category }.into());
            }
            Some("max_tokens") => return Err(NoAnswer::Cut.into()),
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

    /// The prompt tokens of `ask`, counted by the API's token-counting endpoint (no answer is written).
    pub fn count_tokens(&self, ask: &Ask<'_>) -> Result<u64, AskError> {
        #[derive(Deserialize)]
        struct Counted {
            input_tokens: u64,
        }
        let headers = self.headers(ask.model, false);
        let reply = self.send("/v1/messages/count_tokens", ask.model, &headers, ask.body().to_string().as_bytes())?;
        let counted: Counted = serde_json::from_slice(&reply.body).map_err(|_| NoAnswer::Unexpected)?;
        Ok(counted.input_tokens)
    }

    /// Measure how many characters a token is for prompts like `ask`, with one token count, and reckon later
    /// prompts with it: what makes [`estimate`] and the budget's worst cases closer to what calls will cost.
    pub fn calibrate(&self, ask: &Ask<'_>) -> Result<Counting, AskError> {
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

    impl Transport for Fake {
        fn post(
            &self,
            url: &str,
            key: &ApiKey,
            headers: &[(&str, &str)],
            body: &[u8],
            _patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Sent {
                url: url.to_owned(),
                headers: headers.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect(),
                body: serde_json::from_slice(body).unwrap_or(Value::Null),
                key_given: !key.expose().is_empty(),
            });
            self.replies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Err(TransportError::Unreachable))
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
        let estimated = estimate("claude-opus-5-5", &vec![plan; 739], Counting::Measured { chars_per_token: 4.0 });
        assert_eq!(estimated.calls, 739 * 3);
        assert_eq!(estimated.tokens.cache_write_5m, 739 * 10_000);
        assert_eq!(estimated.tokens.cache_read, 739 * 2 * 10_000);
        let likely = estimated.usd.unwrap();
        let most = estimated.most_usd.unwrap();
        assert!(likely < most);
        // per paper: 10000 written at $5, 20000 read at $0.20, 225 question tokens at $4, 2400 answer tokens at $20
        let per_paper = 10_000 * 5_000_000 + 20_000 * 200_000 + 225 * 4_000_000 + 2_400 * 20_000_000;
        assert_eq!(likely, Usd::from_picodollars(739 * per_paper));
        assert_eq!(estimate("claude-unknown-1", &[plan], Counting::ASSUMED).usd, None);
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
