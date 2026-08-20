//! Resumeira — local-first meeting notes.
//!
//! The Rust side owns everything risky: audio capture, transcription,
//! provider calls and storage. API keys are read and used here and never
//! cross IPC into the WebView (ADR-0009).

pub mod audio;
pub mod commands;
pub mod recorder;
pub mod session;
pub mod tracks;
pub mod transcribe;
pub mod tray;

use session::SessionManager;
use std::path::PathBuf;
use tauri::Manager;

/// Folder meetings are written into, until Settings makes it configurable.
///
/// Defaults to `~/Resumeira`; falls back to the current directory rather
/// than failing to launch, so a machine with no resolvable home still runs.
fn default_notes_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    match app.path().home_dir() {
        Ok(home) => home.join("Resumeira"),
        Err(error) => {
            log::warn!("no home directory ({error}); writing meetings next to the app");
            PathBuf::from("Resumeira")
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::start_recording,
            commands::stop_recording,
            commands::recording_state,
        ])
        .setup(|app| {
            let notes_root = default_notes_root(app.handle());
            app.manage(SessionManager::new(
                notes_root,
                Box::new(tracks::LiveTrackFactory),
            ));
            tray::setup(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Resumeira");
}
