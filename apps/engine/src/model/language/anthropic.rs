//! The Claude API's own shapes: the Messages request built from an [`Ask`], the answer and its usage as the API's
//! reference documents them, and what a failed request's status and error body mean.
//!
//! The client in the parent module sends, retries and records; this file holds only what is particular to the
//! Claude API, so that each provider's shapes live in a file of their own.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Ask, AskError, Form, NoAnswer, Reply, Unavailable, retry_after_header};
use crate::usage::Tokens;

impl Ask<'_> {
    /// The messages and settings of the request, without what only `messages` takes.
    pub(super) fn body(&self) -> Value {
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

/// An answer as the API documents it, only what is read of it.
#[derive(Deserialize)]
pub(super) struct Message {
    pub(super) model: Option<String>,
    pub(super) stop_reason: Option<String>,
    pub(super) stop_details: Option<StopDetails>,
    #[serde(default)]
    pub(super) content: Vec<Block>,
    pub(super) usage: Option<Usage>,
}

#[derive(Deserialize)]
pub(super) struct StopDetails {
    pub(super) category: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(super) enum Block {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Clone, Deserialize)]
pub(super) struct Usage {
    pub(super) model: Option<String>,
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
    pub(super) iterations: Option<Vec<Usage>>,
}

#[derive(Clone, Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

impl Usage {
    pub(super) fn tokens(&self) -> Tokens {
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
pub(super) fn failure(reply: &Reply, model: &str) -> AskError {
    let detail = serde_json::from_slice::<ErrorBody>(&reply.body).ok().and_then(|body| body.error);
    let message = detail.as_ref().and_then(|detail| detail.message.clone()).unwrap_or_default();
    let kind = detail.as_ref().and_then(|detail| detail.kind.clone()).unwrap_or_default();
    let code = detail.and_then(|detail| detail.details).and_then(|details| details.error_code);
    let retry_after = retry_after_header(reply);
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
