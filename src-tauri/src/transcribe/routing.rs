//! Deciding which engine transcribes a meeting.
//!
//! This module exists to make one guarantee mechanical: **there is no
//! implicit fallback in either direction** (ADR-0005). If the user chose
//! Local and the model is missing, that is an error the user sees — not a
//! quiet upload of their meeting to a cloud provider. If the user chose Api
//! and there is no key, that is an error too — not a silent switch to a slow
//! local run they did not ask for.

use super::{ApiProvider, Engine, TranscribeError};
use serde::{Deserialize, Serialize};

/// The transcription part of the user's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionSettings {
    pub engine: Engine,
    pub provider: ApiProvider,
    /// Identifier of the local Whisper model, e.g. `large-v3-turbo`.
    pub local_model: String,
}

impl Default for TranscriptionSettings {
    /// Local by default: a fresh install must not be able to send a meeting
    /// anywhere before the user has said so.
    fn default() -> Self {
        Self {
            engine: Engine::Local,
            provider: ApiProvider::Groq,
            local_model: "large-v3-turbo".to_owned(),
        }
    }
}

/// What the environment can actually do right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// A key for the selected provider exists in the keychain.
    pub has_api_key: bool,
    /// The selected local model is downloaded and verified.
    pub local_model_installed: bool,
}

/// The engine that will run, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Local { model: String },
    Api { provider: ApiProvider },
}

impl Route {
    /// Whether taking this route sends audio off the machine.
    ///
    /// The UI uses this to warn before the first upload; it is deliberately a
    /// property of the route, not of the settings, so it cannot disagree with
    /// what actually runs.
    pub fn leaves_the_machine(&self) -> bool {
        matches!(self, Route::Api { .. })
    }
}

/// Resolves settings plus capabilities into the engine that will run.
///
/// Returns an error rather than substituting a different engine. Callers
/// surface that error to the user; nothing retries with the other one.
pub fn route(
    settings: &TranscriptionSettings,
    capabilities: Capabilities,
) -> Result<Route, TranscribeError> {
    match settings.engine {
        Engine::Api => {
            if capabilities.has_api_key {
                Ok(Route::Api {
                    provider: settings.provider,
                })
            } else {
                Err(TranscribeError::MissingKey {
                    provider: settings.provider.key_name(),
                })
            }
        }
        Engine::Local => {
            if capabilities.local_model_installed {
                Ok(Route::Local {
                    model: settings.local_model.clone(),
                })
            } else {
                Err(TranscribeError::ModelMissing {
                    model: settings.local_model.clone(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTHING_AVAILABLE: Capabilities = Capabilities {
        has_api_key: false,
        local_model_installed: false,
    };
    const EVERYTHING_AVAILABLE: Capabilities = Capabilities {
        has_api_key: true,
        local_model_installed: true,
    };

    fn settings(engine: Engine) -> TranscriptionSettings {
        TranscriptionSettings {
            engine,
            ..TranscriptionSettings::default()
        }
    }

    #[test]
    fn a_fresh_install_defaults_to_local() {
        let defaults = TranscriptionSettings::default();
        assert_eq!(defaults.engine, Engine::Local);
        assert_eq!(defaults.local_model, "large-v3-turbo");
    }

    #[test]
    fn local_runs_locally_when_the_model_is_installed() {
        let route = route(
            &settings(Engine::Local),
            Capabilities {
                has_api_key: false,
                local_model_installed: true,
            },
        )
        .expect("local route");
        assert_eq!(
            route,
            Route::Local {
                model: "large-v3-turbo".to_owned()
            }
        );
        assert!(!route.leaves_the_machine());
    }

    #[test]
    fn api_runs_remotely_when_a_key_exists() {
        let route = route(
            &settings(Engine::Api),
            Capabilities {
                has_api_key: true,
                local_model_installed: false,
            },
        )
        .expect("api route");
        assert_eq!(
            route,
            Route::Api {
                provider: ApiProvider::Groq
            }
        );
        assert!(route.leaves_the_machine());
    }

    // The two tests below are the whole point of this module.

    #[test]
    fn a_missing_local_model_never_silently_uploads_the_meeting() {
        let error = route(&settings(Engine::Local), EVERYTHING_AVAILABLE_BUT_NO_MODEL)
            .expect_err("a missing model must be an error, not an upload");
        assert!(matches!(error, TranscribeError::ModelMissing { .. }));
    }

    const EVERYTHING_AVAILABLE_BUT_NO_MODEL: Capabilities = Capabilities {
        has_api_key: true,
        local_model_installed: false,
    };

    #[test]
    fn a_missing_key_never_silently_falls_back_to_a_local_run() {
        let error = route(
            &settings(Engine::Api),
            Capabilities {
                has_api_key: false,
                local_model_installed: true,
            },
        )
        .expect_err("a missing key must be an error, not a local run");
        assert!(matches!(error, TranscribeError::MissingKey { .. }));
    }

    #[test]
    fn nothing_available_fails_for_the_reason_the_user_chose() {
        assert!(matches!(
            route(&settings(Engine::Local), NOTHING_AVAILABLE),
            Err(TranscribeError::ModelMissing { .. })
        ));
        assert!(matches!(
            route(&settings(Engine::Api), NOTHING_AVAILABLE),
            Err(TranscribeError::MissingKey { .. })
        ));
    }

    #[test]
    fn the_chosen_engine_alone_decides_where_audio_goes() {
        // Whatever the capabilities, Local never leaves and Api always does.
        for capabilities in [NOTHING_AVAILABLE, EVERYTHING_AVAILABLE] {
            if let Ok(route) = route(&settings(Engine::Local), capabilities) {
                assert!(!route.leaves_the_machine());
            }
            if let Ok(route) = route(&settings(Engine::Api), capabilities) {
                assert!(route.leaves_the_machine());
            }
        }
    }
}
