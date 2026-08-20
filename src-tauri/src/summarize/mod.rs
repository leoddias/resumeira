//! Turning a transcript into the note a person actually reads.
//!
//! The summary is the product. A recording nobody reopens is a file; a
//! summary someone acts on is the reason this app exists. So the shape below
//! is deliberately narrow and fixed for v0.1 (ADR-0006): a title, roughly
//! five bullets, the decisions taken, and action items with owners.
//!
//! Provider calls happen here in Rust so API keys never cross IPC into the
//! WebView (ADR-0009).

pub mod parse;
pub mod prompt;
pub mod providers;

use serde::{Deserialize, Serialize};

/// LLM providers a user can bring a key for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SummaryProvider {
    Anthropic,
    OpenAi,
    Groq,
}

impl SummaryProvider {
    /// Keychain entry name. Stable — changing one strands every stored key.
    pub fn key_name(self) -> &'static str {
        match self {
            SummaryProvider::Anthropic => "anthropic",
            SummaryProvider::OpenAi => "openai",
            SummaryProvider::Groq => "groq",
        }
    }

    /// Model used when the user has not chosen one.
    pub fn default_model(self) -> &'static str {
        match self {
            SummaryProvider::Anthropic => "claude-sonnet-5",
            SummaryProvider::OpenAi => "gpt-5",
            SummaryProvider::Groq => "llama-3.3-70b-versatile",
        }
    }
}

/// Chat role, mirroring the `role` field every chat-completions-style API
/// expects (Anthropic, OpenAI, Groq).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// One turn in the conversation sent to a provider.
///
/// Lives here rather than in `prompt` or `providers` because both need it:
/// `prompt::build` produces these and `providers::complete` consumes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// Something someone agreed to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    /// What is to be done.
    pub task: String,
    /// Who owns it, when the meeting made that clear.
    ///
    /// Optional on purpose: inventing an owner is worse than admitting the
    /// meeting never assigned one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// A deadline as the meeting stated it, verbatim ("next Friday").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

/// The note's structured content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Short generated title; also the meeting folder's display name.
    pub title: String,
    /// Roughly five bullets covering what the meeting was about.
    pub bullets: Vec<String>,
    /// Decisions actually taken, not topics discussed.
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItem>,
    /// Model that produced this, recorded so the note can say so.
    pub model: String,
}

impl Summary {
    /// Whether this is worth writing into a note.
    ///
    /// A model that returns an empty shell (or only a title) has failed, and
    /// the honest outcome is an error the user sees — not a note that looks
    /// finished and says nothing.
    pub fn is_usable(&self) -> bool {
        !self.title.trim().is_empty()
            && (!self.bullets.iter().all(|b| b.trim().is_empty())
                || !self.decisions.is_empty()
                || !self.action_items.is_empty())
    }

    /// Strips blank entries a model tends to emit around lists.
    pub fn cleaned(mut self) -> Self {
        self.title = self.title.trim().to_owned();
        self.bullets.retain(|b| !b.trim().is_empty());
        self.decisions.retain(|d| !d.trim().is_empty());
        self.action_items
            .retain(|item| !item.task.trim().is_empty());
        for item in &mut self.action_items {
            item.owner = item.owner.take().filter(|o| !o.trim().is_empty());
            item.due = item.due.take().filter(|d| !d.trim().is_empty());
        }
        self
    }
}

/// Anything that can go wrong while summarizing.
///
/// Carries provider names and error kinds only — never transcript text,
/// note content, or key material (docs/CONVENTIONS.md § Privacy).
#[derive(Debug, thiserror::Error)]
pub enum SummarizeError {
    #[error("no API key configured for {provider}")]
    MissingKey { provider: &'static str },

    #[error("{provider} rejected the key")]
    Unauthorized { provider: &'static str },

    #[error("{provider} is rate limiting; retry after {retry_after_secs}s")]
    RateLimited {
        provider: &'static str,
        retry_after_secs: u64,
    },

    #[error("{provider} returned an unexpected response: {reason}")]
    BadResponse {
        provider: &'static str,
        reason: String,
    },

    #[error("network request to {provider} failed: {reason}")]
    Network {
        provider: &'static str,
        reason: String,
    },

    #[error("the transcript is too long for {provider} ({tokens} tokens)")]
    TooLong {
        provider: &'static str,
        tokens: usize,
    },

    #[error("there is nothing in this meeting to summarize")]
    NothingToSummarize,

    #[error("the model returned a summary with no content")]
    EmptySummary,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            title: "Weekly sync".to_owned(),
            bullets: vec!["Discussed the release".to_owned()],
            decisions: vec![],
            action_items: vec![],
            model: "test-model".to_owned(),
        }
    }

    #[test]
    fn provider_key_names_are_stable_because_the_keychain_uses_them() {
        assert_eq!(SummaryProvider::Anthropic.key_name(), "anthropic");
        assert_eq!(SummaryProvider::OpenAi.key_name(), "openai");
        assert_eq!(SummaryProvider::Groq.key_name(), "groq");
    }

    #[test]
    fn an_empty_shell_is_not_usable() {
        let empty = Summary {
            title: "Weekly sync".to_owned(),
            bullets: vec!["  ".to_owned()],
            decisions: vec![],
            action_items: vec![],
            model: "test-model".to_owned(),
        };
        assert!(
            !empty.is_usable(),
            "a title with no content must not pass as a finished note"
        );
    }

    #[test]
    fn a_summary_with_only_action_items_is_still_usable() {
        let actionable = Summary {
            bullets: vec![],
            action_items: vec![ActionItem {
                task: "Ship the build".to_owned(),
                owner: Some("Leo".to_owned()),
                due: None,
            }],
            ..summary()
        };
        assert!(actionable.is_usable());
    }

    #[test]
    fn an_untitled_summary_is_not_usable() {
        let untitled = Summary {
            title: "   ".to_owned(),
            ..summary()
        };
        assert!(!untitled.is_usable());
    }

    #[test]
    fn cleaning_drops_the_blank_entries_models_emit() {
        let messy = Summary {
            title: "  Weekly sync  ".to_owned(),
            bullets: vec!["Real point".to_owned(), "".to_owned(), "  ".to_owned()],
            decisions: vec!["".to_owned()],
            action_items: vec![
                ActionItem {
                    task: "Ship it".to_owned(),
                    owner: Some("  ".to_owned()),
                    due: Some("".to_owned()),
                },
                ActionItem {
                    task: "  ".to_owned(),
                    owner: None,
                    due: None,
                },
            ],
            model: "test-model".to_owned(),
        };

        let clean = messy.cleaned();
        assert_eq!(clean.title, "Weekly sync");
        assert_eq!(clean.bullets, vec!["Real point"]);
        assert!(clean.decisions.is_empty());
        assert_eq!(clean.action_items.len(), 1);
        assert_eq!(clean.action_items[0].owner, None);
        assert_eq!(clean.action_items[0].due, None);
    }

    #[test]
    fn an_unowned_action_item_stays_unowned_rather_than_being_invented() {
        let item = ActionItem {
            task: "Follow up".to_owned(),
            owner: None,
            due: None,
        };
        let json = serde_json::to_value(&item).expect("serialize");
        assert_eq!(json, serde_json::json!({ "task": "Follow up" }));
    }

    #[test]
    fn errors_never_carry_transcript_or_key_material() {
        let error = SummarizeError::Unauthorized {
            provider: "anthropic",
        };
        assert_eq!(error.to_string(), "anthropic rejected the key");
    }
}
