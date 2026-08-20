//! API keys, kept in the OS keychain and out of everything else.
//!
//! ADR-0009: keys are read and used inside Rust and never cross IPC into the
//! WebView. Nothing here returns key material to a caller that could send it
//! onward — Settings can prove a key exists, replace it, or delete it, but it
//! cannot read it back. That removes a whole class of exfiltration through an
//! injected frontend, and it means a screenshot of the Settings screen is
//! never a leaked credential.

use std::collections::HashMap;
use std::sync::Mutex;

/// Keychain service name. Stable — changing it strands every stored key.
pub const SERVICE: &str = "resumeira";

/// What can go wrong talking to the OS credential store.
///
/// Deliberately carries no key material, and no message that could contain
/// one: a keychain backend error can echo its input.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("the system credential store is unavailable")]
    Unavailable,

    #[error("no key stored for '{account}'")]
    NotFound { account: String },

    #[error("the system credential store refused the operation")]
    Refused,
}

/// Storage for one user's API keys.
///
/// A trait so the app's logic can be tested without touching the real
/// credential store, which is global machine state and would make tests
/// order-dependent and destructive.
pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    fn get(&self, account: &str) -> Result<String, SecretError>;
    fn delete(&self, account: &str) -> Result<(), SecretError>;

    /// Whether a key exists, without retrieving it.
    ///
    /// This is what the UI asks; `get` is for the provider clients only.
    fn has(&self, account: &str) -> bool {
        matches!(self.get(account), Ok(secret) if !secret.is_empty())
    }

    /// Whether a key is *known to be absent*.
    ///
    /// Distinct from `!has(..)`: a credential store that is momentarily
    /// unreachable answers "I don't know", and the readiness gate must not
    /// turn that into "you are not set up" and refuse to record a meeting
    /// that cannot be re-run (ADR-0019). Only a definite `NotFound`, or a
    /// stored-but-empty key, counts as missing.
    fn is_definitely_missing(&self, account: &str) -> bool {
        match self.get(account) {
            Ok(secret) => secret.is_empty(),
            Err(SecretError::NotFound { .. }) => true,
            Err(_) => false,
        }
    }
}

/// The real store: Windows Credential Manager, macOS Keychain, Secret Service.
pub struct OsKeychain;

impl SecretStore for OsKeychain {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(map_error)?;
        entry.set_password(secret).map_err(map_error)
    }

    fn get(&self, account: &str) -> Result<String, SecretError> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(map_error)?;
        entry.get_password().map_err(|error| match error {
            keyring::Error::NoEntry => SecretError::NotFound {
                account: account.to_owned(),
            },
            other => map_error(other),
        })
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(map_error)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            // Deleting a key that is not there is the state the caller wanted.
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(other) => Err(map_error(other)),
        }
    }
}

/// Maps a keyring error without letting its message through.
///
/// Backend errors can quote what they were handed, so only the *kind* is
/// kept. The variant is enough to tell the user what to do.
fn map_error(error: keyring::Error) -> SecretError {
    match error {
        keyring::Error::NoEntry => SecretError::NotFound {
            account: String::new(),
        },
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            SecretError::Unavailable
        }
        _ => SecretError::Refused,
    }
}

/// A hint the UI can show to confirm *which* key is stored, without
/// revealing it.
///
/// Shows at most the last four characters. A short or empty secret shows no
/// characters at all rather than most of itself — the failure mode of a
/// naive mask is leaking a whole short key.
pub fn masked_hint(secret: &str) -> String {
    let visible: Vec<char> = secret.chars().collect();
    if visible.len() < 12 {
        return "••••".to_owned();
    }
    let tail: String = visible[visible.len() - 4..].iter().collect();
    format!("••••{tail}")
}

