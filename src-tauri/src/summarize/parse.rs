//! Model response -> [`Summary`].
//!
//! Pure and side-effect free: takes the raw text a provider returned and
//! either produces a usable `Summary` or a `SummarizeError` — no network, no
//! I/O. Real models rarely emit clean, bare JSON, so this is deliberately
//! tolerant: it accepts a fenced ```json block, prose wrapped around the
//! JSON object, a bare string where a list was requested, and a missing
//! optional field. What it does not tolerate is a reply with no usable
//! content — that comes back as `SummarizeError::EmptySummary`
//! (`Summary::is_usable` decides "usable"), never a hollow note that looks
//! finished and says nothing.

use super::{ActionItem, Summary, SummarizeError};
use serde_json::Value;

/// Parses a chat completion's raw reply text into a `Summary`.
///
/// `provider` and `model` are metadata only (used in error messages and
/// recorded on the summary) — never transcript text or key material.
pub fn parse_summary(
    provider: &'static str,
    model: &str,
    body: &str,
) -> Result<Summary, SummarizeError> {
    if body.trim().is_empty() {
        return Err(SummarizeError::EmptySummary);
    }

    let json_text = extract_json(body).ok_or_else(|| SummarizeError::BadResponse {
        provider,
        reason: "no JSON object found in the model's reply".to_owned(),
    })?;

    let value: Value = serde_json::from_str(&json_text).map_err(|err| SummarizeError::BadResponse {
        provider,
        reason: format!("could not parse JSON: {err}"),
    })?;

    let title = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let bullets = string_list(value.get("bullets"));
    let decisions = string_list(value.get("decisions"));
    let action_items = action_item_list(value.get("action_items"));

    let summary = Summary {
        title,
        bullets,
        decisions,
        action_items,
        model: model.to_owned(),
    }
    .cleaned();

    if !summary.is_usable() {
        return Err(SummarizeError::EmptySummary);
    }

    Ok(summary)
}

/// Finds the JSON object in a model's reply, whether it's the whole reply,
/// wrapped in a fenced ```json block, or surrounded by prose.
fn extract_json(body: &str) -> Option<String> {
    extract_fenced(body).or_else(|| extract_braces(body))
}

/// Pulls the contents of the first ```json ... ``` fence, if any.
fn extract_fenced(body: &str) -> Option<String> {
    const MARKER: &str = "```json";
    let start = body.find(MARKER)? + MARKER.len();
    let end = body[start..].find("```")?;
    let text = body[start..start + end].trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// Finds the first balanced `{ ... }` span, ignoring braces inside quoted
/// strings so a brace in the model's own prose text does not break the scan.
fn extract_braces(body: &str) -> Option<String> {
    let start = body.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in body[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return Some(body[start..end].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

/// Reads a field that should be a list of strings, but tolerates a model
/// that returned a single bare string instead, or omitted the field.
fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(single)) => vec![single.clone()],
        _ => Vec::new(),
    }
}

/// Reads "action_items", tolerating a single object where a list was asked
/// for, and dropping any entry with no usable `task`.
fn action_item_list(value: Option<&Value>) -> Vec<ActionItem> {
    let entries: Vec<&Value> = match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(obj @ Value::Object(_)) => vec![obj],
        _ => Vec::new(),
    };

    entries.into_iter().filter_map(parse_action_item).collect()
}

