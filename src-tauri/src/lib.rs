//! Resumeira — local-first meeting notes.
//!
//! The Rust side owns everything risky: audio capture, transcription,
//! provider calls and storage. API keys are read and used here and never
//! cross IPC into the WebView (ADR-0009).

pub mod audio;
pub mod commands;
pub mod config;
pub mod index;
pub mod pipeline;
pub mod recorder;
pub mod secrets;
pub mod session;
pub mod settings_commands;
pub mod storage;
pub mod summarize;
pub mod tracks;
pub mod transcribe;
pub mod tray;

use session::SessionManager;
use std::path::PathBuf;
use tauri::Manager;

/// Folder meetings are written into: the user's choice, or `~/Resumeira`.
///
/// Falls back to a relative path rather than failing to launch, so a machine
/// with no resolvable home still runs.
fn notes_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &settings_commands::SettingsStore,
) -> PathBuf {
    match app.path().home_dir() {
        Ok(home) => settings.get().effective_notes_folder(&home),
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
            settings_commands::get_settings,
            settings_commands::save_settings,
            settings_commands::key_status,
            settings_commands::set_api_key,
            settings_commands::delete_api_key,
        ])
        .setup(|app| {
            let settings = settings_commands::SettingsStore::load_from(
                settings_commands::settings_path(app.handle()),
            );
            let notes_root = notes_root(app.handle(), &settings);
            app.manage(settings);
            app.manage(settings_commands::Secrets(Box::new(
                crate::secrets::OsKeychain,
            )));
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
