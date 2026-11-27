//! An answer in JSON checked against the schema it was asked for, for providers that do not guarantee it.
//!
//! The Claude API, OpenAI and Gemini hold an answer to the schema themselves. Most others take the schema as a
//! request a model may or may not follow, or take no schema at all; a server on this computer may run a model too
//! small to follow one. For those the answer is checked here, against the parts of JSON Schema the engine's
//! features write (types, `properties`, `required`, `additionalProperties`, `items`, `enum`, `const`, `anyOf`, and
//! the bounds of numbers and arrays), and asked for once more with what was wrong when it does not follow it.
//! Keywords this does not know are not checked: they cannot make a good answer fail.

use serde_json::Value;

use super::provider::Answers;

/// The question as sent: for a provider that cannot be held to a schema, with the schema written into it, and on
/// the second attempt with what was wrong with the first answer. Everything added comes after the question, so the
/// shared part before it stays the same and stays cached.
pub(super) fn question(question: &str, schema: &Value, answers: Answers, wrong: Option<&str>) -> String {
    let mut asked = question.to_owned();
    if matches!(answers, Answers::JsonMode | Answers::Prompted) {
        asked.push_str(
            "\n\nAnswer with one JSON value and nothing else: no words before or after it, no code fence. It must be \
             valid against this JSON Schema:\n",
        );
        asked.push_str(&schema.to_string());
    }
    if let Some(wrong) = wrong {
        asked.push_str("\n\nYour previous answer could not be used: ");
        asked.push_str(wrong);
        asked.push_str(". Answer again with JSON that follows the schema exactly.");
    }
    asked
}

/// The JSON in an answer, as text: the answer itself, or what is inside a code fence, or from the first `{` or `[`
/// to the last `}` or `]` when a model wrote words around it.
pub(super) fn json_in(answer: &str) -> Option<(String, Value)> {
    let trimmed = answer.trim();
    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .map(str::trim);
    let mut candidates = vec![trimmed];
    candidates.extend(fenced);
    if let (Some(start), Some(end)) = (trimmed.find(['{', '[']), trimmed.rfind(['}', ']'])) {
        if start < end {
            candidates.extend(trimmed.get(start..=end));
        }
    }
    candidates
        .into_iter()
        .find_map(|text| serde_json::from_str::<Value>(text).ok().map(|value| (text.to_owned(), value)))
}

/// Whether `value` follows `schema`; when not, where and how it does not, in a few words a model can act on.
pub(super) fn check(value: &Value, schema: &Value) -> Result<(), String> {
    walk(value, schema, "the answer")
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_type(value: &Value, name: &str) -> bool {
    let kind = kind_of(value);
    kind == name || (name == "number" && kind == "integer") || (name == "integer" && is_whole(value))
}

fn is_whole(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| number.fract() == 0.0)
}

