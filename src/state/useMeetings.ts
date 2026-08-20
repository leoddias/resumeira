import { useCallback, useEffect, useRef, useState } from 'react';
import { listMeetings, searchMeetings, type MeetingListItem } from '../ipc/meetings';

/** How long to wait after the last keystroke before querying. */
const SEARCH_DEBOUNCE_MS = 300;

export type MeetingsState =
  | { status: 'loading' }
  | { status: 'loaded'; items: MeetingListItem[] }
  | { status: 'error'; error: string };

/**
 * The meetings list, kept in sync with the notes folder on disk.
 *
 * A blank query shows every meeting; a non-blank one is debounced and sent
 * to `searchMeetings` so typing does not fire a query per keystroke. Loading
 * and failure are distinct states from `state` — a failed query must never
 * collapse into an empty list, which would look like "you have no meetings".
 *
 * `reloadKey` re-runs the current query whenever it changes. A meeting
 * becomes a note minutes after the recording stops, long after this list
 * last loaded, and a user watching the screen the whole time would otherwise
 * see nothing appear — the note is on disk and indexed, but the list has no
 * reason to ask again.
 */
export function useMeetings(reloadKey = 0) {
  const [query, setQuery] = useState('');
  const [state, setState] = useState<MeetingsState>({ status: 'loading' });
  // Guards against a slow, stale request overwriting a newer one's result.
  const requestId = useRef(0);

  const run = useCallback((q: string) => {
    const id = ++requestId.current;
    setState({ status: 'loading' });
    const request = q.trim() === '' ? listMeetings() : searchMeetings(q);
    request
      .then((items) => {
        if (id === requestId.current) setState({ status: 'loaded', items });
      })
      .catch((error: unknown) => {
        if (id === requestId.current) setState({ status: 'error', error: describe(error) });
      });
  }, []);

  // Initial load, and the full list whenever the query is cleared.
  useEffect(() => {
    if (query.trim() === '') run(query);
  }, [query, run, reloadKey]);

  // Debounced search while the query is non-blank.
  useEffect(() => {
    if (query.trim() === '') return;
    const timer = setTimeout(() => run(query), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [query, run, reloadKey]);

  const retry = useCallback(() => run(query), [query, run]);

  return { state, query, setQuery, retry };
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
