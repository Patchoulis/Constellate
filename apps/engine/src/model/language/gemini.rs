//! Google's Gemini API (`POST /v1beta/models/{model}:generateContent`, key in `x-goog-api-key`), as its reference
//! describes it (aistudio.google.com/api-reference/generate-content, read 2026-11-27). Google now calls
//! `generateContent` its older API beside a newer one (Interactions); it is still served and documented, and it is
//! the one whose shapes are fully described.
//!
//! - **The prompt**: `systemInstruction`, then one user turn with the shared part and the question as two parts.
//!   Gemini caches a repeated prefix by itself (implicit caching); there is no write charge.
//! - **JSON**: `responseMimeType: application/json` with the schema as `responseJsonSchema` (JSON Schema).
//! - **Effort**: `thinkingConfig.thinkingLevel`, for a model that has one (Gemini 3 and later).
//! - **Before the content**: `promptFeedback.blockReason` (the prompt itself was blocked: no candidates), then the
//!   candidate's `finishReason` (`MAX_TOKENS` is cut; a safety reason is a refusal). Thought parts are skipped.
//! - **Usage**: `promptTokenCount` includes `cachedContentTokenCount`; thinking (`thoughtsTokenCount`) is billed as
//!   output.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{AskError, Asked, Form, NoAnswer, Outcome, Read, Reply, Request, Unavailable, retry_after_header};
use crate::usage::Tokens;

fn generation(asked: &Asked<'_>) -> Value {
    let ask = &asked.ask;
    let mut config = json!({"maxOutputTokens": ask.max_tokens});
    if let Form::Json(schema) = ask.form {
        config["responseMimeType"] = json!("application/json");
        config["responseJsonSchema"] = schema.clone();
    }
    if let Some(effort) = asked.effort {
        config["thinkingConfig"] = json!({"thinkingLevel": effort.as_str()});
    }
    config
}

fn prompt(asked: &Asked<'_>) -> (Value, Value) {
    let ask = &asked.ask;
    let mut parts = Vec::new();
    if let Some(shared) = ask.shared {
        parts.push(json!({"text": shared}));
    }
    parts.push(json!({"text": asked.question}));
    (json!({"parts": [{"text": ask.system}]}), json!([{"role": "user", "parts": parts}]))
}

/// The request for `asked`. The model is in the path: a typed id is checked to be plain before it gets here.
pub(super) fn request(asked: &Asked<'_>) -> Request {
    let (system, contents) = prompt(asked);
    let body = json!({"systemInstruction": system, "contents": contents, "generationConfig": generation(asked)});
    Request { path: format!("/v1beta/models/{}:generateContent", asked.ask.model), body }
}

/// The request that counts the prompt's tokens, the whole prompt as `generateContent` would take it.
pub(super) fn count_request(asked: &Asked<'_>) -> Request {
    let (system, contents) = prompt(asked);
    let model = format!("models/{}", asked.ask.model);
    let body = json!({"generateContentRequest": {"model": model, "systemInstruction": system, "contents": contents}});
    Request { path: format!("/v1beta/models/{}:countTokens", asked.ask.model), body }
}

