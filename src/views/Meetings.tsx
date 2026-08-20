import type { MeetingListItem } from '../ipc/meetings';
import { useMeetings } from '../state/useMeetings';

interface Props {
  /** Called with a meeting's folder — its stable id — when the user picks one. */
  onOpen: (folder: string) => void;
  /** Changes when a meeting has just become a note, so the list asks again. */
  reloadKey?: number;
}

/**
 * Every meeting, newest first, with search.
 *
 * Loading, empty and failed are rendered as three distinct messages on
 * purpose: this app's whole job is to not lose your meetings, so a failed
 * query must never look like "you have no meetings".
 */
export default function Meetings({ onOpen, reloadKey }: Props) {
  const { state, query, setQuery, retry } = useMeetings(reloadKey);

  return (
    <div className="meetings">
      <input
        type="search"
        className="meetings__search"
        placeholder="Search meetings…"
        aria-label="Search meetings"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <MeetingsBody state={state} query={query} onOpen={onOpen} onRetry={retry} />
    </div>
  );
}

function MeetingsBody({
  state,
  query,
  onOpen,
  onRetry,
}: {
  state: ReturnType<typeof useMeetings>['state'];
  query: string;
  onOpen: (folder: string) => void;
  onRetry: () => void;
}) {
  switch (state.status) {
    case 'loading':
      return (
        <p className="meetings__status" role="status">
          Loading meetings…
        </p>
      );
    case 'error':
      return (
        <div className="meetings__status meetings__status--error" role="alert">
          <p>Could not load meetings: {state.error}</p>
          <button type="button" onClick={onRetry}>
            Retry
          </button>
        </div>
      );
    case 'loaded':
      if (state.items.length === 0) {
        return (
          <p className="meetings__status">
            {query.trim() === '' ? 'No meetings yet.' : 'No meetings match your search.'}
          </p>
        );
      }
      return <MeetingsList items={state.items} onOpen={onOpen} />;
  }
}

function MeetingsList({
  items,
  onOpen,
}: {
  items: MeetingListItem[];
  onOpen: (folder: string) => void;
}) {
  return (
    <ul className="meetings__list">
      {items.map((meeting) => (
        <li key={meeting.folder}>
          <button type="button" className="meetings__item" onClick={() => onOpen(meeting.folder)}>
            <span className="meetings__title">{meeting.title}</span>
            <span className="meetings__date">{formatMeetingDate(meeting.createdAt)}</span>
            <span className="meetings__preview">{meeting.preview}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}

/**
 * `createdAt` as a short local date and time, e.g. `Mar 4, 2:30 PM`.
 *
 * Falls back to the raw string when the timestamp cannot be parsed, so a bad
 * value shows up as visibly odd text instead of "Invalid Date".
 */
function formatMeetingDate(createdAt: string): string {
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return createdAt;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}
