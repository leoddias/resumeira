import { useCallback, useEffect, useState } from 'react';
import { readMeeting, type MeetingNote } from '../ipc/meetings';

export type NoteState =
  | { status: 'loading' }
  | { status: 'error'; error: string }
  | { status: 'ready'; note: MeetingNote };

/**
 * Loads one meeting's note by folder and keeps a retryable state around it.
 *
 * Rust owns the note; this hook only asks for it, so failures (a moved
 * folder, a corrupt file) surface as an honest error state instead of a
 * blank note.
 */
export function useNote(folder: string) {
  const [state, setState] = useState<NoteState>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    setState({ status: 'loading' });

    readMeeting(folder)
      .then((note) => {
        if (active) setState({ status: 'ready', note });
      })
      .catch((error: unknown) => {
        if (active) setState({ status: 'error', error: describe(error) });
      });

    return () => {
      active = false;
    };
  }, [folder, attempt]);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);

  return { state, reload };
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
