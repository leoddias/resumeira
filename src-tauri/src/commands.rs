//! Tauri commands and the shared start/stop path.
//!
//! The tray and the window call the same two functions, so they can never
//! disagree about whether a recording is running. Every state change is
//! emitted to the frontend and reflected in the tray.

use crate::session::{ProcessingStage, RecordingState, SessionManager};
use crate::tray;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Event the frontend listens on. Mirrors `RECORDING_STATE_EVENT` in
/// `src/ipc/types.ts`.
pub const RECORDING_STATE_EVENT: &str = "recording-state";

/// Starts a recording, whatever asked for it.
pub fn start<R: Runtime>(app: &AppHandle<R>) -> RecordingState {
    let Some(manager) = app.try_state::<SessionManager>() else {
        return unavailable();
    };
    let state = manager.start(now_ms());
    publish(app, &state);
    state
}

/// Stops the current recording and hands the audio to the pipeline.
///
/// Returns as soon as capture has stopped. Transcribing and summarizing run
/// in the background and report through the same state events, because they
/// take minutes and the user must be free to close the window, start another
/// meeting, or quit — the audio is already safe on disk either way.
pub fn stop<R: Runtime>(app: &AppHandle<R>) -> RecordingState {
    let Some(manager) = app.try_state::<SessionManager>() else {
        return unavailable();
    };

    let (state, report) = manager.stop();
    publish(app, &state);

    let Some(report) = report else {
        let state = manager.finish();
        publish(app, &state);
        return state;
    };

    // Metadata only — never sample data (docs/CONVENTIONS.md § Privacy).
    {
        for track in &report.tracks {
            match &track.error {
                Some(error) => log::warn!(
                    "{:?} track ended with an error after {} samples: {error}",
                    track.track,
                    track.sample_count
                ),
                None => log::info!(
                    "{:?} track recorded {} samples",
                    track.track,
                    track.sample_count
                ),
            }
        }
        log::info!("meeting saved to {}", report.folder.display());
    }

    spawn_pipeline(app.clone(), report.folder);
    manager.state()
}

/// Runs the post-recording pipeline for one meeting, off the caller's thread.
fn spawn_pipeline<R: Runtime>(app: AppHandle<R>, folder: std::path::PathBuf) {
    tauri::async_runtime::spawn(async move {
        let outcome = run_pipeline(&app, &folder).await;

        match outcome {
            Ok(title) => log::info!("note written for '{title}'"),
            Err(error) => {
                log::warn!("could not turn the meeting into a note: {error}");
                // The recording itself is safe; say what failed rather than
                // sliding back to idle as if nothing had happened.
                if let Some(manager) = app.try_state::<SessionManager>() {
                    let state = manager.fail(error);
                    publish(&app, &state);
                    return;
                }
            }
        }

        if let Some(manager) = app.try_state::<SessionManager>() {
            let state = manager.finish();
            publish(&app, &state);
        }
    });
}

async fn run_pipeline<R: Runtime>(
    app: &AppHandle<R>,
    folder: &std::path::Path,
) -> Result<String, String> {
    let settings = app
        .try_state::<crate::settings_commands::SettingsStore>()
        .map(|store| store.get())
        .unwrap_or_default();
    let retention = settings.audio_retention;

    let secrets: std::sync::Arc<dyn crate::secrets::SecretStore> =
        std::sync::Arc::new(crate::secrets::OsKeychain);

    let context = std::sync::Arc::new(crate::live::LiveContext {
        settings,
        secrets,
        models_root: crate::models_root(app),
    });

    let transcriber = crate::live::LiveTranscriber::new(context.clone());
    let summarizer = crate::live::LiveSummarizer::new(context);

    let stage_app = app.clone();
    let outcome =
        crate::pipeline::process(folder, &transcriber, &summarizer, retention, move |stage| {
            if let Some(manager) = stage_app.try_state::<SessionManager>() {
                let state = manager.set_stage(match stage {
                    crate::pipeline::Stage::Transcribing => ProcessingStage::Transcribing,
                    crate::pipeline::Stage::Summarizing => ProcessingStage::Summarizing,
                    crate::pipeline::Stage::Saving => ProcessingStage::Saving,
                });
                publish(&stage_app, &state);
            }
        })
        .await
        .map_err(|error| error.to_string())?;

    // The index follows the file, never the other way round (ADR-0007). A
    // failed index costs search until the next rebuild, not the note.
    if let Some(meetings) = app.try_state::<crate::meetings_commands::MeetingIndex>() {
        if let Err(error) = meetings.index_note(folder) {
            log::warn!("note written but not indexed: {error}");
        }
    }

    Ok(outcome.title)
}

/// Pushes a state change to the window and the tray.
fn publish<R: Runtime>(app: &AppHandle<R>, state: &RecordingState) {
    if let Err(error) = app.emit(RECORDING_STATE_EVENT, state) {
        log::warn!("could not publish recording state: {error}");
    }
    tray::reflect_state(app, state);
}

fn unavailable() -> RecordingState {
    RecordingState::Failed {
        error: "recording is not available".to_owned(),
    }
}

/// Milliseconds since the Unix epoch, saturating rather than panicking on a
/// clock set before 1970.
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[tauri::command]
pub fn start_recording<R: Runtime>(app: AppHandle<R>) -> RecordingState {
    start(&app)
}

#[tauri::command]
pub fn stop_recording<R: Runtime>(app: AppHandle<R>) -> RecordingState {
    stop(&app)
}

#[tauri::command]
pub fn recording_state<R: Runtime>(app: AppHandle<R>) -> RecordingState {
    let Some(manager) = app.try_state::<SessionManager>() else {
        return unavailable();
    };

    // The UI asks for this every couple of seconds while recording, which
    // makes it the natural place to notice that every device has died. Ending
    // the meeting here rather than letting it appear to run means the audio
    // captured before the failure still becomes a note.
    if manager.all_tracks_dead() {
        log::warn!("every track died mid-recording; ending the meeting");
        return stop(&app);
    }

    manager.state()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_name_matches_the_frontend_contract() {
        // `src/ipc/types.ts` exports RECORDING_STATE_EVENT with this value.
        assert_eq!(RECORDING_STATE_EVENT, "recording-state");
    }

    #[test]
    fn now_is_a_plausible_timestamp() {
        // Well past 2020 and nowhere near saturating.
        assert!(now_ms() > 1_600_000_000_000);
        assert!(now_ms() < i64::MAX);
    }

    #[test]
    fn an_unavailable_manager_reports_failure_the_user_can_retry_from() {
        let state = unavailable();
        assert!(state.can_start());
        assert!(!state.is_capturing());
    }
}
