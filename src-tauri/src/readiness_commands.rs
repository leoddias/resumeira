//! The Tauri command behind the setup screen (ADR-0019).
//!
//! Deliberately thin: it turns the app's real environment into the three
//! observations [`crate::readiness::evaluate`] needs and hands the answer
//! back. All the deciding happens in that pure function, where it is tested.

use crate::readiness::{self, Observed, Readiness};
use crate::settings_commands::{Secrets, SettingsStore};
use crate::summarize::cli;
use crate::transcribe::model;
use tauri::{AppHandle, Runtime, State};

/// Resolves what the app can actually do right now.
///
/// The frontend calls this on launch and again after anything that could
/// change the answer — a settings save, a stored key, a finished download —
/// so fixing the cause unblocks the app without a restart.
#[tauri::command]
pub fn check_readiness<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, SettingsStore>,
    secrets: State<'_, Secrets>,
) -> Readiness {
    evaluate(&app, &store, &secrets)
}

/// The readiness of the running app, for callers that hold an `AppHandle`
/// rather than command state — the record path, which has to consult exactly
/// the same verdict the setup screen shows.
///
/// `None` when settings or the keychain are not managed at all: without them
/// nothing can be resolved, and refusing on that basis would let a broken app
/// state silently eat the record button.
pub fn current<R: Runtime>(app: &AppHandle<R>) -> Option<Readiness> {
    use tauri::Manager;

    let store = app.try_state::<SettingsStore>()?;
    let secrets = app.try_state::<Secrets>()?;
    Some(evaluate(app, &store, &secrets))
}

fn evaluate<R: Runtime>(app: &AppHandle<R>, store: &SettingsStore, secrets: &Secrets) -> Readiness {
    let settings = store.get();
    let models_root = crate::models_root(app);

    readiness::evaluate(
        &settings,
        &Observed {
            // "Not definitely missing" rather than "present": a credential
            // store that hiccups must not refuse a meeting over a key the
            // user actually has (ADR-0019).
            key_stored: &|account| !secrets.0.is_definitely_missing(account),
            model_installed: &|id| model::is_installed(&models_root, id),
            cli_available: &|which| cli::is_available(which),
        },
    )
}

/// Why recording is refused, as one sentence for the user.
pub fn blocked_reason(readiness: &Readiness) -> Option<String> {
    if readiness.can_record {
        return None;
    }

    let missing = [&readiness.transcription, &readiness.summary]
        .into_iter()
        .filter(|step| !step.ready)
        .map(|step| step.detail.clone())
        .collect::<Vec<_>>()
        .join(" and ");

    Some(format!(
        "Resumeira is not set up yet ({missing}). Finish setup before recording — \
         a meeting cannot be recorded a second time."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::summarize::cli::AgentCli;

    fn readiness_with(model_installed: bool, key_stored: bool) -> Readiness {
        readiness::evaluate(
            &Settings::default(),
            &Observed {
                key_stored: &move |_| key_stored,
                model_installed: &move |_| model_installed,
                cli_available: &|_: AgentCli| false,
            },
        )
    }

    #[test]
    fn a_ready_app_has_no_reason_to_refuse() {
        assert_eq!(blocked_reason(&readiness_with(true, true)), None);
    }

    #[test]
    fn the_reason_names_every_missing_piece_not_just_the_first() {
        let reason = blocked_reason(&readiness_with(false, false)).expect("blocked");
        assert!(reason.contains("large-v3-turbo"), "{reason}");
        assert!(reason.contains("anthropic"), "{reason}");
    }

    #[test]
    fn the_reason_says_why_it_matters() {
        // The whole point of blocking is that the input cannot be recreated.
        let reason = blocked_reason(&readiness_with(false, true)).expect("blocked");
        assert!(reason.contains("second time"), "{reason}");
    }
}
