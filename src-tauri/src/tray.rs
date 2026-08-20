//! System tray: the primary control surface. Starting a recording must never
//! be more than one click away, whatever window happens to be focused.
//!
//! The tray does not hold its own idea of whether a recording is running —
//! it reflects [`crate::session::RecordingState`] through [`reflect_state`],
//! so the tray and the window can never disagree.

use crate::commands;
use crate::session::RecordingState;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    App, AppHandle, Manager, Runtime,
};

/// What the tray is showing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
}

impl TrayState {
    /// The tray view of an app state.
    pub fn of(state: &RecordingState) -> Self {
        if state.is_capturing() {
            TrayState::Recording
        } else {
            TrayState::Idle
        }
    }
}

/// Tooltip text for the tray icon.
///
/// Pure so it can be asserted without a running app; the wording is part of
/// the product's honesty promise (the user must always be able to tell
/// whether a microphone is live).
pub fn tooltip(state: TrayState) -> &'static str {
    match state {
        TrayState::Idle => "Resumeira — not recording",
        TrayState::Recording => "Resumeira — recording",
    }
}

/// Whether the "Start Recording" entry is clickable in the given state.
pub fn start_enabled(state: TrayState) -> bool {
    matches!(state, TrayState::Idle)
}

/// Whether the "Stop Recording" entry is clickable in the given state.
pub fn stop_enabled(state: TrayState) -> bool {
    matches!(state, TrayState::Recording)
}

/// Handles kept so the tray can be updated as the recording state changes.
struct TrayHandles<R: Runtime> {
    start: MenuItem<R>,
    stop: MenuItem<R>,
    icon: TrayIcon<R>,
}

/// Build the tray icon and its menu, and register it for later updates.
pub fn setup<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    let state = TrayState::Idle;

    let start = MenuItem::with_id(
        app,
        "start",
        "Start Recording",
        start_enabled(state),
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(
        app,
        "stop",
        "Stop Recording",
        stop_enabled(state),
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let open = MenuItem::with_id(app, "open", "Open Resumeira", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&start, &stop, &separator, &open, &quit])?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip(tooltip(state))
        .on_menu_event(on_menu_event);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let icon = builder.build(app)?;
    app.manage(TrayHandles { start, stop, icon });
    Ok(())
}

/// Update the tray to match the app state.
///
/// Failures here are cosmetic and must never interrupt a recording, so they
/// are logged rather than propagated.
pub fn reflect_state<R: Runtime>(app: &AppHandle<R>, state: &RecordingState) {
    let Some(handles) = app.try_state::<TrayHandles<R>>() else {
        return;
    };

    if let Err(error) = handles.start.set_enabled(state.can_start()) {
        log::warn!("tray: could not update the start item: {error}");
    }
    if let Err(error) = handles.stop.set_enabled(state.can_stop()) {
        log::warn!("tray: could not update the stop item: {error}");
    }
    if let Err(error) = handles
        .icon
        .set_tooltip(Some(tooltip(TrayState::of(state))))
    {
        log::warn!("tray: could not update the tooltip: {error}");
    }
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        "start" => {
            commands::start(app);
        }
        "stop" => {
            commands::stop(app);
        }
        "open" => show_main_window(app),
        "quit" => app.exit(0),
        other => log::warn!("tray: unknown menu id {other}"),
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        log::warn!("tray: main window is gone");
        return;
    };
    if let Err(error) = window.show().and_then(|()| window.set_focus()) {
        log::warn!("tray: could not focus the main window: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Track;
    use crate::session::TrackStatus;

    #[test]
    fn tooltip_states_whether_a_microphone_is_live() {
        assert_eq!(tooltip(TrayState::Idle), "Resumeira — not recording");
        assert_eq!(tooltip(TrayState::Recording), "Resumeira — recording");
    }

    #[test]
    fn exactly_one_of_start_and_stop_is_enabled() {
        for state in [TrayState::Idle, TrayState::Recording] {
            assert_ne!(
                start_enabled(state),
                stop_enabled(state),
                "start and stop must never be enabled together in {state:?}"
            );
        }
    }

    #[test]
    fn start_is_the_idle_action_and_stop_the_recording_action() {
        assert!(start_enabled(TrayState::Idle));
        assert!(stop_enabled(TrayState::Recording));
    }

    #[test]
    fn the_tray_shows_recording_exactly_when_audio_is_being_captured() {
        let recording = RecordingState::Recording {
            started_at: 0,
            tracks: vec![TrackStatus {
                track: Track::Mic,
                device_name: "Mic".to_owned(),
                live: true,
                error: None,
            }],
        };
        assert_eq!(TrayState::of(&recording), TrayState::Recording);
        assert_eq!(
            TrayState::of(&RecordingState::Starting),
            TrayState::Recording
        );

        for state in [
            RecordingState::Idle,
            RecordingState::Stopping,
            RecordingState::Failed {
                error: "boom".to_owned(),
            },
        ] {
            assert_eq!(
                TrayState::of(&state),
                TrayState::Idle,
                "{state:?} must not claim a live microphone"
            );
        }
    }
}
