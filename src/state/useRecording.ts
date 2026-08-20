import { useCallback, useEffect, useState } from 'react';
import {
  getRecordingState,
  onRecordingState,
  startRecording,
  stopRecording,
} from '../ipc/recording';
import { INITIAL_RECORDING_STATE, type RecordingState } from '../ipc/types';

/**
 * Live recording state, kept in sync with the Rust core.
 *
 * State is owned by Rust — the tray can start a recording without the window
 * being open — so this hook reflects it rather than holding its own copy.
 */
/** How often to re-ask Rust for per-track liveness while recording. */
const REFRESH_INTERVAL_MS = 2000;

export function useRecording() {
  const [state, setState] = useState<RecordingState>(INITIAL_RECORDING_STATE);

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
      if (active) setState(next);
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

  return { state, start, stop };
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
