//! Resumeira — local-first meeting notes.
//!
//! The Rust side owns everything risky: audio capture, transcription,
//! provider calls and storage. API keys are read and used here and never
//! cross IPC into the WebView (ADR-0009).

pub mod audio;
pub mod recorder;
pub mod session;
pub mod tray;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            tray::setup(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Resumeira");
}
