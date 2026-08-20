import { formatElapsed } from '../views/format';
import type { Track } from '../ipc/types';
import type { TranscriptLine } from '../ipc/meetings';

/**
 * The transcript, one line per row.
 *
 * Track attribution — the user's own microphone versus everyone else — is a
 * headline feature: it cost a separate transcription pass to obtain, so it
 * has to stay visible here, not just carried in the data. A line the engine
 * could not attribute still renders, just without a track badge.
 */
export default function TranscriptView({ lines }: { lines: TranscriptLine[] }) {
  if (lines.length === 0) {
    return <p className="transcript__empty">No transcript for this meeting.</p>;
  }

  return (
    <ol className="transcript">
      {lines.map((line, index) => (
        <li key={index} className={lineClassName(line.track)}>
          <span className="transcript__time">{formatElapsed(line.start * 1000)}</span>
          <span className="transcript__track">{trackLabel(line.track)}</span>
          <span className="transcript__text">{line.text}</span>
        </li>
      ))}
    </ol>
  );
}

function trackLabel(track: Track | undefined): string {
  switch (track) {
    case 'mic':
      return 'You';
    case 'system':
      return 'Others';
    default:
      return 'Unknown speaker';
  }
}

function lineClassName(track: Track | undefined): string {
  const base = 'transcript__line';
  switch (track) {
    case 'mic':
      return `${base} transcript__line--mic`;
    case 'system':
      return `${base} transcript__line--system`;
    default:
      return base;
  }
}
