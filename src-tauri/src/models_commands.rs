//! Tauri commands for the local Whisper models.
//!
//! The local engine is the product's privacy promise, and it does not work
//! without a model on disk. These commands are what makes that reachable
//! from the UI instead of being a feature only the author can enable.

use crate::transcribe::model::{self, DownloadProgress};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Runtime};

/// Event the frontend listens on for download progress.
pub const MODEL_PROGRESS_EVENT: &str = "model-progress";

/// A model as the Settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub installed: bool,
}

/// Download progress, pushed as an event while a model downloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgress {
    pub id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// Whole percent, so the UI does not have to guess at rounding.
    pub percent: u8,
}

impl ModelProgress {
    fn new(id: &str, progress: DownloadProgress) -> Self {
        Self {
            id: id.to_owned(),
            downloaded_bytes: progress.downloaded_bytes,
            total_bytes: progress.total_bytes,
            percent: percent_of(progress.downloaded_bytes, progress.total_bytes),
        }
    }
}

/// Whole-percent progress that never divides by zero and never exceeds 100.
///
/// A server that misreports `Content-Length` must not produce "137%" or a
/// panic; a progress bar is not worth either.
fn percent_of(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let ratio = (done.min(total) as f64 / total as f64) * 100.0;
    ratio.round().clamp(0.0, 100.0) as u8
}

fn models_root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::models_root(app)
}

#[tauri::command]
pub fn list_models<R: Runtime>(app: AppHandle<R>) -> Vec<ModelEntry> {
    let root = models_root(&app);
    model::CATALOG
        .iter()
        .map(|info| ModelEntry {
            id: info.id.to_owned(),
            display_name: info.display_name.to_owned(),
            size_bytes: info.size_bytes,
            installed: model::is_installed(&root, info.id),
        })
        .collect()
}

/// Downloads a model, reporting progress as events.
///
/// Long-running on purpose: `large-v3-turbo` is about 1.6 GB, so the UI
/// follows the event stream rather than waiting on the return value.
#[tauri::command]
pub async fn download_model<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    let root = models_root(&app);
    let progress_app = app.clone();
    let progress_id = id.clone();

    model::download(&root, &id, move |progress| {
        let payload = ModelProgress::new(&progress_id, progress);
        if let Err(error) = progress_app.emit(MODEL_PROGRESS_EVENT, payload) {
            log::warn!("could not publish model progress: {error}");
        }
    })
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_model<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    model::delete(&models_root(&app), &id).map_err(|error| error.to_string())
}

/// Opens the folder models are stored in, for anyone who would rather place
/// a file there by hand than download 1.6 GB through the app.
#[tauri::command]
pub fn open_models_folder<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let root = models_root(&app);
    std::fs::create_dir_all(&root).map_err(|error| error.kind().to_string())?;
    app.opener()
        .open_path(root.display().to_string(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_whole_and_bounded() {
        assert_eq!(percent_of(0, 100), 0);
        assert_eq!(percent_of(50, 100), 50);
        assert_eq!(percent_of(100, 100), 100);
    }

    #[test]
    fn an_unknown_total_reports_zero_rather_than_dividing_by_it() {
        assert_eq!(percent_of(1_000, 0), 0);
    }

    #[test]
    fn a_misreported_length_never_exceeds_one_hundred() {
        // Some servers understate Content-Length; the bar must not read 137%.
        assert_eq!(percent_of(137, 100), 100);
    }

    #[test]
    fn the_catalog_offers_a_default_and_smaller_options() {
        let ids: Vec<&str> = model::CATALOG.iter().map(|info| info.id).collect();
        assert!(
            ids.contains(&"large-v3-turbo"),
            "the documented default must be installable, got {ids:?}"
        );
        assert!(
            ids.len() >= 3,
            "weak machines need smaller options, got {ids:?}"
        );
    }

    #[test]
    fn every_catalog_entry_has_a_checksum_and_a_size() {
        for info in model::CATALOG {
            assert_eq!(
                info.sha256.len(),
                64,
                "{} needs a full sha-256, got '{}'",
                info.id,
                info.sha256
            );
            assert!(info.size_bytes > 0, "{} needs a size", info.id);
        }
    }

    #[test]
    fn the_progress_event_name_matches_the_frontend_contract() {
        assert_eq!(MODEL_PROGRESS_EVENT, "model-progress");
    }
}
