//! Can this app do what it is configured to do, right now? (ADR-0019)
//!
//! The failure this module exists to prevent: a fresh install defaults to
//! local transcription with a model nobody has downloaded yet, so recording
//! worked, the meeting ended, and only then did the pipeline report "the
//! local model 'large-v3-turbo' is not installed". A meeting is the one input
//! the user cannot produce again, so the check has to happen *before* the
//! record button does anything — not after the audio is already history.
//!
//! [`evaluate`] is a pure function over the settings plus three observations
//! (is that key stored, is that model on disk, is that CLI on the PATH). It
//! reuses [`routing::route`] for the transcription half rather than
//! re-deciding it, so the screen the user reads and the code that runs the
//! meeting can never disagree about what will happen.

use crate::config::Settings;
use crate::summarize::{cli::AgentCli, SummaryEngine};
use crate::transcribe::routing::{self, Capabilities, Route};
use crate::transcribe::TranscribeError;
use serde::Serialize;

/// What the user has to do before a step can run.
///
/// Structured rather than a sentence: the UI turns each of these into the
/// button that fixes it, and a string cannot be turned into a button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Blocker {
    /// The chosen local Whisper model is not downloaded.
    ModelMissing { model: String },
    /// No key is stored for the provider this step would call.
    KeyMissing { account: String },
    /// The chosen agent CLI is not installed on this machine.
    CliMissing { cli: String, name: String },
}

/// One half of the pipeline: can it run, and what is it going to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReadiness {
    pub ready: bool,
    /// What will run, in the user's words — "Local · large-v3-turbo".
    pub detail: String,
    /// Whether this step sends meeting content off the machine.
    pub leaves_the_machine: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<Blocker>,
}

/// The whole answer, as the setup screen renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub transcription: StepReadiness,
    pub summary: StepReadiness,
    /// Both steps can run. Recording is refused unless this is true, so a
    /// meeting is never captured into a pipeline that cannot finish it.
    pub can_record: bool,
}

/// What the app can observe about its environment.
///
/// Injected as closures so [`evaluate`] can be tested against every
/// combination without a keychain, a 1.6 GB model, or a CLI installed.
pub struct Observed<'a> {
    pub key_stored: &'a dyn Fn(&str) -> bool,
    pub model_installed: &'a dyn Fn(&str) -> bool,
    pub cli_available: &'a dyn Fn(AgentCli) -> bool,
}

/// Resolves the configured route into what will happen, or what is missing.
pub fn evaluate(settings: &Settings, observed: &Observed<'_>) -> Readiness {
    let transcription = transcription_readiness(settings, observed);
    let summary = summary_readiness(settings, observed);

    Readiness {
        can_record: transcription.ready && summary.ready,
        transcription,
        summary,
    }
}

fn transcription_readiness(settings: &Settings, observed: &Observed<'_>) -> StepReadiness {
    let transcription = &settings.transcription;
    let capabilities = Capabilities {
        has_api_key: (observed.key_stored)(transcription.provider.key_name()),
        local_model_installed: (observed.model_installed)(&transcription.local_model),
    };

    match routing::route(transcription, capabilities) {
        Ok(route) => StepReadiness {
            ready: true,
            leaves_the_machine: route.leaves_the_machine(),
            detail: match &route {
                Route::Local { model } => format!("Local · {model}"),
                Route::Api { provider } => format!("{} API", provider.key_name()),
            },
            blocker: None,
        },
        Err(TranscribeError::ModelMissing { model }) => StepReadiness {
            ready: false,
            leaves_the_machine: false,
            detail: format!("Local · {model}"),
            blocker: Some(Blocker::ModelMissing { model }),
        },
        Err(TranscribeError::MissingKey { provider }) => StepReadiness {
            ready: false,
            leaves_the_machine: true,
            detail: format!("{provider} API"),
            blocker: Some(Blocker::KeyMissing {
                account: provider.to_owned(),
            }),
        },
        // `route` returns only those two. Anything else is a code change that
        // has to be reflected here, and until it is, the honest answer is
        // "not ready" rather than a record button that lies.
        Err(other) => StepReadiness {
            ready: false,
            leaves_the_machine: false,
            detail: other.to_string(),
            blocker: None,
        },
    }
}

