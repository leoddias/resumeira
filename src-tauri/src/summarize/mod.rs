//! Turning a transcript into the note a person actually reads.
//!
//! The summary is the product. A recording nobody reopens is a file; a
//! summary someone acts on is the reason this app exists. So the shape below
//! is deliberately narrow and fixed for v0.1 (ADR-0006): a title, roughly
//! five bullets, the decisions taken, and action items with owners.
//!
//! Provider calls happen here in Rust so API keys never cross IPC into the
//! WebView (ADR-0009).

pub mod cli;
pub mod parse;
pub mod prompt;
pub mod providers;

use serde::{Deserialize, Serialize};

/// How the summary is produced.
///
/// Both engines send the transcript to a frontier model — the CLI one just
/// bills it to a subscription the user already has instead of an API key
/// (ADR-0020). Neither is private, and neither is ever chosen implicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SummaryEngine {
    /// A cloud chat API with the user's own key.
    #[default]
    Api,
    /// An agent CLI installed on this machine, under the user's own account.
    Cli,
}

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
///
/// `Debug` is implemented by hand and prints no content: this struct carries
/// the meeting transcript, and a single stray `{:?}` in a log line would put
/// it on disk (docs/CONVENTIONS.md § Privacy).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// Something someone agreed to do.
///
/// `Debug` is redacted — see [`ChatMessage`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
///
/// `Debug` is redacted — see [`ChatMessage`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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

impl std::fmt::Debug for ChatMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatMessage")
            .field("role", &self.role)
            .field("content_chars", &self.content.chars().count())
            .finish()
    }
}

impl std::fmt::Debug for ActionItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionItem")
            .field("has_owner", &self.owner.is_some())
            .field("has_due", &self.due.is_some())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Summary")
            .field("model", &self.model)
            .field("bullets", &self.bullets.len())
            .field("decisions", &self.decisions.len())
            .field("action_items", &self.action_items.len())
            .finish_non_exhaustive()
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

    #[error("{cli} is not installed, or not on this machine's PATH")]
    CliMissing { cli: &'static str },

    #[error("{cli} could not summarize this meeting: {reason}")]
    CliFailed { cli: &'static str, reason: String },

    #[error("there is nothing in this meeting to summarize")]
    NothingToSummarize,

    #[error("the model returned a summary with no content")]
    EmptySummary,
}

impl SummarizeError {
    /// The same failure, safe to write to a log file.
    ///
    /// Two variants carry text this app did not write: `BadResponse` echoes a
    /// provider's error description, and `CliFailed` echoes a line of some
    /// local binary's stderr. Both are worth showing to the person whose
    /// meeting it is — that is how they learn they are logged out — and
    /// neither is worth putting on disk, because an agent CLI writes prompt
    /// and reasoning text to stderr and the prompt is the transcript
    /// (docs/CONVENTIONS.md § Privacy).
    ///
    /// So `Display` goes to the user and this goes to `log::`.
    pub fn log_safe(&self) -> String {
        match self {
            SummarizeError::BadResponse { provider, .. } => {
                format!("{provider} returned an unexpected response")
            }
            SummarizeError::CliFailed { cli, .. } => {
                format!("{cli} could not summarize this meeting")
            }
            other => other.to_string(),
        }
    }
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
    fn debug_output_never_contains_meeting_content() {
        // Guards against someone restoring `#[derive(Debug)]` on types that
        // carry what was said or written about a meeting.
        let secret = "the merger closes on Friday";

        let message = ChatMessage {
            role: ChatRole::User,
            content: secret.to_owned(),
        };
        let rendered = format!("{message:?}");
        assert!(!rendered.contains(secret), "ChatMessage leaked: {rendered}");

        let summary = Summary {
            title: secret.to_owned(),
            bullets: vec![secret.to_owned()],
            decisions: vec![secret.to_owned()],
            action_items: vec![ActionItem {
                task: secret.to_owned(),
                owner: Some("Leo".to_owned()),
                due: None,
            }],
            model: "test-model".to_owned(),
        };
        let rendered = format!("{summary:?}");
        assert!(!rendered.contains(secret), "Summary leaked: {rendered}");
        assert!(
            !rendered.contains("Leo"),
            "an owner's name is meeting content too: {rendered}"
        );
        assert!(rendered.contains("test-model"), "{rendered}");
    }

    #[test]
    fn errors_never_carry_transcript_or_key_material() {
        let error = SummarizeError::Unauthorized {
            provider: "anthropic",
        };
        assert_eq!(error.to_string(), "anthropic rejected the key");
        assert_eq!(error.log_safe(), error.to_string());
    }

    #[test]
    fn what_a_cli_wrote_reaches_the_user_but_never_a_log_file() {
        // An agent CLI writes prompt and reasoning text to stderr, and the
        // prompt is the transcript. The user needs to read it; the log file
        // — the thing that gets attached to bug reports — must not.
        let leaked = "prompt was: Leo said the merger closes on Friday (key sk-ant-0123)";
        let error = SummarizeError::CliFailed {
            cli: "claude",
            reason: leaked.to_owned(),
        };

        assert!(
            error.to_string().contains(leaked),
            "the user must still see why it failed"
        );

        let logged = error.log_safe();
        assert!(!logged.contains("merger"), "leaked to the log: {logged}");
        assert!(!logged.contains("sk-ant"), "leaked to the log: {logged}");
        assert!(logged.contains("claude"), "{logged}");
    }

    #[test]
    fn a_providers_error_text_is_the_same_kind_of_secret() {
        let error = SummarizeError::BadResponse {
            provider: "groq",
            reason: "your prompt beginning 'Leo said the merger' was rejected".to_owned(),
        };
        assert!(!error.log_safe().contains("merger"), "{}", error.log_safe());
    }
}
