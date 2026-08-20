/**
 * Local Whisper models.
 *
 * The local engine is what makes "your meeting never leaves this machine"
 * true, and it does not work until one of these is on disk — so downloading
 * one has to be something a user can do, not a setup step only the author
 * knows about.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** A model the app can install. */
export interface ModelEntry {
  id: string;
  displayName: string;
  sizeBytes: number;
  installed: boolean;
}

/** Progress of a download in flight. */
export interface ModelProgress {
  id: string;
  downloadedBytes: number;
  totalBytes: number;
  /** Whole percent, already clamped to 0–100 by Rust. */
  percent: number;
}

/** Event name the Rust core emits download progress on. */
export const MODEL_PROGRESS_EVENT = 'model-progress';

export function listModels(): Promise<ModelEntry[]> {
  return invoke<ModelEntry[]>('list_models');
}

/** Downloads a model. Resolves when it is installed and verified. */
export function downloadModel(id: string): Promise<void> {
  return invoke<void>('download_model', { id });
}

export function deleteModel(id: string): Promise<void> {
  return invoke<void>('delete_model', { id });
}

/** Reveals the models folder, for placing a file there by hand. */
export function openModelsFolder(): Promise<void> {
  return invoke<void>('open_models_folder');
}

export function onModelProgress(handler: (progress: ModelProgress) => void): Promise<UnlistenFn> {
  return listen<ModelProgress>(MODEL_PROGRESS_EVENT, (event) => {
    handler(event.payload);
  });
}

/** A model's size as a human-readable string. */
export function formatSize(bytes: number): string {
  if (bytes <= 0) return '—';
  const gb = bytes / 1_000_000_000;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${Math.round(bytes / 1_000_000)} MB`;
}
