//! System tray: the primary control surface. Starting a recording must never
//! be more than one click away, whatever window happens to be focused.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Manager, Runtime,
};

/// What the tray is showing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
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

/// Build the tray icon and its menu. M0 wires the menu only — the recording
/// items log their intent until `recorder` lands in M1.
pub fn setup(app: &App) -> tauri::Result<()> {
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

    builder.build(app)?;
    Ok(())
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        // M1 replaces these with real session control.
        "start" => log::info!("tray: start recording requested"),
        "stop" => log::info!("tray: stop recording requested"),
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
}
