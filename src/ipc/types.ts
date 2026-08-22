/**
 * The IPC contract with the Rust core.
 *
 * Deliberately narrow: the frontend asks for actions and receives state. It
 * never receives API keys or file contents it did not ask for (ADR-0009).
 */

/** Which side of the meeting a track captured. */
export type Track = 'mic' | 'system';

/** A track that is currently being captured. */
export interface TrackStatus {
  track: Track;
  /** Device the samples are coming from, for the UI and for error messages. */
  deviceName: string;
  /**
   * Whether this track is still recording. A track can fail on its own
   * without ending the meeting — the other one keeps going.
   */
  live: boolean;
  /** Why the track stopped, when it stopped for a reason. */
  error?: string;
}

/** What the app is doing right now. */
export type RecordingState =
  | { status: 'idle' }
  | { status: 'starting' }
  | {
      status: 'recording';
      /** Unix ms when capture began, for the elapsed-time display. */
      startedAt: number;
      tracks: TrackStatus[];
    }
  | { status: 'stopping' }
  | {
      status: 'processing';
      /** Coarse stage so the UI can say something honest while waiting. */
      stage: 'transcribing' | 'identifying' | 'summarizing' | 'saving';
    }
  | { status: 'failed'; error: string };

/** Event name the Rust core emits recording-state changes on. */
export const RECORDING_STATE_EVENT = 'recording-state';

/** The state the app starts in, before Rust has said anything. */
export const INITIAL_RECORDING_STATE: RecordingState = { status: 'idle' };

/**
 * Whether a microphone is live right now.
 *
 * The UI must never claim "not recording" while audio is being captured, so
 * this is a single tested function rather than a check spread across views.
 */
export function isCapturing(state: RecordingState): boolean {
  return state.status === 'recording' || state.status === 'starting';
}

/** Whether the user may start a new recording. */
export function canStart(state: RecordingState): boolean {
  return state.status === 'idle' || state.status === 'failed';
}

/** Whether the user may stop the current recording. */
export function canStop(state: RecordingState): boolean {
  return state.status === 'recording';
}
