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
