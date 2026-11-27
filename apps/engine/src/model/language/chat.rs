//! OpenAI's Chat Completions (`POST /chat/completions`), which most hosted providers and every common local server
//! (Ollama, LM Studio, llama.cpp, vLLM) speak, each with its quirks ([`Chat`]). The shapes are OpenAI's reference
//! (developers.openai.com/api/reference/resources/chat), and each provider's own page named in its row of the
//! provider table for what it does differently, read 2026-11-27.
//!
//! - **The prompt** is a `system` message and one `user` message: the shared part, then the question. Content is
//!   plain text rather than parts, which some local servers do not take; the shared part still comes first, so a
//!   provider that caches a repeated prefix by itself can.
//! - **JSON**: `response_format` with `json_schema` where the provider takes one, `json_object` where it only
//!   promises JSON, nothing where it promises neither; the schema is then in the question (`schema.rs`), and the
//!   answer is checked here either way.
//! - **Usage**: `prompt_tokens` includes what was read from the cache (`prompt_tokens_details.cached_tokens`, or
//!   DeepSeek's `prompt_cache_hit_tokens`) and written to it (`cache_write_tokens`, where reported). A server that
//!   reports no usage at all is counted by the client at the worst case the request allows.
//! - **Errors** come in several shapes (`{"error": {"message", "code"}}`, `{"error": "..."}`, `{"message"}`,
//!   `{"detail"}`, MiniMax's `base_resp`); what they mean is decided by the status first.

use serde::Deserialize;
use serde_json::{Value, json};

use super::provider::{Answers, Chat, EffortField, Provider, Strict};
use super::{
    AskError, Asked, Form, NoAnswer, Outcome, Read, Reply, Request, Setup, Unavailable, retry_after_header, schema,
};
use crate::usage::Tokens;

/// The request for `asked`, in the way `chat` takes it.
pub(super) fn request(asked: &Asked<'_>, chat: Chat) -> Request {
    let ask = &asked.ask;
    let user = match ask.shared {
        Some(shared) => format!("{shared}\n\n{}", asked.question),
        None => asked.question.clone(),
    };
    let mut body = json!({
        "model": ask.model,
        "messages": [{"role": "system", "content": ask.system}, {"role": "user", "content": user}],
        "max_tokens": ask.max_tokens,
    });
    if let Some(effort) = asked.effort {
        match chat.effort {
            EffortField::TopLevel => body["reasoning_effort"] = json!(effort.as_str()),
            EffortField::Object => body["reasoning"] = json!({"effort": effort.as_str()}),
        }
    }
    if let Form::Json(schema) = ask.form {
        match asked.answers {
            Answers::Enforced | Answers::Requested => {
                let mut format = json!({"name": "answer", "schema": schema});
                if chat.strict == Strict::Sent && schema::strict_ready(schema) {
                    format["strict"] = json!(true);
                }
                body["response_format"] = json!({"type": "json_schema", "json_schema": format});
            }
            Answers::JsonMode => body["response_format"] = json!({"type": "json_object"}),
            Answers::Prompted => {}
        }
    }
    Request { path: "/chat/completions".to_owned(), body }
}

