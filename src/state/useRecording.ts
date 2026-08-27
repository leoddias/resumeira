import { useCallback, useEffect, useRef, useState } from 'react';
import {
  getRecordingState,
  onRecordingState,
  startRecording,
  stopRecording,
} from '../ipc/recording';
import { INITIAL_RECORDING_STATE, type RecordingState } from '../ipc/types';

/**
 * How many transcript lines the preview keeps on screen.
 *
 * Enough to look like speech arriving, few enough that the recording bar
 * stays a bar. The transcript itself is the note, not this.
 */
const PREVIEW_LINES = 3;

/** How often to re-ask Rust for per-track liveness while recording. */
const REFRESH_INTERVAL_MS = 2000;

/**
 * Live recording state, kept in sync with the Rust core.
 *
 * State is owned by Rust — the tray can start a recording without the window
 * being open — so this hook reflects it rather than holding its own copy.
 */
export function useRecording() {
  const [state, setState] = useState<RecordingState>(INITIAL_RECORDING_STATE);
  // Rust sends one line at a time and keeps none of them; the few lines the
  // preview shows are accumulated here and dropped when the step ends.
  const [preview, setPreview] = useState<string[]>([]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    getRecordingState()
      .then((current) => {
        if (active) setState(current);
      })
      .catch((error: unknown) => {
        if (active) setState({ status: 'failed', error: describe(error) });
      });

    onRecordingState((next) => {
      if (!active) return;
      setState(next);
      // Only from pushed events: a poll would re-deliver the line that is
      // already on screen and print it twice.
      setPreview((lines) => nextPreview(lines, next));
    })
      .then((fn) => {
        if (active) unlisten = fn;
        else fn();
      })
      .catch(() => {
        // Losing the subscription is not worth blanking the UI over; the
        // next explicit action still reports the real state.
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  // Per-track liveness changes without an event — a device can disappear at
  // any moment and Rust learns about it on the audio thread, not through a
  // user action. Re-asking while recording is what keeps the UI from showing
  // a dead microphone as live.
  const recording = state.status === 'recording';
  useEffect(() => {
    if (!recording) return;
    let active = true;
    const timer = setInterval(() => {
      getRecordingState()
        .then((current) => {
          if (active) setState(current);
        })
        .catch(() => {
          // A failed refresh is not worth destroying a good state over; the
          // next tick tries again.
        });
    }, REFRESH_INTERVAL_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [recording]);

  const start = useCallback(async () => {
    setState({ status: 'starting' });
    try {
      setState(await startRecording());
    } catch (error: unknown) {
      setState({ status: 'failed', error: describe(error) });
    }
  }, []);

  const stop = useCallback(async () => {
    setState({ status: 'stopping' });
    try {
      setState(await stopRecording());
    } catch (error: unknown) {
      setState({ status: 'failed', error: describe(error) });
    }
  }, []);

  return { state, start, stop, preview };
}

/**
 * The preview lines after `state` arrives.
 *
 * Pure, and exported, because the rule it encodes is worth pinning: the
 * preview belongs to one transcription step and does not outlive it.
 */
export function nextPreview(lines: string[], state: RecordingState): string[] {
  if (state.status !== 'processing' || state.stage !== 'transcribing') {
    return lines.length === 0 ? lines : [];
  }
  const line = state.transcribing?.line;
  if (line === undefined) {
    return lines;
  }
  return [...lines, line].slice(-PREVIEW_LINES);
}

/**
 * Counts pipelines that have finished, so the rest of the UI can notice.
 *
 * Turning a meeting into a note takes minutes and happens in the background,
 * so nothing the user did marks the moment it lands. Leaving `processing` is
 * that moment: the note has just been written and indexed, and any list of
 * meetings on screen is now out of date.
 *
 * Counts failures too. A failed pipeline leaves no new note, and asking for
 * the list again costs one query — while missing a real one leaves the user
 * staring at a screen that says they have no meetings.
 */
export function useCompletedPipelines(state: RecordingState): number {
  const [completed, setCompleted] = useState(0);
  const wasProcessing = useRef(false);

  useEffect(() => {
    const processing = state.status === 'processing';
    if (wasProcessing.current && !processing) {
      setCompleted((count) => count + 1);
    }
    wasProcessing.current = processing;
  }, [state.status]);

  return completed;
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
