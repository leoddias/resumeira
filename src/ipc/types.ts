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

/** The post-recording steps, in the order they run. */
export type ProcessingStage = 'transcribing' | 'identifying' | 'summarizing' | 'saving';

/**
 * How far transcription has got.
 *
 * Carries a line of the meeting, so it belongs on screen and nowhere else —
 * never logged, never stored. Rust clears it the moment the step ends.
 */
export interface TranscribeProgress {
  track: Track;
  /** 1-based position among the tracks this meeting recorded. */
  index: number;
  total: number;
  /** 0-100, or absent for an engine that cannot say how far it is. */
  percent?: number;
  /** The most recent line the engine produced. */
  line?: string;
}

/**
 * How loud one track is right now.
 *
 * Polled several times a second, separately from `RecordingState`: a meter
 * that only moves when the state changes is not a meter.
 */
export interface TrackLevel {
  track: Track;
  /** Bar height in 0..1, already scaled for display by Rust. */
  level: number;
  /**
   * Whether the device is delivering audio at all. Silence from a live
   * device is `level: 0, receiving: true`; a device that is not there is
   * `false`, and the two mean very different things to someone checking
   * their microphone.
   */
  receiving: boolean;
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
      stage: ProcessingStage;
      /** Unix ms when the pipeline started, for the elapsed-time display. */
      startedAt: number;
      /** Only ever present during the transcribing stage. */
      transcribing?: TranscribeProgress;
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