#[derive(Deserialize)]
struct Completion {
    model: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<Usage>,
    error: Option<Value>,
    base_resp: Option<BaseResponse>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<Said>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Said {
    content: Option<Value>,
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    prompt_tokens_details: Option<PromptDetails>,
    prompt_cache_hit_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
}

/// MiniMax's status, beside or instead of an HTTP error.
#[derive(Deserialize)]
struct BaseResponse {
    #[serde(default)]
    status_code: i64,
    #[serde(default)]
    status_msg: String,
}

impl Usage {
    fn tokens(&self) -> Tokens {
        let details = self.prompt_tokens_details.as_ref();
        let read = self.prompt_cache_hit_tokens.or(details.map(|details| details.cached_tokens)).unwrap_or(0);
        let written = details.map_or(0, |details| details.cache_write_tokens);
        Tokens {
            input: self.prompt_tokens.saturating_sub(read).saturating_sub(written),
            output: self.completion_tokens,
            cache_read: read,
            cache_write_5m: written,
            cache_write_1h: 0,
        }
    }
}

/// The text of a message's content: a string, or the text parts of a list of them.
fn text_of(content: Option<Value>) -> String {
    match content {
        Some(Value::String(text)) => text,
        Some(Value::Array(parts)) => {
            parts.iter().filter_map(|part| part.get("text").and_then(Value::as_str)).collect::<String>()
        }
        _ => String::new(),
    }
}

/// What an answer says; `None` when it is not shaped as one.
pub(super) fn read(body: &[u8], model: &str, provider: Provider) -> Option<Read> {
    let completion: Completion = serde_json::from_slice(body).ok()?;
    let tokens = completion.usage.as_ref().map(Usage::tokens);
    if let Some(status) = completion.base_resp.as_ref().filter(|status| status.status_code != 0) {
        return Some(Read { model: completion.model, tokens, outcome: Outcome::Failed(minimax(status)) });
    }
    let Some(choice) = completion.choices.into_iter().next() else {
        // an error told in a successful answer (OpenRouter, when a model fails mid-way), or nothing at all
        let error = completion.error?;
        let outcome = Outcome::Failed(body_error(&error, model, provider));
        return Some(Read { model: completion.model, tokens, outcome });
    };
    let outcome = match choice.finish_reason.as_deref() {
        Some("length") => Outcome::Cut,
        Some("content_filter") => Outcome::Refused(Some("content_filter".to_owned())),
        _ => match choice.message {
            Some(Said { refusal: Some(refusal), .. }) if !refusal.trim().is_empty() => Outcome::Refused(None),
            Some(said) => Outcome::Text(text_of(said.content).trim().to_owned()),
            None => Outcome::Text(String::new()),
        },
    };
    Some(Read { model: completion.model, tokens, outcome })
}

fn minimax(status: &BaseResponse) -> AskError {
    match status.status_code {
        1004 => Unavailable::KeyRefused.into(),
        1008 => Unavailable::Billing.into(),
        1002 => NoAnswer::RateLimited { retry_after: None }.into(),
        _ => NoAnswer::Rejected(status.status_msg.chars().take(super::RAW_MESSAGE_CHARS).collect()).into(),
    }
}

/// An error told inside an answer's body: its `code` is the status it stands for when it is a number.
fn body_error(error: &Value, model: &str, provider: Provider) -> AskError {
    let status = error.get("code").and_then(Value::as_u64).and_then(|code| u16::try_from(code).ok()).unwrap_or(500);
    let body = json!({"error": error}).to_string().into_bytes();
    openai_style_failure(&Reply { status, retry_after: None, should_retry: None, body }, model, provider)
}

/// The error codes that mean the account cannot pay, as OpenAI documents them.
const BILLING: [&str; 3] = ["insufficient_quota", "billing_hard_limit_reached", "credit_balance_exhausted"];
/// The error codes that mean the account has reached a limit set on its spending.
const SPEND_LIMIT: [&str; 3] =
    ["organization_spend_limit_exceeded", "project_spend_limit_exceeded", "organization_usage_limit_exceeded"];

/// What a request that failed with `reply.status` means, for any provider whose errors are shaped like OpenAI's
/// (and the looser shapes of local servers).
pub(super) fn openai_style_failure(reply: &Reply, model: &str, provider: Provider) -> AskError {
    let body = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let error = body.get("error");
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let message = text(error.and_then(|error| error.get("message")))
        .or_else(|| text(error))
        .or_else(|| text(body.get("message")))
        .or_else(|| text(body.get("detail")))
        .unwrap_or_default();
    let code = text(error.and_then(|error| error.get("code")))
        .or_else(|| text(error.and_then(|error| error.get("type"))))
        .unwrap_or_default();
    match reply.status {
        401 => Unavailable::KeyRefused.into(),
        402 => Unavailable::Billing.into(),
        403 => Unavailable::NotAllowed.into(),
        // a server that answers 404 with no error of its own is not the API the address was meant to lead to
        404 if provider == Provider::Compatible && body.is_null() => {
            Unavailable::Setup(Setup::NoSuchEndpoint("CONSTELLATE_MODEL_BASE_URL")).into()
        }
        404 => Unavailable::UnknownModel(model.to_owned()).into(),
        408 => NoAnswer::ServerError(408).into(),
        413 => NoAnswer::TooLarge.into(),
        400 if code == "context_length_exceeded" => NoAnswer::TooLarge.into(),
        429 if BILLING.contains(&code.as_str()) => Unavailable::Billing.into(),
        429 if SPEND_LIMIT.contains(&code.as_str()) => Unavailable::SpendLimit.into(),
        429 => NoAnswer::RateLimited { retry_after: retry_after_header(reply) }.into(),
        498 | 503 | 529 => NoAnswer::Overloaded.into(),
        status if status >= 500 => NoAnswer::ServerError(status).into(),
        // what the request got wrong, in the provider's words (which never repeat the key), shortened
        _ => NoAnswer::Rejected(message.chars().take(super::RAW_MESSAGE_CHARS).collect()).into(),
    }
}

/// The ids a server lists (`GET /models`): `{"data": [{"id": ...}]}`.
pub(super) fn read_models(body: &[u8]) -> Option<Vec<String>> {
    #[derive(Deserialize)]
    struct Listed {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }
    let listed: Listed = serde_json::from_slice(body).ok()?;
    Some(listed.data.into_iter().map(|entry| entry.id).filter(|id| super::provider::plain_id(id)).collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::language::{Ask, Effort};

    fn schema() -> Value {
        json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"], "additionalProperties": false})
    }

    fn asked(schema: &Value, answers: Answers, effort: Option<Effort>) -> Asked<'_> {
        let ask = Ask {
            purpose: "questions about a paper",
            model: "kimi-k3.1-pro",
            system: "Answer from the paper only.",
            shared: Some("The paper's text."),
            question: "What does the paper claim?",
            form: Form::Json(schema),
            effort: Effort::Low,
            max_tokens: 2_000,
        };
        Asked { ask, question: ask.question.to_owned(), effort, answers }
    }

    #[test]
    fn a_request_is_built_as_chat_completions_documents_it() {
        let schema = schema();
        let plain = Chat { strict: Strict::Sent, effort: EffortField::TopLevel };
        let Request { path, body } = request(&asked(&schema, Answers::Requested, Some(Effort::Low)), plain);
        assert_eq!(path, "/chat/completions");
        assert_eq!(
            body,
            json!({
                "model": "kimi-k3.1-pro",
                "messages": [
                    {"role": "system", "content": "Answer from the paper only."},
                    {"role": "user", "content": "The paper's text.\n\nWhat does the paper claim?"}
                ],
                "max_tokens": 2_000,
                "reasoning_effort": "low",
                "response_format": {"type": "json_schema", "json_schema": {"name": "answer", "schema": schema, "strict": true}}
            })
        );
    }

    #[test]
    fn each_provider_s_quirks_change_only_their_part_of_the_request() {
        let schema = schema();
        let openrouter = Chat { strict: Strict::Sent, effort: EffortField::Object };
        let body = request(&asked(&schema, Answers::Requested, Some(Effort::High)), openrouter).body;
        assert_eq!(body["reasoning"], json!({"effort": "high"}));
        assert!(body.get("reasoning_effort").is_none());
        let fireworks = Chat { strict: Strict::Left, effort: EffortField::TopLevel };
        let body = request(&asked(&schema, Answers::Requested, None), fireworks).body;
        assert!(body["response_format"]["json_schema"].get("strict").is_none());
        assert!(body.get("reasoning_effort").is_none(), "no effort for a model that has none");
        let plain = Chat { strict: Strict::Sent, effort: EffortField::TopLevel };
        assert_eq!(
            request(&asked(&schema, Answers::JsonMode, None), plain).body["response_format"],
            json!({"type": "json_object"})
        );
        assert!(request(&asked(&schema, Answers::Prompted, None), plain).body.get("response_format").is_none());
    }

    #[test]
    fn an_answer_s_text_usage_and_stop_are_read() {
        let body = json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "kimi-k3.1-pro",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "{\"answer\": \"x\"}", "refusal": null}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12_000, "completion_tokens": 300, "total_tokens": 12_300,
                "prompt_tokens_details": {"cached_tokens": 10_000, "cache_write_tokens": 0}}
        });
        let read = read(body.to_string().as_bytes(), "kimi-k3.1-pro", Provider::Moonshot).unwrap();
        assert!(matches!(read.outcome, Outcome::Text(ref text) if text == "{\"answer\": \"x\"}"));
        let tokens = read.tokens.unwrap();
        assert_eq!((tokens.input, tokens.cache_read, tokens.output), (2_000, 10_000, 300));

        let deepseek: Usage = serde_json::from_value(json!({
            "prompt_tokens": 5_000, "completion_tokens": 10, "prompt_cache_hit_tokens": 4_000, "prompt_cache_miss_tokens": 1_000
        }))
        .unwrap();
        assert_eq!((deepseek.tokens().input, deepseek.tokens().cache_read), (1_000, 4_000));
    }

    #[test]
    fn a_cut_a_filtered_and_a_refused_answer_are_told_before_their_content() {
        let choice = |content: Value, refusal: Value, finish: &str| json!({"model": "m", "choices": [{"message": {"content": content, "refusal": refusal}, "finish_reason": finish}]});
        let outcome = |body: Value| read(body.to_string().as_bytes(), "m", Provider::Groq).unwrap().outcome;
        assert!(matches!(outcome(choice(json!("{\"answer\": \"ha"), Value::Null, "length")), Outcome::Cut));
        assert!(matches!(outcome(choice(json!("x"), Value::Null, "content_filter")), Outcome::Refused(Some(_))));
        assert!(matches!(
            outcome(choice(Value::Null, json!("I can't help with that."), "stop")),
            Outcome::Refused(None)
        ));
    }

    #[test]
    fn errors_are_told_apart_into_stop_asking_and_try_again() {
        let reply = |status: u16, body: Value| Reply {
            status,
            retry_after: Some("3".to_owned()),
            should_retry: None,
            body: body.to_string().into_bytes(),
        };
        let openai = |message: &str, code: &str| json!({"error": {"message": message, "type": "x", "param": null, "code": code}});
        let cases: Vec<(Reply, AskError)> = vec![
            (reply(401, openai("Incorrect API key provided", "invalid_api_key")), Unavailable::KeyRefused.into()),
            (
                reply(402, json!({"error": {"code": 402, "message": "Insufficient credits"}})),
                Unavailable::Billing.into(),
            ),
            (
                reply(404, openai("The model does not exist", "model_not_found")),
                Unavailable::UnknownModel("m".to_owned()).into(),
            ),
            (reply(404, json!({"error": "model 'qwen9' not found"})), Unavailable::UnknownModel("m".to_owned()).into()),
            (reply(429, openai("You exceeded your current quota", "insufficient_quota")), Unavailable::Billing.into()),
            (reply(429, openai("Spend limit", "project_spend_limit_exceeded")), Unavailable::SpendLimit.into()),
            (
                reply(429, openai("Rate limit reached", "rate_limit_exceeded")),
                NoAnswer::RateLimited { retry_after: Some(std::time::Duration::from_secs(3)) }.into(),
            ),
            (reply(503, openai("Slow down", "server_is_overloaded")), NoAnswer::Overloaded.into()),
            (reply(500, openai("Server error", "server_error")), NoAnswer::ServerError(500).into()),
            (
                reply(422, json!({"detail": [{"loc": ["body"], "msg": "bad", "type": "x"}]})),
                NoAnswer::Rejected(String::new()).into(),
            ),
            (
                reply(400, openai("Unsupported parameter: 'reasoning_effort'", "unsupported_parameter")),
                NoAnswer::Rejected("Unsupported parameter: 'reasoning_effort'".to_owned()).into(),
            ),
        ];
        for (reply, expected) in cases {
            assert_eq!(openai_style_failure(&reply, "m", Provider::OpenRouter), expected, "{}", reply.status);
        }
        let empty = Reply { status: 404, retry_after: None, should_retry: None, body: b"404 page not found".to_vec() };
        assert_eq!(
            openai_style_failure(&empty, "m", Provider::Compatible),
            Unavailable::Setup(Setup::NoSuchEndpoint("CONSTELLATE_MODEL_BASE_URL")).into()
        );
        let minimax = json!({"model": "MiniMax-M3", "choices": [], "base_resp": {"status_code": 1008, "status_msg": "insufficient balance"}});
        let read = read(minimax.to_string().as_bytes(), "MiniMax-M3", Provider::MiniMax).unwrap();
        assert!(matches!(read.outcome, Outcome::Failed(AskError::Unavailable(Unavailable::Billing))));
        let midway = json!({"choices": [], "error": {"code": 502, "message": "the model is down"}});
        let read = super::read(midway.to_string().as_bytes(), "m", Provider::OpenRouter).unwrap();
        assert!(matches!(read.outcome, Outcome::Failed(AskError::NoAnswer(NoAnswer::ServerError(502)))));
    }

    #[test]
    fn a_server_s_models_are_read_from_its_list() {
        let listed = json!({"object": "list", "data": [{"id": "qwen3:32b", "object": "model", "owned_by": "library"}, {"id": "bad id"}]});
        assert_eq!(read_models(listed.to_string().as_bytes()).unwrap(), ["qwen3:32b"]);
        assert!(read_models(b"not json").is_none());
    }
}
