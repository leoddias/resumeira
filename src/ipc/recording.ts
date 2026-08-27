/**
 * Typed wrappers around the recording commands.
 *
 * Everything the frontend knows about recording goes through here, so there
 * is exactly one place to look when the Rust signatures change.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { RECORDING_STATE_EVENT, type RecordingState, type TrackLevel } from './types';

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

/**
 * How loud each track is right now. Empty unless a recording is running.
 *
 * Its own command rather than part of the state: this is polled at meter
 * speed, and a full state event at that rate would drive the tray too.
 */
export function getRecordingLevels(): Promise<TrackLevel[]> {
  return invoke<TrackLevel[]>('recording_levels');
}

/** Subscribe to state changes pushed by Rust. Returns an unsubscribe fn. */
export function onRecordingState(handler: (state: RecordingState) => void): Promise<UnlistenFn> {
  return listen<RecordingState>(RECORDING_STATE_EVENT, (event) => {
    handler(event.payload);
  });
}
