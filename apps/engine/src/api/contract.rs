//! What the features' route tests need to hold a route's JSON against the type the interface reads it as, in
//! `packages/contracts/src/index.ts`: a field the engine names differently from the contract is a field the
//! interface finds missing at run time, which no type checker on either side sees.
//!
//! The TypeScript is read with a small reader that knows only what the contracts use (interfaces, type aliases,
//! object literals, unions, intersections, `extends`): it collects the field names a type can have, through every
//! type it names, and the fields an interface requires. A route's answer is then checked both ways: every key in
//! the JSON is a field the contract knows, and every field the contract's interface requires is in the JSON.

use std::collections::BTreeSet;

use serde_json::Value;

const CONTRACTS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/contracts/src/index.ts"));

/// The contracts with their comments blanked out, so that a word in a comment is never taken for a field.
fn source() -> String {
    let bytes = CONTRACTS.as_bytes();
    let mut out = String::with_capacity(CONTRACTS.len());
    let mut at = 0;
    let mut in_string: Option<u8> = None;
    while at < bytes.len() {
        let here = bytes[at];
        if let Some(quote) = in_string {
            if here == b'\\' && at + 1 < bytes.len() {
                out.push_str(&CONTRACTS[at..at + 2]);
                at += 2;
                continue;
            }
            if here == quote {
                in_string = None;
            }
        } else if here == b'"' || here == b'\'' || here == b'`' {
            in_string = Some(here);
        } else if CONTRACTS[at..].starts_with("//") {
            let end = CONTRACTS[at..].find('\n').map_or(bytes.len(), |line| at + line);
            out.push(' ');
            at = end;
            continue;
        } else if CONTRACTS[at..].starts_with("/*") {
            let end = CONTRACTS[at + 2..].find("*/").map_or(bytes.len(), |close| at + 2 + close + 2);
            out.push(' ');
            at = end;
            continue;
        }
        let width = CONTRACTS[at..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&CONTRACTS[at..at + width]);
        at += width;
    }
    out
}

/// The text of `name`'s declaration after its name: `extends ... { ... }` for an interface, `= ...` for a type.
fn declaration(source: &str, name: &str) -> Option<String> {
    for keyword in ["export interface ", "export type "] {
        let mut from = 0;
        while let Some(found) = source[from..].find(&format!("{keyword}{name}")) {
            let start = from + found + keyword.len() + name.len();
            from = start;
            let next = source[start..].chars().next()?;
            if next.is_alphanumeric() || next == '_' {
                continue;
            }
            let rest = &source[start..];
            let end = if keyword.contains("interface") {
                let open = rest.find('{')?;
                open + balanced(&rest[open..])?
            } else {
                type_end(rest)?
            };
            return Some(rest[..end].to_owned());
        }
    }
    None
}

/// The length of the bracketed text `text` starts with, its closing bracket included.
fn balanced(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, letter) in text.char_indices() {
        match letter {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Where a type alias ends: its `;` outside every bracket.
fn type_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, letter) in text.char_indices() {
        match letter {
            '{' | '(' | '[' | '<' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            '>' if !text[..at].ends_with('=') => depth = depth.saturating_sub(1),
            ';' if depth == 0 => return Some(at),
            _ => {}
        }
    }
    Some(text.len())
}

/// The fields written in `text` (`name:` or `name?:` after `{`, `;`, `,` or a new line), each with whether it is
/// required, and its depth in braces (1 is the declaration's own object).
fn fields(text: &str) -> Vec<(String, bool, usize)> {
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut previous = '\n';
    let letters: Vec<(usize, char)> = text.char_indices().collect();
    let mut index = 0;
    while index < letters.len() {
        let (at, letter) = letters[index];
        match letter {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        let starts_word = letter.is_alphabetic() || letter == '_' || letter == '"';
        if starts_word && matches!(previous, '{' | ';' | ',' | '\n') {
            let quoted = letter == '"';
            let begin = if quoted { at + 1 } else { at };
            let rest = &text[begin..];
            let length = if quoted {
                rest.find('"').unwrap_or(rest.len())
            } else {
                rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len())
            };
            let key = &rest[..length];
            let after = rest.get(length + usize::from(quoted)..).unwrap_or("").trim_start();
            if let Some(optional) = after.strip_prefix('?') {
                if optional.trim_start().starts_with(':') {
                    found.push((key.to_owned(), false, depth));
                }
            } else if after.starts_with(':') {
                found.push((key.to_owned(), true, depth));
            }
        }
        if !letter.is_whitespace() || letter == '\n' {
            previous = letter;
        }
        index += 1;
    }
    found
}

