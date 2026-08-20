//! The real transcriber and summarizer behind [`crate::pipeline`].
//!
//! Everything above this file is testable without a model, a key or a
//! network. This is where those become real, so it stays as thin as it can:
//! resolve the route, fetch the key, call the module that does the work.

use crate::audio::{decoder, Track};
use crate::config::Settings;
use crate::pipeline::{Summarizer, Transcriber};
use crate::secrets::SecretStore;
use crate::summarize::{self, cli, SummarizeError, Summary, SummaryEngine};
use crate::transcribe::routing::{self, Capabilities, Route};
use crate::transcribe::{api, local, model, Engine, TranscribeError, Transcript};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Everything the live implementations need from the app.
pub struct LiveContext {
    pub settings: Settings,
    pub secrets: Arc<dyn SecretStore>,
    pub models_root: PathBuf,
    /// Neutral directory an agent CLI is run from. Never one of the user's
    /// project folders: a CLI started there would read that project's
    /// instructions as context for a meeting they have nothing to do with.
    pub cli_workdir: PathBuf,
}

impl LiveContext {
    /// What the environment can actually do right now, for the router.
    fn capabilities(&self) -> Capabilities {
        let transcription = &self.settings.transcription;
        Capabilities {
            has_api_key: self.secrets.has(transcription.provider.key_name()),
            local_model_installed: model::is_installed(
                &self.models_root,
                &transcription.local_model,
            ),
        }
    }

    /// The engine that will run, or the error explaining why none can.
    pub fn route(&self) -> Result<Route, TranscribeError> {
        routing::route(&self.settings.transcription, self.capabilities())
    }
}

/// Transcribes with whichever engine the user chose.
pub struct LiveTranscriber {
    context: Arc<LiveContext>,
}

impl LiveTranscriber {
    pub fn new(context: Arc<LiveContext>) -> Self {
        Self { context }
    }
}

impl Transcriber for LiveTranscriber {
    async fn transcribe(&self, track: Track, audio: &Path) -> Result<Transcript, TranscribeError> {
        // Resolved per call rather than cached: a route that became invalid
        // (key deleted, model removed) must fail, never quietly switch.
        let route = self.context.route()?;

        match route {
            Route::Local { model: model_id } => {
                let model_path = model::model_path(&self.context.models_root, &model_id)?;
                let samples =
                    decoder::decode_opus_file(audio).map_err(|error| TranscribeError::Io {
                        path: audio.display().to_string(),
                        source: std::io::Error::other(error.to_string()),
                    })?;

                // whisper-rs is CPU-bound and blocking; running it on the
                // async runtime would stall every other task for minutes.
                let track_name = format!("{track:?}");
                tokio::task::spawn_blocking(move || {
                    local::transcribe(&model_path, &samples, None, move |percent| {
                        log::debug!("transcribing {track_name}: {percent}%");
                    })
                })
                .await
                .map_err(|error| {
                    TranscribeError::LocalEngine(format!("transcription did not run: {error}"))
                })?
            }
            Route::Api { provider } => {
                let key = self.context.secrets.get(provider.key_name()).map_err(|_| {
                    TranscribeError::MissingKey {
                        provider: provider.key_name(),
                    }
                })?;
                api::transcribe(provider, &key, audio, None).await
            }
        }
    }
}

/// The engine that will write the notes, resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummarySource {
    Api(crate::summarize::SummaryProvider),
    Cli(cli::AgentCli),
}

/// Reads the user's choice of summary engine, and nothing else.
///
/// Pure and separate from running it so ADR-0020's rule — the engine is
/// whatever the user chose, never what happens to be available — is a tested
/// fact rather than a `match` arm nobody exercises.
pub fn summary_source(settings: &Settings) -> SummarySource {
    match settings.summary_engine {
        SummaryEngine::Api => SummarySource::Api(settings.summary_provider),
        SummaryEngine::Cli => SummarySource::Cli(settings.summary_cli),
    }
}

/// Summarizes with the user's chosen provider and key.
pub struct LiveSummarizer {
    context: Arc<LiveContext>,
}

impl LiveSummarizer {
    pub fn new(context: Arc<LiveContext>) -> Self {
        Self { context }
    }
}

