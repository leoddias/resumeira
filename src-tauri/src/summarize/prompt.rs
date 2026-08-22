//! Transcript -> chat messages.
//!
//! Pure and provider-agnostic: this module never makes a network call, it
//! only builds the messages a chat client sends. The two rules that carry
//! most of the summary's quality live in the system prompt text below —
//! answer in the transcript's language (ADR-0006), and never invent an
//! owner for an action item — and both are asserted by the tests at the
//! bottom of this file.
//!
//! `ChatMessage` is defined here rather than imported from
//! `summarize::providers` because that module is owned by a different
//! packet running concurrently (T-M3-2); the orchestrator reconciles the two
//! shapes at integration time if they disagree.

use super::{ChatMessage, ChatRole};
use crate::transcribe::Transcript;

/// Knobs a caller can set for a specific summarization request.
///
/// Empty for now — every option today (BYOK provider, model) lives outside
/// the prompt itself. The struct exists so `build`'s signature does not
/// change when the first real option (e.g. a custom instruction) lands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SummaryOptions {}

/// The JSON schema the system prompt asks the model to return, and that
/// [`super::parse::parse_summary`] expects back. Kept in one place so the
/// prompt text and the parser cannot silently drift apart.
const JSON_SCHEMA: &str = r#"{
  "title": "string, a short generated title for the meeting",
  "bullets": ["string", "..."],
  "decisions": ["string", "..."],
  "action_items": [
    {
      "task": "string, what is to be done",
      "owner": "string or omitted, who owns it - omit if the meeting did not make it clear",
      "due": "string or omitted, the deadline verbatim as stated, e.g. 'next Friday'"
    }
  ]
}"#;

fn system_prompt() -> String {
    format!(
        "You are an assistant that turns a meeting transcript into structured \
         notes.\n\
         \n\
         Respond with a single JSON object and nothing else, matching exactly \
         this schema:\n\
         {JSON_SCHEMA}\n\
         \n\
         Rules:\n\
         - Answer in the same language the transcript is written in. If the \
         transcript is in Portuguese, the title, bullets, decisions and action \
         items must all be in Portuguese; never translate to English or any \
         other language.\n\
         - \"bullets\" should be about five items covering what the meeting was \
         about.\n\
         - \"decisions\" must contain only decisions the meeting actually took, \
         not topics that were merely discussed or proposed. Leave it empty if \
         no decision was taken.\n\
         - For each action item, only set \"owner\" when the transcript makes \
         the owner clear. Never guess or infer an owner from context - omit the \
         field entirely rather than attaching the wrong name. An action item \
         with no owner is correct and expected; a guessed owner is a worse \
         outcome than an absent one.\n\
         - Only set \"due\" when the transcript states a deadline; use its own \
         words rather than resolving it to a calendar date.\n\
         - Output raw JSON only: no prose before or after it, no markdown code \
         fence."
    )
}

fn user_prompt(transcript: &Transcript) -> String {
    let language_hint = transcript
        .language
        .as_deref()
        .map(|lang| format!("The transcript's detected language tag is \"{lang}\".\n\n"))
        .unwrap_or_default();

    format!(
        "{language_hint}Summarize the following meeting transcript.\n\n\
         Transcript:\n\
         {text}",
        text = transcript.to_prompt_text()
    )
}

/// Builds the messages to send to a chat completions API for this
/// transcript: a system prompt describing the task and JSON schema, and a
/// user message carrying the transcript text.
pub fn build(transcript: &Transcript, _options: &SummaryOptions) -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: ChatRole::System,
            content: system_prompt(),
        },
        ChatMessage {
            role: ChatRole::User,
            content: user_prompt(transcript),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::{Engine, Segment};

    fn transcript(language: Option<&str>, lines: &[(&str, f64, f64)]) -> Transcript {
        Transcript {
            segments: lines
                .iter()
                .map(|(text, start, end)| Segment {
                    start: *start,
                    end: *end,
                    text: (*text).to_owned(),
                    track: None,
                    speaker: None,
                })
                .collect(),
            language: language.map(|l| l.to_owned()),
            engine: Engine::Local,
        }
    }

    #[test]
    fn build_produces_a_system_message_and_a_user_message() {
        let t = transcript(Some("pt"), &[("Bom dia pessoal", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, ChatRole::System);
        assert_eq!(messages[1].role, ChatRole::User);
    }

    #[test]
    fn system_prompt_names_the_language_rule() {
        let t = transcript(None, &[("hello", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());
        let system = &messages[0].content;

        assert!(
            system.to_lowercase().contains("same language"),
            "system prompt must instruct the model to answer in the \
             transcript's language (ADR-0006): {system}"
        );
    }

    #[test]
    fn system_prompt_names_the_no_guessing_rule_for_owners() {
        let t = transcript(None, &[("hello", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());
        let system = &messages[0].content;

        assert!(
            system.to_lowercase().contains("never guess"),
            "system prompt must forbid inventing an action item's owner: {system}"
        );
        assert!(
            system.to_lowercase().contains("omit"),
            "system prompt must tell the model to omit an unclear owner: {system}"
        );
    }

    #[test]
    fn system_prompt_separates_decisions_from_discussion() {
        let t = transcript(None, &[("hello", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());
        let system = &messages[0].content;

        assert!(
            system.to_lowercase().contains("merely discussed"),
            "system prompt must distinguish decisions taken from topics \
             merely discussed: {system}"
        );
    }

    #[test]
    fn system_prompt_asks_for_json_only() {
        let t = transcript(None, &[("hello", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());
        let system = &messages[0].content;

        assert!(system.contains("JSON"));
        assert!(system.contains("title"));
        assert!(system.contains("action_items"));
    }

    #[test]
    fn user_message_carries_the_transcript_text() {
        let t = transcript(
            Some("en"),
            &[("First point", 0.0, 1.0), ("Second point", 1.0, 2.0)],
        );
        let messages = build(&t, &SummaryOptions::default());
        let user = &messages[1].content;

        assert!(user.contains("First point"));
        assert!(user.contains("Second point"));
    }

    #[test]
    fn user_message_mentions_the_detected_language_when_present() {
        let t = transcript(Some("pt"), &[("Ola", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());
        assert!(messages[1].content.contains("pt"));
    }

    #[test]
    fn user_message_is_fine_without_a_detected_language() {
        let t = transcript(None, &[("Ola", 0.0, 1.0)]);
        let messages = build(&t, &SummaryOptions::default());
        assert!(messages[1].content.contains("Ola"));
    }
}
