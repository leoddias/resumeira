import { useCallback, useEffect, useState } from 'react';
import { checkReadiness, type Readiness } from '../ipc/readiness';

export type ReadinessState =
  | { status: 'checking' }
  | { status: 'ready'; readiness: Readiness }
  | { status: 'failed'; error: string };

/**
 * Whether the configured pipeline can run (ADR-0019).
 *
 * Checked on mount and re-checked through `recheck` after anything that
 * could change the answer — a saved key, a finished model download, a
 * settings change — so fixing the cause unblocks the app immediately
 * instead of at the next launch.
 */
export function useReadiness() {
  const [state, setState] = useState<ReadinessState>({ status: 'checking' });

  const recheck = useCallback(async () => {
    try {
      setState({ status: 'ready', readiness: await checkReadiness() });
    } catch (error: unknown) {
      setState({ status: 'failed', error: describe(error) });
    }
  }, []);

  useEffect(() => {
    void recheck();
  }, [recheck]);

  return { state, recheck };
}

/**
 * Whether to hold the app on the setup screen.
 *
 * Only a definite "not ready" blocks. While the check is in flight, or if it
 * failed outright, the app stays usable: Rust refuses the recording either
 * way, so a failed check costs a clear explanation, not a lost meeting —
 * whereas locking the whole window behind an IPC hiccup would cost both.
 */
export function isBlocked(state: ReadinessState): boolean {
  return state.status === 'ready' && !state.readiness.canRecord;
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
