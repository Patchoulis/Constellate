//! OpenAI's Responses API (`POST /v1/responses`), which xAI speaks too, as their references describe it
//! (developers.openai.com/api/reference/resources/responses, read 2026-11-27).
//!
//! - **Instructions** go first as a `developer` message rather than in `instructions`: on the newest models
//!   `instructions` are put after the input and never cached. The shared part follows in the user's message, the
//!   question after it, so the prefix that repeats is the one the provider caches by itself.
//! - **JSON** through `text.format` (`json_schema`), `strict` when the schema is one strict mode takes.
//! - **Effort** is `reasoning.effort`, for a model that has one.
//! - **`store: false`**: nothing a paper says is kept by the provider for later.
//! - **Usage**: `input_tokens` includes the tokens read from and written to the cache
//!   (`input_tokens_details.cached_tokens`, `cache_write_tokens`); writes are split by lifetime
//!   (`cache_write_tokens_details`: 30 minutes kept as the short lifetime, 12 hours as the long one).
//! - **Before the content**: `status: incomplete` (cut at `max_output_tokens`, or a content filter) and a `refusal`
//!   item are told first.

use serde::Deserialize;
use serde_json::{Value, json};

use super::chat::openai_style_failure;
use super::provider::Provider;
use super::{AskError, Asked, Form, NoAnswer, Outcome, Read, Reply, Request, schema};
use crate::usage::Tokens;

/// The request for `asked`.
pub(super) fn request(asked: &Asked<'_>) -> Request {
    let ask = &asked.ask;
    let mut content = Vec::new();
    if let Some(shared) = ask.shared {
        content.push(json!({"type": "input_text", "text": shared}));
    }
    content.push(json!({"type": "input_text", "text": asked.question}));
    let mut body = json!({
        "model": ask.model,
        "input": [
            {"role": "developer", "content": [{"type": "input_text", "text": ask.system}]},
            {"role": "user", "content": content},
        ],
        "max_output_tokens": ask.max_tokens,
        "store": false,
    });
    if let Some(effort) = asked.effort {
        body["reasoning"] = json!({"effort": effort.as_str()});
    }
    if let Form::Json(schema) = ask.form {
        let mut format = json!({"type": "json_schema", "name": "answer", "schema": schema});
        if schema::strict_ready(schema) {
            format["strict"] = json!(true);
        }
        body["text"] = json!({"format": format});
    }
    Request { path: "/responses".to_owned(), body }
}

/// The request that counts the prompt's tokens (`POST /v1/responses/input_tokens`), with nothing of the answer.
pub(super) fn count_request(asked: &Asked<'_>) -> Request {
    let Request { body, .. } = request(asked);
    Request {
        path: "/responses/input_tokens".to_owned(),
        body: json!({"model": body["model"], "input": body["input"]}),
    }
}

pub(super) fn read_count(body: &[u8]) -> Option<u64> {
    #[derive(Deserialize)]
    struct Counted {
        input_tokens: u64,
    }
    serde_json::from_slice::<Counted>(body).ok().map(|counted| counted.input_tokens)
}

