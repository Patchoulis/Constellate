//! What the features' route tests need to ask a provider other than the Claude API: an engine whose settings
//! choose OpenAI or an OpenAI-compatible server on this computer, a transport that answers as those providers' APIs
//! document (never the network: a real call costs money), and the routes asked as the interface asks them.
//!
//! Both providers are given the same model id ([`MODEL`]), so that a test can switch from one to the other and see
//! that an answer kept for one is not taken for the other's.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

use super::config::tests::STRONG;
use super::config::{Environment, Listen};
use super::routes::Engine;
use super::tests::served;
use crate::model::language::fake::{Sent, completion, message, ok, response};
use crate::model::language::{ApiKey, KeyHeader, Provider, Providers, Reply, Transport, TransportError};
use crate::settings::{self, Choices};

/// The model both providers are asked for: OpenAI lists it with a price; the server on this computer is told it
/// as a typed id, which costs no API fee there.
pub(crate) const MODEL: &str = "gpt-6.2-sol";
const AT: &str = "127.0.0.1:8765";
/// Where the compatible endpoint is: a server on this computer, which the fake transport stands in for.
pub(crate) const LOCAL: &str = "http://localhost:11434/v1";

/// The provider a test has chosen in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chosen {
    OpenAi,
    /// The OpenAI-compatible endpoint, on this computer.
    ThisComputer,
}

impl Chosen {
    pub(crate) fn provider(self) -> Provider {
        match self {
            Chosen::OpenAi => Provider::OpenAi,
            Chosen::ThisComputer => Provider::Compatible,
        }
    }

    /// Where its answers are asked for.
    pub(crate) fn url(self) -> String {
        match self {
            Chosen::OpenAi => "https://api.openai.com/v1/responses".to_owned(),
            Chosen::ThisComputer => format!("{LOCAL}/chat/completions"),
        }
    }

    /// How it is sent the key: in the header OpenAI reads, or none to a server that needs none.
    pub(crate) fn key_as(self) -> KeyHeader {
        match self {
            Chosen::OpenAi => KeyHeader::Bearer,
            Chosen::ThisComputer => KeyHeader::NotSent,
        }
    }
}

/// A provider's API as the tests need it: each request is answered with the text `answer` makes of its body,
/// in the shape of the API the request was sent to, and every request is kept.
pub(crate) struct Scripted {
    answer: Box<dyn Fn(&str) -> String + Send + Sync>,
    sent: Mutex<Vec<Sent>>,
}

impl Scripted {
    pub(crate) fn new(answer: impl Fn(&str) -> String + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { answer: Box::new(answer), sent: Mutex::new(Vec::new()) })
    }

    pub(crate) fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn answer(&self, url: &str, key: &ApiKey, sent_as: KeyHeader, body: &[u8]) -> Reply {
        let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let text = (self.answer)(&request.to_string());
        let model = request["model"].as_str().unwrap_or("").to_owned();
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push(Sent {
            url: url.to_owned(),
            headers: Vec::new(),
            body: request,
            key_given: !key.expose().is_empty(),
            key_as: sent_as,
        });
        if url.ends_with("/responses") {
            ok(&response(&model, &text, json!({"input_tokens": 2_000, "output_tokens": 300})))
        } else if url.ends_with("/chat/completions") {
            ok(&completion(&model, &text, json!({"prompt_tokens": 2_000, "completion_tokens": 300})))
        } else {
            ok(&message(&model, &text, "end_turn", json!({"input_tokens": 2_000, "output_tokens": 300})))
        }
    }
}

impl Transport for Scripted {
    fn post(
        &self,
        url: &str,
        key: &ApiKey,
        _headers: &[(&str, &str)],
        body: &[u8],
        _patience: Duration,
    ) -> Result<Reply, TransportError> {
        Ok(self.answer(url, key, KeyHeader::XApiKey, body))
    }

    fn post_keyed(
        &self,
        url: &str,
        key: &ApiKey,
        sent_as: KeyHeader,
        _headers: &[(&str, &str)],
        body: &[u8],
        _patience: Duration,
    ) -> Result<Reply, TransportError> {
        Ok(self.answer(url, key, sent_as, body))
    }

    fn wait(&self, _duration: Duration) {}
}

pub(crate) fn choices() -> Choices {
    let one = |name: &str| vec![name.to_owned()];
    Choices {
        embedding_models: one("Qwen/Qwen3-Embedding-0.6B"),
        clustering: one("spectral"),
        acceptance: one("all"),
        flags: one("llm_declared"),
        describe_models: one("claude-opus-5-5"),
    }
}

