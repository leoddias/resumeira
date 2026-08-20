//! User settings: plain JSON in appdata, readable and hand-editable.
//!
//! No secrets live here — API keys are in the OS keychain (ADR-0009, see
//! [`crate::secrets`]). This file is safe to copy, diff, and paste into a bug
//! report, which is the point.
//!
//! Loading is deliberately forgiving: a corrupt or partial file yields
//! defaults with a warning rather than refusing to start. Settings are not
//! worth losing a meeting over — the user can hit record either way.

use crate::summarize::SummaryProvider;
use crate::transcribe::routing::TranscriptionSettings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What to do with the audio once a meeting has been transcribed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AudioRetention {
    /// Keep it next to the note. The default: a transcript can be redone
    /// from audio, but audio cannot be redone from a transcript.
    #[default]
    Keep,
    /// Delete it once a transcript exists, for disk or privacy reasons.
    DeleteAfterTranscription,
}

/// Everything the user can configure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where meetings are written. Empty means "use the default location".
    pub notes_folder: Option<PathBuf>,
    pub transcription: TranscriptionSettings,
    pub summary_provider: SummaryProvider,
    /// Model override; `None` uses the provider's default.
    pub summary_model: Option<String>,
    pub audio_retention: AudioRetention,
    /// Anonymous usage and crash reporting. Off unless explicitly enabled
    /// (ADR-0010) — nothing in this app turns it on by itself.
    pub telemetry_opt_in: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            notes_folder: None,
            transcription: TranscriptionSettings::default(),
            summary_provider: SummaryProvider::Anthropic,
            summary_model: None,
            audio_retention: AudioRetention::Keep,
            telemetry_opt_in: false,
        }
    }
}

impl Settings {
    /// The model to summarize with, honouring an override.
    pub fn effective_summary_model(&self) -> String {
        self.summary_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.summary_provider.default_model().to_owned())
    }

    /// Where meetings go, given the user's home directory.
    pub fn effective_notes_folder(&self, home: &Path) -> PathBuf {
        self.notes_folder
            .clone()
            .filter(|folder| !folder.as_os_str().is_empty())
            .unwrap_or_else(|| home.join("Resumeira"))
    }
}

/// Reads settings from `path`.
///
/// A missing file yields defaults. A malformed file also yields defaults,
/// with a warning: refusing to start because a settings file lost a brace
/// would be a worse outcome than starting with defaults the user can fix.
pub fn load(path: &Path) -> Settings {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
        Err(error) => {
            log::warn!("could not read settings ({}); using defaults", error.kind());
            return Settings::default();
        }
    };

    match serde_json::from_str(&raw) {
        Ok(settings) => settings,
        Err(error) => {
            // Line/column only — a settings file has no secrets, but the
            // habit of not echoing file contents into logs is the point.
            log::warn!(
                "settings file is not valid JSON at line {}; using defaults",
                error.line()
            );
            Settings::default()
        }
    }
}

/// Writes settings to `path`, creating parent directories as needed.
///
/// Written atomically: a crash mid-write must not leave a settings file that
/// fails to parse on next launch.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(settings)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

    let staging = path.with_extension("json.tmp");
    std::fs::write(&staging, json)?;
    match std::fs::rename(&staging, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::Engine;

    #[test]
    fn defaults_never_send_anything_anywhere() {
        let defaults = Settings::default();
        assert_eq!(
            defaults.transcription.engine,
            Engine::Local,
            "a fresh install must not transcribe in the cloud before the user says so"
        );
        assert!(
            !defaults.telemetry_opt_in,
            "telemetry is opt-in only (ADR-0010)"
        );
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings = load(&dir.path().join("nope.json"));
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn a_corrupt_file_yields_defaults_instead_of_refusing_to_start() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ this is not json").expect("write");
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("config.json");

        let mut settings = Settings::default();
        settings.transcription.engine = Engine::Api;
        settings.summary_model = Some("some-model".to_owned());
        settings.audio_retention = AudioRetention::DeleteAfterTranscription;

        save(&path, &settings).expect("save");
        assert_eq!(load(&path), settings);
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.json");
        save(&path, &Settings::default()).expect("save");

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "found {leftovers:?}");
    }

    #[test]
    fn a_file_from_an_older_version_keeps_its_known_fields() {
        // Settings files outlive releases. A file missing newer keys must
        // load, not reset everything the user configured.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{ "telemetryOptIn": true }"#).expect("write");

        let settings = load(&path);
        assert!(settings.telemetry_opt_in);
        assert_eq!(settings.audio_retention, AudioRetention::Keep);
    }

    #[test]
    fn an_unknown_field_from_a_newer_version_is_ignored() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{ "somethingFromTheFuture": 42 }"#).expect("write");
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn the_summary_model_falls_back_to_the_providers_default() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.effective_summary_model(),
            SummaryProvider::Anthropic.default_model()
        );

        settings.summary_model = Some("   ".to_owned());
        assert_eq!(
            settings.effective_summary_model(),
            SummaryProvider::Anthropic.default_model(),
            "a blank override must not become the model id"
        );

        settings.summary_model = Some(" custom-model ".to_owned());
        assert_eq!(settings.effective_summary_model(), "custom-model");
    }

    #[test]
    fn the_notes_folder_defaults_under_the_home_directory() {
        let home = Path::new("/home/leo");
        let mut settings = Settings::default();
        assert_eq!(
            settings.effective_notes_folder(home),
            home.join("Resumeira")
        );

        settings.notes_folder = Some(PathBuf::from("/elsewhere/Meetings"));
        assert_eq!(
            settings.effective_notes_folder(home),
            PathBuf::from("/elsewhere/Meetings")
        );
    }

    #[test]
    fn settings_json_never_contains_a_key_field() {
        // A regression guard for ADR-0009: if someone adds an api_key field
        // here, this fails and they have to think about it.
        let json = serde_json::to_string(&Settings::default()).expect("serialize");
        for forbidden in ["key", "secret", "token", "password"] {
            assert!(
                !json.to_lowercase().contains(forbidden),
                "settings must not carry credentials, found '{forbidden}' in {json}"
            );
        }
    }
}