/// In-memory store for tests. Never used by the app.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        match self.entries.lock() {
            Ok(mut entries) => {
                entries.insert(account.to_owned(), secret.to_owned());
                Ok(())
            }
            Err(_) => Err(SecretError::Unavailable),
        }
    }

    fn get(&self, account: &str) -> Result<String, SecretError> {
        match self.entries.lock() {
            Ok(entries) => entries
                .get(account)
                .cloned()
                .ok_or_else(|| SecretError::NotFound {
                    account: account.to_owned(),
                }),
            Err(_) => Err(SecretError::Unavailable),
        }
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match self.entries.lock() {
            Ok(mut entries) => {
                entries.remove(account);
                Ok(())
            }
            Err(_) => Err(SecretError::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::SummaryProvider;
    use crate::transcribe::ApiProvider;

    #[test]
    fn a_stored_key_round_trips() {
        let store = MemoryStore::default();
        store.set("groq", "sk-test-value").expect("set");
        assert_eq!(store.get("groq").expect("get"), "sk-test-value");
    }

    #[test]
    fn presence_can_be_checked_without_reading_the_key() {
        let store = MemoryStore::default();
        assert!(!store.has("groq"));
        store.set("groq", "sk-test-value").expect("set");
        assert!(store.has("groq"));
    }

    #[test]
    fn an_empty_stored_value_does_not_count_as_a_key() {
        // Otherwise a user who saved a blank field would see "key configured"
        // and then get an authentication error they cannot explain.
        let store = MemoryStore::default();
        store.set("groq", "").expect("set");
        assert!(!store.has("groq"));
    }

    #[test]
    fn deleting_a_missing_key_is_not_an_error() {
        let store = MemoryStore::default();
        assert!(store.delete("groq").is_ok());
    }

    #[test]
    fn deleting_removes_only_the_named_account() {
        let store = MemoryStore::default();
        store.set("groq", "a").expect("set");
        store.set("openai", "b").expect("set");
        store.delete("groq").expect("delete");
        assert!(!store.has("groq"));
        assert!(store.has("openai"));
    }

    #[test]
    fn the_mask_never_reveals_a_short_key() {
        // A naive "show the last four" leaks most of a short secret.
        assert_eq!(masked_hint(""), "••••");
        assert_eq!(masked_hint("sk-abc"), "••••");
        assert_eq!(masked_hint("sk-abcdefgh"), "••••");
    }

    #[test]
    fn the_mask_shows_only_the_tail_of_a_long_key() {
        let hint = masked_hint("sk-proj-0123456789abcdef");
        assert_eq!(hint, "••••cdef");
        assert!(!hint.contains("0123456789"));
    }

    #[test]
    fn the_mask_counts_characters_not_bytes() {
        // A multibyte key must not panic on a byte-index slice.
        let hint = masked_hint("çãoçãoçãoçãoção");
        assert!(hint.starts_with("••••"));
    }

    #[test]
    fn provider_account_names_are_what_the_store_is_keyed_by() {
        // The two provider enums must agree on the account name, or a key
        // saved for transcription would be invisible to summarization.
        let store = MemoryStore::default();
        store
            .set(ApiProvider::OpenAi.key_name(), "shared")
            .expect("set");
        assert!(store.has(SummaryProvider::OpenAi.key_name()));
        assert_eq!(
            ApiProvider::Groq.key_name(),
            SummaryProvider::Groq.key_name()
        );
    }

    #[test]
    fn errors_never_carry_key_material() {
        let error = SecretError::NotFound {
            account: "groq".to_owned(),
        };
        assert_eq!(error.to_string(), "no key stored for 'groq'");
        assert_eq!(
            SecretError::Unavailable.to_string(),
            "the system credential store is unavailable"
        );
    }
}

#[cfg(test)]
mod availability {
    use super::*;

    /// A store whose backend is down — every read fails the same way.
    struct Unreachable;

    impl SecretStore for Unreachable {
        fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
            Err(SecretError::Unavailable)
        }
        fn get(&self, _: &str) -> Result<String, SecretError> {
            Err(SecretError::Unavailable)
        }
        fn delete(&self, _: &str) -> Result<(), SecretError> {
            Err(SecretError::Unavailable)
        }
    }

    #[test]
    fn an_unreachable_store_is_not_the_same_as_no_key() {
        let store = Unreachable;
        assert!(
            !store.has("groq"),
            "nothing can be read, so nothing is known"
        );
        assert!(
            !store.is_definitely_missing("groq"),
            "'I cannot tell' must never become 'you are not set up' - that \
             would refuse a meeting the user cannot record again"
        );
    }

    #[test]
    fn a_store_that_answers_reports_the_truth_either_way() {
        let store = MemoryStore::default();
        assert!(store.is_definitely_missing("groq"));

        store.set("groq", "sk-test").expect("set");
        assert!(!store.is_definitely_missing("groq"));
        assert!(store.has("groq"));
    }
}