#[derive(Deserialize)]
struct Response {
    model: Option<String>,
    status: Option<String>,
    incomplete_details: Option<Incomplete>,
    error: Option<Value>,
    #[serde(default)]
    output: Vec<Item>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Incomplete {
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Item {
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        content: Vec<Part>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Part {
    #[serde(rename = "output_text")]
    Text { text: String },
    #[serde(rename = "refusal")]
    Refusal,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    input_tokens_details: Option<InputDetails>,
}

#[derive(Deserialize)]
struct InputDetails {
    #[serde(default)]
    cached_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
    cache_write_tokens_details: Option<Lifetimes>,
}

#[derive(Deserialize)]
struct Lifetimes {
    #[serde(default)]
    ttl_30m_tokens: u64,
    #[serde(default)]
    ttl_12h_tokens: u64,
}

impl Usage {
    fn tokens(&self) -> Tokens {
        let details = self.input_tokens_details.as_ref();
        let read = details.map_or(0, |details| details.cached_tokens);
        let written = details.map_or(0, |details| details.cache_write_tokens);
        let (short, long) = match details.and_then(|details| details.cache_write_tokens_details.as_ref()) {
            Some(lifetimes) => (lifetimes.ttl_30m_tokens, lifetimes.ttl_12h_tokens),
            // without the split, every write is taken as the default lifetime, 30 minutes
            None => (written, 0),
        };
        Tokens {
            input: self.input_tokens.saturating_sub(read).saturating_sub(short + long),
            output: self.output_tokens,
            cache_read: read,
            cache_write_5m: short,
            cache_write_1h: long,
        }
    }
}

/// What an answer says; `None` when it is not shaped as one.
pub(super) fn read(body: &[u8]) -> Option<Read> {
    let response: Response = serde_json::from_slice(body).ok()?;
    let tokens = response.usage.as_ref().map(Usage::tokens);
    let outcome = match (response.status.as_deref(), response.incomplete_details.and_then(|details| details.reason)) {
        (Some("incomplete"), Some(reason)) if reason == "content_filter" => Outcome::Refused(Some(reason)),
        (Some("incomplete"), _) => Outcome::Cut,
        (Some("failed"), _) => Outcome::Failed(failed(response.error.as_ref())),
        _ => {
            let parts: Vec<Part> = response
                .output
                .into_iter()
                .flat_map(|item| match item {
                    Item::Message { content } => content,
                    Item::Other => Vec::new(),
                })
                .collect();
            if parts.iter().any(|part| matches!(part, Part::Refusal)) {
                Outcome::Refused(None)
            } else {
                let text = parts.into_iter().filter_map(|part| match part {
                    Part::Text { text } => Some(text),
                    Part::Refusal | Part::Other => None,
                });
                Outcome::Text(text.collect::<String>().trim().to_owned())
            }
        }
    };
    Some(Read { model: response.model, tokens, outcome })
}

/// A response the API gave up on after it started (`status: failed`), by its error's code.
fn failed(error: Option<&Value>) -> AskError {
    let code = error.and_then(|error| error.get("code")).and_then(Value::as_str).unwrap_or_default();
    let message = error.and_then(|error| error.get("message")).and_then(Value::as_str).unwrap_or_default();
    match code {
        "rate_limit_exceeded" => NoAnswer::RateLimited { retry_after: None }.into(),
        "server_error" | "" => NoAnswer::ServerError(500).into(),
        _ => NoAnswer::Rejected(message.chars().take(300).collect()).into(),
    }
}

/// What a request that failed with `reply.status` means: the same error body as Chat Completions.
pub(super) fn failure(reply: &Reply, model: &str, provider: Provider) -> AskError {
    openai_style_failure(reply, model, provider)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::language::provider::Answers;
    use crate::model::language::{Ask, Effort};

    fn schema() -> Value {
        json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"], "additionalProperties": false})
    }

    #[test]
    fn a_request_is_built_as_the_responses_reference_documents_it() {
        let schema = schema();
        let ask = Ask {
            purpose: "questions about a paper",
            model: "gpt-6.2-sol",
            system: "Answer from the paper only.",
            shared: Some("The paper's text."),
            question: "What does the paper claim?",
            form: Form::Json(&schema),
            effort: Effort::Low,
            max_tokens: 2_000,
        };
        let asked =
            Asked { ask, question: ask.question.to_owned(), effort: Some(Effort::Low), answers: Answers::Enforced };
        let Request { path, body } = request(&asked);
        assert_eq!(path, "/responses");
        assert_eq!(
            body,
            json!({
                "model": "gpt-6.2-sol",
                "input": [
                    {"role": "developer", "content": [{"type": "input_text", "text": "Answer from the paper only."}]},
                    {"role": "user", "content": [
                        {"type": "input_text", "text": "The paper's text."},
                        {"type": "input_text", "text": "What does the paper claim?"}
                    ]}
                ],
                "max_output_tokens": 2_000,
                "store": false,
                "reasoning": {"effort": "low"},
                "text": {"format": {"type": "json_schema", "name": "answer", "schema": schema, "strict": true}}
            })
        );
        let counted = count_request(&asked);
        assert_eq!(counted.path, "/responses/input_tokens");
        assert!(counted.body.get("max_output_tokens").is_none() && counted.body.get("text").is_none());
    }

    #[test]
    fn usage_is_split_into_what_was_read_written_and_sent_in_full() {
        let usage: Usage = serde_json::from_value(json!({
            "input_tokens": 50_000, "output_tokens": 900,
            "input_tokens_details": {"cached_tokens": 30_000, "cache_write_tokens": 15_000,
                "cache_write_tokens_details": {"ttl_30m_tokens": 15_000, "ttl_12h_tokens": 0}},
            "output_tokens_details": {"reasoning_tokens": 400}, "total_tokens": 50_900
        }))
        .unwrap();
        let tokens = usage.tokens();
        assert_eq!(
            (tokens.input, tokens.cache_read, tokens.cache_write_5m, tokens.output),
            (5_000, 30_000, 15_000, 900)
        );
        assert_eq!(tokens.prompt(), 50_000, "the prompt is counted once, however it was billed");
    }
}