pub(super) fn read_count(body: &[u8]) -> Option<u64> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Counted {
        total_tokens: u64,
    }
    serde_json::from_slice::<Counted>(body).ok().map(|counted| counted.total_tokens)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Generated {
    #[serde(default)]
    candidates: Vec<Candidate>,
    prompt_feedback: Option<Feedback>,
    usage_metadata: Option<Usage>,
    model_version: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<Content>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    text: Option<String>,
    #[serde(default)]
    thought: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feedback {
    block_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    cached_content_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    #[serde(default)]
    thoughts_token_count: u64,
    #[serde(default)]
    tool_use_prompt_token_count: u64,
}

impl Usage {
    fn tokens(&self) -> Tokens {
        Tokens {
            input: self
                .prompt_token_count
                .saturating_sub(self.cached_content_token_count)
                .saturating_add(self.tool_use_prompt_token_count),
            output: self.candidates_token_count.saturating_add(self.thoughts_token_count),
            cache_read: self.cached_content_token_count,
            ..Tokens::default()
        }
    }
}

/// The finish reasons that mean the model, or Google's filters, declined: a refusal, not an answer.
const DECLINED: [&str; 8] = [
    "SAFETY",
    "RECITATION",
    "LANGUAGE",
    "BLOCKLIST",
    "PROHIBITED_CONTENT",
    "SPII",
    "IMAGE_SAFETY",
    "IMAGE_PROHIBITED_CONTENT",
];

/// What an answer says; `None` when it is not shaped as one.
pub(super) fn read(body: &[u8]) -> Option<Read> {
    let generated: Generated = serde_json::from_slice(body).ok()?;
    let tokens = generated.usage_metadata.as_ref().map(Usage::tokens);
    let model = generated.model_version;
    if let Some(reason) = generated.prompt_feedback.and_then(|feedback| feedback.block_reason) {
        return Some(Read { model, tokens, outcome: Outcome::Refused(Some(reason)) });
    }
    let candidate = generated.candidates.into_iter().next()?;
    let outcome = match candidate.finish_reason.as_deref() {
        Some("MAX_TOKENS") => Outcome::Cut,
        Some(reason) if DECLINED.contains(&reason) => Outcome::Refused(Some(reason.to_owned())),
        Some("STOP") | None => {
            let parts = candidate.content.map(|content| content.parts).unwrap_or_default();
            let text: String = parts.into_iter().filter(|part| !part.thought).filter_map(|part| part.text).collect();
            Outcome::Text(text.trim().to_owned())
        }
        // MALFORMED_RESPONSE, OTHER and the like: something went wrong on Google's side
        Some(_) => Outcome::Failed(NoAnswer::Unexpected.into()),
    };
    Some(Read { model, tokens, outcome })
}

/// How long Google's `RetryInfo` says to wait ("30s"), when an error carries one.
fn retry_info(error: &Value) -> Option<std::time::Duration> {
    let details = error.get("details")?.as_array()?;
    let delay = details.iter().find_map(|detail| detail.get("retryDelay").and_then(Value::as_str))?;
    let seconds =
        delay.strip_suffix('s')?.parse::<f64>().ok().filter(|seconds| seconds.is_finite() && *seconds >= 0.0)?;
    std::time::Duration::try_from_secs_f64(seconds.min(super::LONGEST_WAIT.as_secs_f64() + 1.0)).ok()
}

/// What a request that failed with `reply.status` means: `{"error": {"code", "message", "status", "details"}}`.
pub(super) fn failure(reply: &Reply, model: &str) -> AskError {
    let body = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let error = body.get("error").cloned().unwrap_or(Value::Null);
    let status = error.get("status").and_then(Value::as_str).unwrap_or_default();
    let message = error.get("message").and_then(Value::as_str).unwrap_or_default();
    let reason = |wanted: &str| {
        error
            .get("details")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|detail| detail.get("reason").and_then(Value::as_str) == Some(wanted))
    };
    match reply.status {
        400 if reason("API_KEY_INVALID") => Unavailable::KeyRefused.into(),
        // the free tier is not offered where the request came from and billing is not set up
        400 if status == "FAILED_PRECONDITION" => Unavailable::Billing.into(),
        401 => Unavailable::KeyRefused.into(),
        402 => Unavailable::Billing.into(),
        403 => Unavailable::NotAllowed.into(),
        404 => Unavailable::UnknownModel(model.to_owned()).into(),
        413 => NoAnswer::TooLarge.into(),
        429 => NoAnswer::RateLimited { retry_after: retry_info(&error).or_else(|| retry_after_header(reply)) }.into(),
        503 => NoAnswer::Overloaded.into(),
        code if code >= 500 => NoAnswer::ServerError(code).into(),
        _ => NoAnswer::Rejected(message.chars().take(300).collect()).into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::language::provider::Answers;
    use crate::model::language::{Ask, Effort};

    #[test]
    fn a_request_is_built_as_generate_content_documents_it() {
        let schema = json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"], "additionalProperties": false});
        let ask = Ask {
            purpose: "questions about a paper",
            model: "gemini-4-carbon",
            system: "Answer from the paper only.",
            shared: Some("The paper's text."),
            question: "What does the paper claim?",
            form: Form::Json(&schema),
            effort: Effort::High,
            max_tokens: 2_000,
        };
        let asked =
            Asked { ask, question: ask.question.to_owned(), effort: Some(Effort::High), answers: Answers::Enforced };
        let Request { path, body } = request(&asked);
        assert_eq!(path, "/v1beta/models/gemini-4-carbon:generateContent");
        assert_eq!(
            body,
            json!({
                "systemInstruction": {"parts": [{"text": "Answer from the paper only."}]},
                "contents": [{"role": "user", "parts": [{"text": "The paper's text."}, {"text": "What does the paper claim?"}]}],
                "generationConfig": {
                    "maxOutputTokens": 2_000,
                    "responseMimeType": "application/json",
                    "responseJsonSchema": schema,
                    "thinkingConfig": {"thinkingLevel": "high"}
                }
            })
        );
        let counted = count_request(&asked);
        assert_eq!(counted.path, "/v1beta/models/gemini-4-carbon:countTokens");
        assert_eq!(counted.body["generateContentRequest"]["model"], "models/gemini-4-carbon");
        assert_eq!(read_count(br#"{"totalTokens": 31, "cachedContentTokenCount": 0}"#), Some(31));
    }

    #[test]
    fn an_answer_skips_thoughts_and_its_usage_counts_thinking_as_output() {
        let body = json!({
            "candidates": [{"content": {"role": "model", "parts": [
                {"text": "Weighing it.", "thought": true},
                {"text": "{\"answer\": \"it merges\"}"}
            ]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 9_000, "cachedContentTokenCount": 8_000, "candidatesTokenCount": 20,
                "thoughtsTokenCount": 180, "totalTokenCount": 9_200},
            "modelVersion": "gemini-4-carbon"
        });
        let read = read(body.to_string().as_bytes()).unwrap();
        assert!(matches!(read.outcome, Outcome::Text(ref text) if text == "{\"answer\": \"it merges\"}"));
        let tokens = read.tokens.unwrap();
        assert_eq!((tokens.input, tokens.cache_read, tokens.output), (1_000, 8_000, 200));
        assert_eq!(read.model.as_deref(), Some("gemini-4-carbon"));
    }

    #[test]
    fn a_blocked_prompt_a_safety_stop_and_a_cut_are_told_before_any_content() {
        let blocked =
            json!({"promptFeedback": {"blockReason": "PROHIBITED_CONTENT"}, "usageMetadata": {"promptTokenCount": 10}});
        assert!(
            matches!(read(blocked.to_string().as_bytes()).unwrap().outcome, Outcome::Refused(Some(ref why)) if why == "PROHIBITED_CONTENT")
        );
        let stop =
            |reason: &str| json!({"candidates": [{"content": {"parts": [{"text": "{\"a"}]}, "finishReason": reason}]});
        assert!(matches!(read(stop("SAFETY").to_string().as_bytes()).unwrap().outcome, Outcome::Refused(_)));
        assert!(matches!(read(stop("MAX_TOKENS").to_string().as_bytes()).unwrap().outcome, Outcome::Cut));
        assert!(matches!(read(stop("MALFORMED_RESPONSE").to_string().as_bytes()).unwrap().outcome, Outcome::Failed(_)));
    }

    #[test]
    fn errors_are_told_apart_into_stop_asking_and_try_again() {
        let reply = |status: u16, error: Value| Reply {
            status,
            retry_after: None,
            should_retry: None,
            body: json!({"error": error}).to_string().into_bytes(),
        };
        let invalid_key = json!({"code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT",
            "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "API_KEY_INVALID", "domain": "googleapis.com"}]});
        assert_eq!(failure(&reply(400, invalid_key), "m"), Unavailable::KeyRefused.into());
        let region = json!({"code": 400, "message": "User location is not supported", "status": "FAILED_PRECONDITION"});
        assert_eq!(failure(&reply(400, region), "m"), Unavailable::Billing.into());
        let missing = json!({"code": 404, "message": "models/gemini-9 is not found", "status": "NOT_FOUND"});
        assert_eq!(failure(&reply(404, missing), "gemini-9"), Unavailable::UnknownModel("gemini-9".to_owned()).into());
        let limited = json!({"code": 429, "message": "Resource has been exhausted", "status": "RESOURCE_EXHAUSTED",
            "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "30s"}]});
        assert_eq!(
            failure(&reply(429, limited), "m"),
            NoAnswer::RateLimited { retry_after: Some(Duration::from_secs(30)) }.into()
        );
        let busy = json!({"code": 503, "message": "The model is overloaded", "status": "UNAVAILABLE"});
        assert_eq!(failure(&reply(503, busy), "m"), NoAnswer::Overloaded.into());
        let bad = json!({"code": 400, "message": "Invalid JSON payload", "status": "INVALID_ARGUMENT"});
        assert_eq!(failure(&reply(400, bad), "m"), NoAnswer::Rejected("Invalid JSON payload".to_owned()).into());
    }
}
