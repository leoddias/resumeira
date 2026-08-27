//! Model reply -> [`Turn`]s.
//!
//! Pure and side-effect free. Tolerant in the same way `summarize::parse` is,
//! and for the same reason: real models wrap JSON in prose and fences. The
//! difference is the stakes. A summary that will not parse is an error the
//! user must see; a speaker list that will not parse is a label missing from
//! a note that is otherwise complete, so anything unusable here is dropped
//! quietly and the rest is kept.

use super::Turn;
use crate::summarize::{parse::extract_json, SummarizeError};
use serde_json::Value;

/// Reads the turns out of a model's reply.
///
/// `provider` is metadata only, for the error message - never transcript
/// text or key material.
pub fn parse_turns(provider: &'static str, body: &str) -> Result<Vec<Turn>, SummarizeError> {
    // A clean reply parses whole - which is also the only way a bare `[...]`
    // survives, since digging for a brace inside one would find the first
    // turn and lose the rest.
    let value: Value = match serde_json::from_str(body.trim()) {
        Ok(value) => value,
        Err(_) => {
            let json_text = extract_json(body).ok_or_else(|| SummarizeError::BadResponse {
                provider,
                reason: "no JSON object found in the model's reply".to_owned(),
            })?;
            serde_json::from_str(&json_text).map_err(|err| SummarizeError::BadResponse {
                provider,
                reason: format!("could not parse JSON: {err}"),
            })?
        }
    };

    // `{"turns": [...]}` is what the prompt asks for; a bare array is what
    // models hand back often enough to be worth accepting.
    let entries = value
        .get("turns")
        .or_else(|| value.get("speakers"))
        .unwrap_or(&value);
    let Some(entries) = entries.as_array() else {
        return Err(SummarizeError::BadResponse {
            provider,
            reason: "the reply has no list of turns".to_owned(),
        });
    };

    Ok(entries.iter().filter_map(turn).collect())
}

/// One entry, or nothing if it is not usable. A single malformed turn costs
/// its own lines their label and nothing else.
fn turn(value: &Value) -> Option<Turn> {
    let speaker = value.get("speaker").and_then(Value::as_str)?.trim();
    if speaker.is_empty() {
        return None;
    }

    let from = index(value.get("from"))?;
    // A turn covering one line is often written without an end.
    let to = index(value.get("to")).unwrap_or(from);

    Some(Turn {
        from,
        to,
        speaker: speaker.to_owned(),
    })
}

/// A line number, whether the model wrote it as a number or as a string.
///
/// A negative or fractional index is refused rather than rounded: it means
/// the reply is not addressing the lines that were sent.
fn index(value: Option<&Value>) -> Option<usize> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return usize::try_from(number).ok();
    }
    value.as_str()?.trim().parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "test";

    #[test]
    fn a_clean_reply_parses() {
        let turns = parse_turns(
            PROVIDER,
            r#"{"turns":[{"from":0,"to":2,"speaker":"Ana"},{"from":3,"to":3,"speaker":"Bruno"}]}"#,
        )
        .expect("parses");

        assert_eq!(
            turns,
            vec![
                Turn {
                    from: 0,
                    to: 2,
                    speaker: "Ana".to_owned()
                },
                Turn {
                    from: 3,
                    to: 3,
                    speaker: "Bruno".to_owned()
                },
            ]
        );
    }

    #[test]
    fn prose_and_a_code_fence_around_the_json_are_tolerated() {
        let body = "Sure, here you go:\n```json\n{\"turns\":[{\"from\":0,\"to\":1,\"speaker\":\"Ana\"}]}\n```\nHope that helps.";
        let turns = parse_turns(PROVIDER, body).expect("parses");
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn a_bare_array_is_accepted() {
        let turns = parse_turns(PROVIDER, r#"[{"from":1,"to":2,"speaker":"Ana"}]"#)
            .expect("a bare array is a shape models really return");
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn a_missing_end_covers_the_single_starting_line() {
        let turns =
            parse_turns(PROVIDER, r#"{"turns":[{"from":4,"speaker":"Ana"}]}"#).expect("parses");
        assert_eq!(
            turns,
            vec![Turn {
                from: 4,
                to: 4,
                speaker: "Ana".to_owned()
            }]
        );
    }

    #[test]
    fn line_numbers_written_as_strings_are_read() {
        let turns = parse_turns(
            PROVIDER,
            r#"{"turns":[{"from":"0","to":"1","speaker":"Ana"}]}"#,
        )
        .expect("parses");
        assert_eq!(
            turns,
            vec![Turn {
                from: 0,
                to: 1,
                speaker: "Ana".to_owned()
            }]
        );
    }

    #[test]
    fn one_malformed_turn_does_not_cost_the_others_their_labels() {
        let turns = parse_turns(
            PROVIDER,
            r#"{"turns":[{"from":0,"to":1,"speaker":"Ana"},{"to":5},{"from":-2,"speaker":"Bruno"},{"from":6,"to":7,"speaker":"Cris"}]}"#,
        )
        .expect("parses");

        assert_eq!(
            turns.iter().map(|t| t.speaker.as_str()).collect::<Vec<_>>(),
            vec!["Ana", "Cris"],
            "a nameless turn and a negative index are dropped, the rest survive"
        );
    }

    #[test]
    fn a_reply_with_no_json_is_an_error() {
        let error = parse_turns(PROVIDER, "I could not tell the speakers apart.")
            .expect_err("no JSON is an error");
        assert!(matches!(error, SummarizeError::BadResponse { .. }));
    }

    #[test]
    fn an_empty_reply_is_an_error_not_an_empty_list() {
        assert!(parse_turns(PROVIDER, "").is_err());
    }

    #[test]
    fn a_json_object_that_is_not_a_list_of_turns_is_an_error() {
        let error =
            parse_turns(PROVIDER, r#"{"error":"rate limited"}"#).expect_err("not a turn list");
        assert!(matches!(error, SummarizeError::BadResponse { .. }));
    }

    #[test]
    fn an_empty_turn_list_parses_to_nothing_labelled() {
        let turns = parse_turns(PROVIDER, r#"{"turns":[]}"#).expect("parses");
        assert!(turns.is_empty());
    }
}