/// The names in `text` that are types declared in the contracts.
fn named_types(source: &str, text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut word = String::new();
    for letter in text.chars().chain(std::iter::once(' ')) {
        if letter.is_alphanumeric() || letter == '_' {
            word.push(letter);
        } else if !word.is_empty() {
            let named = std::mem::take(&mut word);
            if named.starts_with(|c: char| c.is_ascii_uppercase()) && declaration(source, &named).is_some() {
                names.push(named);
            }
        }
    }
    names
}

/// Every field a value of the contract's type `name` can have, at any depth, through every type it names.
pub(crate) fn known_fields(name: &str) -> BTreeSet<String> {
    let source = source();
    let mut known = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut waiting = vec![name.to_owned()];
    while let Some(next) = waiting.pop() {
        if !seen.insert(next.clone()) {
            continue;
        }
        let Some(text) = declaration(&source, &next) else { continue };
        known.extend(fields(&text).into_iter().map(|(key, _, _)| key));
        waiting.extend(named_types(&source, &text));
    }
    assert!(seen.contains(name) && declaration(&source, name).is_some(), "the contracts declare no type {name}");
    known
}

/// The fields the contract's interface `name` requires at its top level, those of what it extends included.
pub(crate) fn required_fields(name: &str) -> BTreeSet<String> {
    let source = source();
    let mut required = BTreeSet::new();
    let mut waiting = vec![name.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(next) = waiting.pop() {
        if !seen.insert(next.clone()) {
            continue;
        }
        let Some(text) = declaration(&source, &next) else { continue };
        let Some(open) = text.find('{') else { continue };
        if !text.trim_start().starts_with("extends") && !text.trim_start().starts_with('{') {
            // a type alias: its variants differ, so no field is required of all of them here
            continue;
        }
        required.extend(
            fields(&text[open..])
                .into_iter()
                .filter(|(_, needed, depth)| *needed && *depth == 1)
                .map(|(key, _, _)| key),
        );
        waiting.extend(named_types(&source, &text[..open]));
    }
    required
}

/// Every key of `json`, at any depth, except inside the values of `opaque` keys (maps keyed by paper ids, say).
fn keys_of(json: &Value, opaque: &[&str], into: &mut BTreeSet<String>) {
    match json {
        Value::Object(map) => {
            for (key, value) in map {
                into.insert(key.clone());
                if !opaque.contains(&key.as_str()) {
                    keys_of(value, opaque, into);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys_of(item, opaque, into)),
        _ => {}
    }
}

/// Holds a route's answer against the contract's type `name`: every key it has is a field the contract knows, and
/// (when `name` is an interface) every field the contract requires is there. `opaque` names keys whose values
/// are maps with keys of their own (ids), not looked into.
pub(crate) fn assert_fits(name: &str, json: &Value, opaque: &[&str]) {
    let known = known_fields(name);
    let mut keys = BTreeSet::new();
    keys_of(json, opaque, &mut keys);
    let unknown: Vec<&String> = keys.difference(&known).collect();
    assert!(unknown.is_empty(), "{name}: the engine sends {unknown:?}, which the contract does not name, in {json}");
    if let Value::Object(map) = json {
        let missing: Vec<String> = required_fields(name).into_iter().filter(|key| !map.contains_key(key)).collect();
        assert!(
            missing.is_empty(),
            "{name}: the contract requires {missing:?}, which the engine does not send, in {json}"
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_interfaces_fields_are_read_with_those_it_extends() {
        let known = known_fields("BatchCost");
        assert!(known.contains("usd"), "{known:?}");
        let required = required_fields("BatchCost");
        assert!(!required.is_empty());
        assert!(required.iter().all(|field| known.contains(field)));
    }

    #[test]
    fn a_word_in_a_comment_is_not_taken_for_a_field() {
        // Evidence's comment says "the paper's own words at that place, which is what is shown"
        let known = known_fields("Evidence");
        assert!(known.contains("paper") && known.contains("quote") && known.contains("established"));
        assert!(!known.contains("which"));
    }

    #[test]
    fn a_key_the_contract_does_not_name_fails() {
        let wrong = json!({"paper": "p", "quotes": "q", "established": {"kind": "verified"}});
        let failed = std::panic::catch_unwind(|| assert_fits("Evidence", &wrong, &[]));
        assert!(failed.is_err());
    }

    #[test]
    fn a_missing_required_field_fails_and_a_whole_answer_passes() {
        let missing = json!({"paper": "p", "established": {"kind": "abstract"}});
        assert!(std::panic::catch_unwind(|| assert_fits("Evidence", &missing, &[])).is_err());
        let whole = json!({"paper": "p", "quote": "q", "established": {"kind": "verified", "address": {"page": 1}, "matched": {"kind": "exact"}}});
        assert_fits("Evidence", &whole, &[]);
    }

    #[test]
    fn a_type_the_contracts_do_not_declare_fails_loudly() {
        assert!(std::panic::catch_unwind(|| known_fields("NoSuchType")).is_err());
    }
}