fn parse_action_item(value: &Value) -> Option<ActionItem> {
    let task = value.get("task").and_then(Value::as_str)?.to_owned();
    let owner = value
        .get("owner")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let due = value.get("due").and_then(Value::as_str).map(str::to_owned);

    Some(ActionItem { task, owner, due })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "anthropic";
    const MODEL: &str = "test-model";

    #[test]
    fn parses_clean_json() {
        let body = r#"{
            "title": "Weekly sync",
            "bullets": ["Point one", "Point two"],
            "decisions": ["Ship on Friday"],
            "action_items": [
                {"task": "Write the changelog", "owner": "Leo", "due": "Friday"}
            ]
        }"#;

        let summary = parse_summary(PROVIDER, MODEL, body).expect("clean JSON parses");

        assert_eq!(summary.title, "Weekly sync");
        assert_eq!(summary.bullets, vec!["Point one", "Point two"]);
        assert_eq!(summary.decisions, vec!["Ship on Friday"]);
        assert_eq!(summary.action_items.len(), 1);
        assert_eq!(summary.action_items[0].task, "Write the changelog");
        assert_eq!(summary.action_items[0].owner.as_deref(), Some("Leo"));
        assert_eq!(summary.action_items[0].due.as_deref(), Some("Friday"));
        assert_eq!(summary.model, MODEL);
    }

    #[test]
    fn parses_a_fenced_json_block() {
        let body = "Here is the summary:\n```json\n{\"title\": \"Standup\", \"bullets\": [\"Did stuff\"], \"decisions\": [], \"action_items\": []}\n```\n";

        let summary = parse_summary(PROVIDER, MODEL, body).expect("fenced JSON parses");

        assert_eq!(summary.title, "Standup");
        assert_eq!(summary.bullets, vec!["Did stuff"]);
    }

    #[test]
    fn parses_json_wrapped_in_prose() {
        let body = "Sure thing! Here's the JSON you asked for:\n\
                     {\"title\": \"Planning\", \"bullets\": [\"Discussed roadmap\"], \
                     \"decisions\": [], \"action_items\": []}\n\
                     Let me know if you need anything else.";

        let summary = parse_summary(PROVIDER, MODEL, body).expect("prose-wrapped JSON parses");

        assert_eq!(summary.title, "Planning");
        assert_eq!(summary.bullets, vec!["Discussed roadmap"]);
    }

    #[test]
    fn a_brace_inside_a_quoted_string_does_not_break_extraction() {
        let body = r#"{"title": "Talked about {braces} in code", "bullets": ["ok"], "decisions": [], "action_items": []}"#;

        let summary = parse_summary(PROVIDER, MODEL, body).expect("embedded brace is harmless");

        assert_eq!(summary.title, "Talked about {braces} in code");
    }

    #[test]
    fn a_missing_owner_stays_absent_rather_than_being_invented() {
        let body = r#"{
            "title": "Sync",
            "bullets": ["Point"],
            "decisions": [],
            "action_items": [{"task": "Follow up"}]
        }"#;

        let summary = parse_summary(PROVIDER, MODEL, body).expect("parses");

        assert_eq!(summary.action_items.len(), 1);
        assert_eq!(summary.action_items[0].owner, None);
        assert_eq!(summary.action_items[0].due, None);
    }

    #[test]
    fn a_bare_string_is_accepted_where_a_list_was_requested() {
        let body = r#"{
            "title": "Sync",
            "bullets": "Just one point as a bare string",
            "decisions": [],
            "action_items": []
        }"#;

        let summary = parse_summary(PROVIDER, MODEL, body).expect("bare string is tolerated");

        assert_eq!(summary.bullets, vec!["Just one point as a bare string"]);
    }

    #[test]
    fn a_single_action_item_object_is_accepted_where_a_list_was_requested() {
        let body = r#"{
            "title": "Sync",
            "bullets": ["Point"],
            "decisions": [],
            "action_items": {"task": "Follow up", "owner": "Sam"}
        }"#;

        let summary = parse_summary(PROVIDER, MODEL, body).expect("bare object is tolerated");

        assert_eq!(summary.action_items.len(), 1);
        assert_eq!(summary.action_items[0].task, "Follow up");
        assert_eq!(summary.action_items[0].owner.as_deref(), Some("Sam"));
    }

    #[test]
    fn an_empty_reply_is_empty_summary_not_bad_response() {
        let result = parse_summary(PROVIDER, MODEL, "");
        assert!(matches!(result, Err(SummarizeError::EmptySummary)));

        let whitespace_only = parse_summary(PROVIDER, MODEL, "   \n  ");
        assert!(matches!(whitespace_only, Err(SummarizeError::EmptySummary)));
    }

    #[test]
    fn a_reply_with_no_json_at_all_is_a_bad_response() {
        let result = parse_summary(PROVIDER, MODEL, "Sorry, I can't help with that.");
        assert!(matches!(result, Err(SummarizeError::BadResponse { .. })));
    }

    #[test]
    fn malformed_json_is_a_bad_response() {
        let body = r#"{"title": "Sync", "bullets": [oops this is not valid json}"#;
        let result = parse_summary(PROVIDER, MODEL, body);
        assert!(matches!(result, Err(SummarizeError::BadResponse { .. })));
    }

    #[test]
    fn a_title_only_reply_is_not_usable_and_becomes_empty_summary() {
        let body = r#"{"title": "Sync", "bullets": [], "decisions": [], "action_items": []}"#;
        let result = parse_summary(PROVIDER, MODEL, body);
        assert!(matches!(result, Err(SummarizeError::EmptySummary)));
    }

    #[test]
    fn missing_optional_top_level_fields_default_to_empty() {
        let body = r#"{"title": "Sync", "bullets": ["A real point"]}"#;
        let summary = parse_summary(PROVIDER, MODEL, body).expect("missing fields default");

        assert!(summary.decisions.is_empty());
        assert!(summary.action_items.is_empty());
    }

    #[test]
    fn an_action_item_missing_task_is_dropped_rather_than_invented() {
        let body = r#"{
            "title": "Sync",
            "bullets": ["Point"],
            "decisions": [],
            "action_items": [{"owner": "Leo"}, {"task": "Real task"}]
        }"#;

        let summary = parse_summary(PROVIDER, MODEL, body).expect("parses");

        assert_eq!(summary.action_items.len(), 1);
        assert_eq!(summary.action_items[0].task, "Real task");
    }

    #[test]
    fn errors_never_carry_the_transcript_or_key_material() {
        let result = parse_summary(PROVIDER, MODEL, "not json");
        let message = result.unwrap_err().to_string();
        assert!(!message.contains("sk-"));
    }
}