impl Summarizer for LiveSummarizer {
    async fn summarize(&self, transcript: &Transcript) -> Result<Summary, SummarizeError> {
        let settings = &self.context.settings;
        let model = settings.effective_summary_model();
        let messages = summarize::prompt::build(transcript, &summarize::prompt::SummaryOptions {});

        // The engine is whatever the user chose, and only that. A missing key
        // never reaches for an installed CLI, and a missing CLI never reaches
        // for a key (ADR-0020).
        let (source, reply) = match summary_source(settings) {
            SummarySource::Api(provider) => {
                let key = self.context.secrets.get(provider.key_name()).map_err(|_| {
                    SummarizeError::MissingKey {
                        provider: provider.key_name(),
                    }
                })?;
                let reply =
                    summarize::providers::complete(provider, &key, Some(&model), &messages).await?;
                (provider.key_name(), reply)
            }
            SummarySource::Cli(which) => {
                let reply = cli::complete(which, &self.context.cli_workdir, &messages).await?;
                (which.id(), reply)
            }
        };

        summarize::parse::parse_summary(source, &model, &reply).map(Summary::cleaned)
    }
}

/// Human-readable name of the engine that ran, for the note's provenance.
pub fn engine_label(engine: Engine) -> &'static str {
    match engine {
        Engine::Local => "local",
        Engine::Api => "api",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use crate::transcribe::routing::TranscriptionSettings;
    use crate::transcribe::ApiProvider;

    fn context(settings: Settings, store: MemoryStore, models_root: PathBuf) -> LiveContext {
        LiveContext {
            settings,
            secrets: Arc::new(store),
            models_root,
            cli_workdir: PathBuf::from("cli"),
        }
    }

    #[test]
    fn the_api_route_needs_a_key_for_the_selected_provider() {
        let settings = Settings {
            transcription: TranscriptionSettings {
                engine: Engine::Api,
                provider: ApiProvider::Groq,
                ..TranscriptionSettings::default()
            },
            ..Settings::default()
        };

        let without = context(
            settings.clone(),
            MemoryStore::default(),
            PathBuf::from("models"),
        );
        assert!(
            matches!(without.route(), Err(TranscribeError::MissingKey { .. })),
            "no key must be an error, never a silent local run"
        );

        let store = MemoryStore::default();
        store.set("groq", "sk-test").expect("set");
        let with = context(settings, store, PathBuf::from("models"));
        assert!(matches!(with.route(), Ok(Route::Api { .. })));
    }

    #[test]
    fn the_local_route_needs_the_model_on_disk() {
        let settings = Settings::default();
        let missing = context(
            settings.clone(),
            MemoryStore::default(),
            PathBuf::from("no-such-models-dir"),
        );
        assert!(
            matches!(missing.route(), Err(TranscribeError::ModelMissing { .. })),
            "a missing model must be an error, never a silent upload"
        );
    }

    #[test]
    fn having_a_key_does_not_pull_a_local_run_into_the_cloud() {
        // The user chose Local. A key sitting in the keychain for some other
        // purpose must not change where their audio goes.
        let store = MemoryStore::default();
        store.set("groq", "sk-test").expect("set");
        store.set("anthropic", "sk-test").expect("set");

        let ctx = context(
            Settings::default(),
            store,
            PathBuf::from("no-such-models-dir"),
        );
        assert!(matches!(
            ctx.route(),
            Err(TranscribeError::ModelMissing { .. })
        ));
    }

    #[test]
    fn a_cli_engine_is_used_even_when_every_key_is_available() {
        // The failure this forbids: routing a CLI user down the API arm,
        // which would report "no API key" to someone who deliberately has
        // none and chose a CLI for exactly that reason (ADR-0020).
        let settings = Settings {
            summary_engine: SummaryEngine::Cli,
            summary_cli: cli::AgentCli::Gemini,
            summary_provider: crate::summarize::SummaryProvider::OpenAi,
            ..Settings::default()
        };
        assert_eq!(
            summary_source(&settings),
            SummarySource::Cli(cli::AgentCli::Gemini)
        );
    }

    #[test]
    fn an_api_engine_is_used_even_when_a_cli_is_installed() {
        let settings = Settings {
            summary_engine: SummaryEngine::Api,
            summary_provider: crate::summarize::SummaryProvider::Groq,
            // Left over from a previous choice: it must not win.
            summary_cli: cli::AgentCli::Claude,
            ..Settings::default()
        };
        assert_eq!(
            summary_source(&settings),
            SummarySource::Api(crate::summarize::SummaryProvider::Groq)
        );
    }

    #[test]
    fn engine_labels_match_the_ipc_contract() {
        // `src/ipc/meetings.ts` types these as 'local' | 'api'.
        assert_eq!(engine_label(Engine::Local), "local");
        assert_eq!(engine_label(Engine::Api), "api");
    }
}