fn walk(value: &Value, schema: &Value, at: &str) -> Result<(), String> {
    let Some(schema) = schema.as_object() else { return Ok(()) };
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if !options.iter().any(|option| walk(value, option, at).is_ok()) {
            return Err(format!("{at} matches none of the forms allowed"));
        }
    }
    match schema.get("type") {
        Some(Value::String(name)) if !is_type(value, name) => {
            return Err(format!("{at} must be {name}, not {}", kind_of(value)));
        }
        Some(Value::Array(names)) if !names.iter().filter_map(Value::as_str).any(|name| is_type(value, name)) => {
            return Err(format!("{at} is {}, which is not allowed", kind_of(value)));
        }
        _ => {}
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{at} must be one of {}", Value::Array(allowed.clone())));
        }
    }
    if let Some(constant) = schema.get("const") {
        if constant != value {
            return Err(format!("{at} must be {constant}"));
        }
    }
    if let Some(number) = value.as_f64() {
        if schema.get("minimum").and_then(Value::as_f64).is_some_and(|least| number < least) {
            return Err(format!("{at} is below its minimum"));
        }
        if schema.get("maximum").and_then(Value::as_f64).is_some_and(|most| number > most) {
            return Err(format!("{at} is above its maximum"));
        }
    }
    match value {
        Value::Object(fields) => {
            let properties = schema.get("properties").and_then(Value::as_object);
            for name in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
            {
                if !fields.contains_key(name) {
                    return Err(format!("{at} lacks \"{name}\""));
                }
            }
            for (name, field) in fields {
                let inner = format!("\"{name}\" of {at}");
                match properties.and_then(|properties| properties.get(name)) {
                    Some(property) => walk(field, property, &inner)?,
                    None => match schema.get("additionalProperties") {
                        Some(Value::Bool(false)) => return Err(format!("{at} has \"{name}\", which is not asked for")),
                        Some(extra @ Value::Object(_)) => walk(field, extra, &inner)?,
                        _ => {}
                    },
                }
            }
        }
        Value::Array(items) => {
            let count = |key: &str| schema.get(key).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
            if count("minItems").is_some_and(|least| items.len() < least) {
                return Err(format!("{at} has too few items"));
            }
            if count("maxItems").is_some_and(|most| items.len() > most) {
                return Err(format!("{at} has too many items"));
            }
            if let Some(item) = schema.get("items") {
                for (index, element) in items.iter().enumerate() {
                    walk(element, item, &format!("item {} of {at}", index + 1))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether OpenAI's strict mode can take `schema`: every object closed (`additionalProperties: false`) and every
/// property required, which strict mode refuses a schema without. When not, the schema is sent without `strict`
/// and the answer checked here.
pub(super) fn strict_ready(schema: &Value) -> bool {
    match schema {
        Value::Object(fields) => {
            let closed_and_complete = match fields.get("properties").and_then(Value::as_object) {
                Some(properties) => {
                    let required: Vec<&str> = fields
                        .get("required")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    fields.get("additionalProperties") == Some(&Value::Bool(false))
                        && properties.keys().all(|name| required.contains(&name.as_str()))
                }
                None => true,
            };
            closed_and_complete && fields.values().all(strict_ready)
        }
        Value::Array(items) => items.iter().all(strict_ready),
        _ => true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "claims": {"type": "array", "minItems": 1, "items": {
                    "type": "object",
                    "properties": {"paper": {"type": "string"}, "quote": {"type": "string"}},
                    "required": ["paper", "quote"],
                    "additionalProperties": false
                }},
                "tier": {"enum": ["oral", "poster"]},
                "score": {"type": ["number", "null"], "minimum": 0}
            },
            "required": ["name", "claims"],
            "additionalProperties": false
        })
    }

    #[test]
    fn an_answer_that_follows_the_schema_passes() {
        let good = json!({"name": "Merging", "claims": [{"paper": "p1", "quote": "q"}], "tier": "oral", "score": null});
        assert_eq!(check(&good, &schema()), Ok(()));
        assert_eq!(
            check(&json!({"name": "x", "claims": [{"paper": "p", "quote": "q"}], "score": 3}), &schema()),
            Ok(())
        );
    }

    #[test]
    fn an_answer_that_does_not_is_told_where_and_how() {
        let cases = [
            (json!({"claims": []}), "lacks \"name\""),
            (json!({"name": 3, "claims": [{"paper": "p", "quote": "q"}]}), "must be string"),
            (json!({"name": "x", "claims": []}), "too few items"),
            (json!({"name": "x", "claims": [{"paper": "p"}]}), "lacks \"quote\""),
            (json!({"name": "x", "claims": [{"paper": "p", "quote": "q"}], "extra": 1}), "not asked for"),
            (json!({"name": "x", "claims": [{"paper": "p", "quote": "q"}], "tier": "talk"}), "one of"),
            (json!({"name": "x", "claims": [{"paper": "p", "quote": "q"}], "score": -1}), "below its minimum"),
            (json!(["not", "an", "object"]), "must be object"),
        ];
        for (value, said) in cases {
            let why = check(&value, &schema()).unwrap_err();
            assert!(why.contains(said), "{value}: {why}");
        }
    }

    #[test]
    fn the_json_is_found_in_a_fence_or_among_words() {
        assert_eq!(json_in(" {\"a\": 1} ").unwrap().1, json!({"a": 1}));
        assert_eq!(json_in("```json\n{\"a\": 1}\n```").unwrap().1, json!({"a": 1}));
        assert_eq!(json_in("Here it is: {\"a\": [1]} I hope it helps.").unwrap().1, json!({"a": [1]}));
        assert!(json_in("no json here").is_none());
        assert!(json_in("{\"a\": 1").is_none());
    }

    #[test]
    fn the_schema_is_written_into_the_question_only_where_it_cannot_be_sent() {
        let schema = schema();
        assert_eq!(question("Q?", &schema, Answers::Requested, None), "Q?");
        let prompted = question("Q?", &schema, Answers::Prompted, None);
        assert!(prompted.starts_with("Q?\n\n") && prompted.contains("\"additionalProperties\":false"));
        let again = question("Q?", &schema, Answers::Requested, Some("the answer lacks \"name\""));
        assert!(again.ends_with("follows the schema exactly.") && again.contains("lacks \"name\""));
    }

    #[test]
    fn strict_mode_is_used_only_for_a_schema_it_takes() {
        let complete = json!({"type": "object", "properties": {"a": {"type": "array", "items": {
            "type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"], "additionalProperties": false
        }}}, "required": ["a"], "additionalProperties": false});
        assert!(strict_ready(&complete));
        assert!(!strict_ready(&schema()), "\"tier\" and \"score\" are optional");
        let optional =
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": false});
        assert!(!strict_ready(&optional));
        let open = json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]});
        assert!(!strict_ready(&open));
    }
}
