//! Tauri commands for settings and API keys.
//!
//! On the key direction of travel (ADR-0009): a key necessarily travels
//! *inward* once, when the user pastes it into Settings. It never travels
//! back out. `key_status` answers "is one stored, and what does its tail look
//! like" — enough for the UI to be useful, useless to anyone who scrapes it.

use crate::config::{self, Settings};
use crate::secrets::{SecretError, SecretStore};
use crate::summarize::SummaryProvider;
use crate::transcribe::ApiProvider;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, State};

/// Where settings live, and the in-memory copy the app is running with.
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load_from(path: PathBuf) -> Self {
        let current = config::load(&path);
        Self {
            path,
            current: Mutex::new(current),
        }
    }

    pub fn get(&self) -> Settings {
        match self.current.lock() {
            Ok(settings) => settings.clone(),
            // Settings are not worth taking the app down for.
            Err(_) => Settings::default(),
        }
    }

    pub fn put(&self, next: Settings) -> Result<(), String> {
        config::save(&self.path, &next).map_err(|error| error.kind().to_string())?;
        match self.current.lock() {
            Ok(mut current) => {
                *current = next;
                Ok(())
            }
            Err(_) => Err("settings are unavailable".to_owned()),
        }
    }
}

/// The store the app talks to. Boxed so tests can swap in a memory store.
pub struct Secrets(pub Box<dyn SecretStore>);

/// What the UI is allowed to know about a stored key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    /// Keychain account name, which is also the provider id.
    pub account: String,
    pub configured: bool,
    /// A masked tail so the user can tell which key is stored. Never the key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Every provider account the app can hold a key for, deduplicated.
///
/// Transcription and summarization share account names where they share a
/// provider — a key pasted once works for both.
pub fn known_accounts() -> Vec<&'static str> {
    let mut accounts = vec![
        SummaryProvider::Anthropic.key_name(),
        SummaryProvider::OpenAi.key_name(),
        SummaryProvider::Groq.key_name(),
        ApiProvider::Groq.key_name(),
        ApiProvider::OpenAi.key_name(),
    ];
    accounts.sort_unstable();
    accounts.dedup();
    accounts
}

#[tauri::command]
pub fn get_settings(store: State<'_, SettingsStore>) -> Settings {
    store.get()
}

#[tauri::command]
pub fn save_settings(store: State<'_, SettingsStore>, settings: Settings) -> Result<(), String> {
    store.put(settings)
}

#[tauri::command]
pub fn key_status(secrets: State<'_, Secrets>) -> Vec<KeyStatus> {
    known_accounts()
        .into_iter()
        .map(|account| status_for(secrets.0.as_ref(), account))
        .collect()
}

#[tauri::command]
pub fn set_api_key(
    secrets: State<'_, Secrets>,
    account: String,
    key: String,
) -> Result<KeyStatus, String> {
    let key = key.trim();
    if key.is_empty() {
        // Saving a blank field would show "configured" and then fail at the
        // provider with an error the user cannot connect to this action.
        return Err("the key is empty".to_owned());
    }
    if !known_accounts().contains(&account.as_str()) {
        return Err(format!("unknown provider '{account}'"));
    }

    secrets.0.set(&account, key).map_err(describe)?;
    Ok(status_for(secrets.0.as_ref(), &account))
}

#[tauri::command]
pub fn delete_api_key(secrets: State<'_, Secrets>, account: String) -> Result<KeyStatus, String> {
    secrets.0.delete(&account).map_err(describe)?;
    Ok(status_for(secrets.0.as_ref(), &account))
}

/// Reads a key's presence and hint. Note this never returns the key itself.
fn status_for(store: &dyn SecretStore, account: &str) -> KeyStatus {
    match store.get(account) {
        Ok(secret) if !secret.is_empty() => KeyStatus {
            account: account.to_owned(),
            configured: true,
            hint: Some(crate::secrets::masked_hint(&secret)),
        },
        _ => KeyStatus {
            account: account.to_owned(),
            configured: false,
            hint: None,
        },
    }
}

fn describe(error: SecretError) -> String {
    error.to_string()
}

/// Settings file location under the app's config directory.
pub fn settings_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    match app.path().app_config_dir() {
        Ok(dir) => dir.join("config.json"),
        Err(error) => {
            log::warn!("no config directory ({error}); keeping settings next to the app");
            PathBuf::from("config.json")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn a_provider_shared_between_features_has_one_account() {
        let accounts = known_accounts();
        let groq_entries = accounts.iter().filter(|a| **a == "groq").count();
        assert_eq!(
            groq_entries, 1,
            "a key pasted once must serve both transcription and summaries"
        );
        assert!(accounts.contains(&"anthropic"));
        assert!(accounts.contains(&"openai"));
    }

    #[test]
    fn status_reports_absence_without_a_hint() {
        let store = MemoryStore::default();
        let status = status_for(&store, "groq");
        assert!(!status.configured);
        assert_eq!(status.hint, None);
    }

    #[test]
    fn status_never_contains_the_key() {
        let store = MemoryStore::default();
        store
            .set("groq", "sk-secret-0123456789abcdef")
            .expect("set");

        let status = status_for(&store, "groq");
        assert!(status.configured);

        let json = serde_json::to_string(&status).expect("serialize");
        assert!(
            !json.contains("sk-secret"),
            "the key must never reach the UI, got {json}"
        );
        assert!(json.contains("cdef"), "the tail hint is expected in {json}");
    }

    #[test]
    fn settings_round_trip_through_the_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = SettingsStore::load_from(dir.path().join("config.json"));
        assert_eq!(store.get(), Settings::default());

        let next = Settings {
            telemetry_opt_in: true,
            ..Settings::default()
        };
        store.put(next.clone()).expect("put");

        assert_eq!(store.get(), next);
        // And it survived to disk, not just memory.
        let reloaded = SettingsStore::load_from(dir.path().join("config.json"));
        assert_eq!(reloaded.get(), next);
    }
}
