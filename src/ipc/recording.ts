/**
 * Typed wrappers around the recording commands.
 *
 * Everything the frontend knows about recording goes through here, so there
 * is exactly one place to look when the Rust signatures change.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { RECORDING_STATE_EVENT, type RecordingState } from './types';

/** Begin capturing. Rust answers with the state it moved to. */
export function startRecording(): Promise<RecordingState> {
  return invoke<RecordingState>('start_recording');
}

/** End the current capture and hand the audio to the pipeline. */
export function stopRecording(): Promise<RecordingState> {
  return invoke<RecordingState>('stop_recording');
}

/** Ask Rust what is happening right now (used once on mount). */
export function getRecordingState(): Promise<RecordingState> {
  return invoke<RecordingState>('recording_state');
}

/** Subscribe to state changes pushed by Rust. Returns an unsubscribe fn. */
export function onRecordingState(handler: (state: RecordingState) => void): Promise<UnlistenFn> {
  return listen<RecordingState>(RECORDING_STATE_EVENT, (event) => {
    handler(event.payload);
  });
}
