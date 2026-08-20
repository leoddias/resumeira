//! Tauri commands and the shared start/stop path.
//!
//! The tray and the window call the same two functions, so they can never
//! disagree about whether a recording is running. Every state change is
//! emitted to the frontend and reflected in the tray.

use crate::session::{RecordingState, SessionManager};
use crate::tray;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

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

/// Stops the current recording and returns the app to idle.
///
/// M1 ends here: the audio is on disk. M2/M3 insert transcription and note
/// writing between the stop and the return to idle.
pub fn stop<R: Runtime>(app: &AppHandle<R>) -> RecordingState {
    let Some(manager) = app.try_state::<SessionManager>() else {
        return unavailable();
    };

    let (state, report) = manager.stop();
    publish(app, &state);

    if let Some(report) = report {
        // Metadata only — never sample data (docs/CONVENTIONS.md § Privacy).
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

    let state = manager.finish();
    publish(app, &state);
    state
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
pub fn recording_state(manager: State<'_, SessionManager>) -> RecordingState {
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