/// Choose `chosen` (with [`MODEL`]) in the settings of the data folder `data`.
pub(crate) fn choose(data: &Path, chosen: Chosen) {
    let changes: Map<String, Value> = json!({"describe_provider": chosen.provider().id(), "describe_model": MODEL})
        .as_object()
        .cloned()
        .unwrap_or_default();
    settings::save(&data.join("settings.json"), &changes, &choices()).unwrap_or_else(|why| panic!("{why}"));
}

/// The routes of an engine on `data` with `chosen` in its settings, an OpenAI key and a compatible server on this
/// computer set up, and `transport` for every request to them.
pub(crate) fn router(data: &Path, chosen: Chosen, transport: &Arc<Scripted>) -> Router {
    choose(data, chosen);
    let mut engine = Engine::new(data.to_path_buf(), choices());
    engine.providers = Providers::from_lookup(|name| match name {
        "OPENAI_API_KEY" => Some("test-key-not-real".to_owned()),
        "CONSTELLATE_MODEL_BASE_URL" => Some(LOCAL.to_owned()),
        _ => None,
    });
    engine.transport = Arc::clone(transport) as Arc<dyn Transport>;
    let listen = Listen::from_environment(&Environment::from_pairs([
        ("CONSTELLATE_TOKEN", STRONG),
        ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
    ]))
    .unwrap_or_else(|why| panic!("{why}"));
    served(&listen, AT, engine).0
}

/// Ask `router` as the interface does.
pub(crate) async fn call(router: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, AT)
        .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or(Body::empty(), |body| Body::from(body.to_string())))
        .unwrap_or_else(|why| panic!("{why}"));
    let response = router.clone().oneshot(request).await.unwrap_or_else(|why| panic!("{why}"));
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap_or_else(|why| panic!("{why}"));
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The status once job `id` has finished, as `/api/status` tells it.
pub(crate) async fn finished(router: &Router, id: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, status) = call(router, "GET", "/api/status", None).await;
        if status["id"] == *id && status["state"] != "running" {
            return status;
        }
        assert!(Instant::now() < deadline, "job {id} never finished: {status}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// What every feature's test checks of what was sent to `chosen`: its address, its key's header, the model id
/// asked for, and no Claude model.
pub(crate) fn sent_to(sent: &[Sent], chosen: Chosen) {
    assert!(!sent.is_empty(), "nothing was sent");
    for one in sent {
        assert_eq!(one.url, chosen.url(), "sent to {chosen:?}'s API");
        assert_eq!(one.key_as, chosen.key_as());
        assert_eq!(one.body["model"], MODEL, "{chosen:?}'s model id, never another provider's");
        assert!(!one.body.to_string().contains("claude-"), "no Claude model id is sent to {chosen:?}");
    }
}

/// What every feature's test checks of an estimate shown for `chosen` (a `BatchCost` or an `AskingCost`): its
/// provider and model, a price from OpenAI's list, or no fee on this computer and a worst case in tokens.
pub(crate) fn priced_for(cost: &Value, chosen: Chosen) {
    assert_eq!(cost["provider"], chosen.provider().id(), "{cost}");
    assert_eq!(cost["model"], MODEL, "{cost}");
    assert!(cost["most_tokens"].as_u64().is_some_and(|tokens| tokens > 0), "{cost}");
    match chosen {
        Chosen::OpenAi => {
            assert!(cost["usd"].as_f64().is_some_and(|usd| usd > 0.0), "priced from OpenAI's prices: {cost}");
            assert_eq!(cost["on_this_computer"], false);
            assert_eq!(cost["caching"], "automatic_writes_billed");
        }
        Chosen::ThisComputer => {
            assert_eq!((cost["usd"].clone(), cost["most_usd"].clone()), (Value::Null, Value::Null), "{cost}");
            assert_eq!(cost["on_this_computer"], true);
            assert_eq!(cost["caching"], "not_reported", "every question pays for the prefix in full");
        }
    }
}

/// The fields a request accepts the worst case shown in (dollars, tokens).
pub(crate) const ACCEPTED: (&str, &str) = ("accepted_usd", "accepted_tokens");
/// The fields a request names a budget of its own in (dollars, tokens).
pub(crate) const BUDGET: (&str, &str) = ("budget_usd", "budget_tokens");

/// What a request accepts of `cost`, in the fields `names`: its most in dollars, or in tokens when it has no price
/// (as the interface sends it), with the provider and model it was shown for.
pub(crate) fn accepting(cost: &Value, (usd, tokens): (&str, &str)) -> Value {
    let mut accepted = json!({"provider": cost["provider"], "model": cost["model"]});
    match cost["most_usd"].as_f64() {
        Some(most) => accepted[usd] = json!(most),
        None => accepted[tokens] = cost["most_tokens"].clone(),
    }
    accepted
}