fn summary_readiness(settings: &Settings, observed: &Observed<'_>) -> StepReadiness {
    // Both summary engines send the transcript to a frontier model. The CLI
    // one bills a subscription instead of a key; it is not private, and the
    // UI must never imply otherwise (ADR-0020).
    match settings.summary_engine {
        SummaryEngine::Api => {
            let account = settings.summary_provider.key_name();
            let ready = (observed.key_stored)(account);
            StepReadiness {
                ready,
                leaves_the_machine: true,
                detail: format!("{account} · {}", settings.effective_summary_model()),
                blocker: (!ready).then(|| Blocker::KeyMissing {
                    account: account.to_owned(),
                }),
            }
        }
        SummaryEngine::Cli => {
            let cli = settings.summary_cli;
            let ready = (observed.cli_available)(cli);
            StepReadiness {
                ready,
                leaves_the_machine: true,
                detail: format!("{} (CLI)", cli.display_name()),
                blocker: (!ready).then(|| Blocker::CliMissing {
                    cli: cli.id().to_owned(),
                    name: cli.display_name().to_owned(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::SummaryProvider;
    use crate::transcribe::routing::TranscriptionSettings;
    use crate::transcribe::{ApiProvider, Engine};

    /// A machine with nothing set up — the state a fresh install is in.
    fn bare() -> Observed<'static> {
        Observed {
            key_stored: &|_| false,
            model_installed: &|_| false,
            cli_available: &|_| false,
        }
    }

    fn everything() -> Observed<'static> {
        Observed {
            key_stored: &|_| true,
            model_installed: &|_| true,
            cli_available: &|_| true,
        }
    }

    #[test]
    fn a_fresh_install_cannot_record() {
        // This is the regression test for the bug that motivated ADR-0019:
        // defaults point at a local model that no fresh install has.
        let readiness = evaluate(&Settings::default(), &bare());

        assert!(!readiness.can_record);
        assert_eq!(
            readiness.transcription.blocker,
            Some(Blocker::ModelMissing {
                model: "large-v3-turbo".to_owned()
            })
        );
    }

    #[test]
    fn a_fully_set_up_machine_can_record() {
        let readiness = evaluate(&Settings::default(), &everything());
        assert!(readiness.can_record);
        assert!(readiness.transcription.blocker.is_none());
        assert!(readiness.summary.blocker.is_none());
    }

    #[test]
    fn a_missing_summary_key_blocks_recording_even_when_transcription_is_ready() {
        // The model is installed, so audio would record and transcribe — and
        // then die with no note. Half a pipeline is not permission to record.
        let readiness = evaluate(
            &Settings::default(),
            &Observed {
                key_stored: &|_| false,
                model_installed: &|_| true,
                cli_available: &|_| false,
            },
        );

        assert!(readiness.transcription.ready);
        assert!(!readiness.summary.ready);
        assert!(!readiness.can_record);
        assert_eq!(
            readiness.summary.blocker,
            Some(Blocker::KeyMissing {
                account: "anthropic".to_owned()
            })
        );
    }

    #[test]
    fn an_installed_cli_replaces_the_need_for_a_summary_key() {
        let settings = Settings {
            summary_engine: SummaryEngine::Cli,
            summary_cli: AgentCli::Claude,
            ..Settings::default()
        };

        let readiness = evaluate(
            &settings,
            &Observed {
                key_stored: &|_| false,
                model_installed: &|_| true,
                cli_available: &|cli| cli == AgentCli::Claude,
            },
        );

        assert!(readiness.can_record, "a CLI is a complete summary engine");
        assert!(readiness.summary.detail.contains("Claude Code"));
    }

    #[test]
    fn the_chosen_cli_is_the_one_that_has_to_be_installed() {
        // Having `claude` does not make a user who selected `gemini` ready:
        // the engine is explicit, and substituting one is exactly the kind of
        // silent switch ADR-0020 forbids.
        let settings = Settings {
            summary_engine: SummaryEngine::Cli,
            summary_cli: AgentCli::Gemini,
            ..Settings::default()
        };

        let readiness = evaluate(
            &settings,
            &Observed {
                key_stored: &|_| true,
                model_installed: &|_| true,
                cli_available: &|cli| cli == AgentCli::Claude,
            },
        );

        assert!(!readiness.can_record);
        assert_eq!(
            readiness.summary.blocker,
            Some(Blocker::CliMissing {
                cli: "gemini".to_owned(),
                name: "Gemini CLI".to_owned()
            })
        );
    }

    #[test]
    fn a_key_for_another_provider_does_not_count() {
        let settings = Settings {
            summary_provider: SummaryProvider::OpenAi,
            transcription: TranscriptionSettings {
                engine: Engine::Api,
                provider: ApiProvider::Groq,
                ..TranscriptionSettings::default()
            },
            ..Settings::default()
        };

        let readiness = evaluate(
            &settings,
            &Observed {
                key_stored: &|account| account == "groq",
                model_installed: &|_| false,
                cli_available: &|_| false,
            },
        );

        assert!(readiness.transcription.ready, "the groq key covers whisper");
        assert_eq!(
            readiness.summary.blocker,
            Some(Blocker::KeyMissing {
                account: "openai".to_owned()
            })
        );
        assert!(!readiness.can_record);
    }

    #[test]
    fn the_screen_says_when_a_meeting_would_leave_the_machine() {
        let local = evaluate(&Settings::default(), &everything());
        assert!(
            !local.transcription.leaves_the_machine,
            "local transcription stays here"
        );
        assert!(
            local.summary.leaves_the_machine,
            "there is no local summarizer; the user must be told"
        );

        let cli_summary = Settings {
            summary_engine: SummaryEngine::Cli,
            ..Settings::default()
        };
        let readiness = evaluate(&cli_summary, &everything());
        assert!(
            readiness.summary.leaves_the_machine,
            "a CLI uploads the transcript too - it is not a local summarizer"
        );
    }

    #[test]
    fn readiness_never_carries_meeting_content_or_key_material() {
        let readiness = evaluate(&Settings::default(), &bare());
        let json = serde_json::to_string(&readiness).expect("serialize");
        // Account *names* are expected here — the UI needs them to say which
        // key is missing. Key material is not, and neither is anything from a
        // meeting: this payload is built from settings and file existence
        // only, and this guards the day someone adds a field to it.
        for forbidden in ["secret", "password", "sk-", "bearer"] {
            assert!(!json.to_lowercase().contains(forbidden), "{json}");
        }
    }
}
