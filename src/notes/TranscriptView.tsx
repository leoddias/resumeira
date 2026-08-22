import { formatElapsed } from '../views/format';
import type { TranscriptLine } from '../ipc/meetings';

/**
 * The transcript, one line per row.
 *
 * Attribution — who said it, or failing that whether it came from the user's
 * own microphone or from the call — is a headline feature: it cost a separate
 * transcription pass and, when speakers are identified, an extra model call
 * (ADR-0004, ADR-0021). So it stays visible here, not just carried in the
 * data. A line nobody could attribute still renders, just without a name.
 */
export default function TranscriptView({ lines }: { lines: TranscriptLine[] }) {
  if (lines.length === 0) {
    return <p className="transcript__empty">No transcript for this meeting.</p>;
  }

  return (
    <ol className="transcript">
      {lines.map((line, index) => (
        <li key={index} className={lineClassName(line)}>
          <span className="transcript__time">{formatElapsed(line.start * 1000)}</span>
          <span className="transcript__track">{speakerLabel(line)}</span>
          <span className="transcript__text">{line.text}</span>
        </li>
      ))}
    </ol>
  );
}

/** A name if the meeting supplied one, else the side it came from. */
function speakerLabel(line: TranscriptLine): string {
  if (line.speaker !== undefined && line.speaker !== '') {
    return line.speaker;
  }
  switch (line.track) {
    case 'mic':
      return 'You';
    case 'system':
      return 'Others';
    default:
      return 'Unknown speaker';
  }
}

/**
 * Named lines are grouped by speaker so one person's turns read as one
 * thread; unnamed ones keep the mic/system split they have always had.
 */
function lineClassName(line: TranscriptLine): string {
  const base = 'transcript__line';
  if (line.speaker !== undefined && line.speaker !== '') {
    return `${base} ${base}--named ${base}--speaker-${speakerSlot(line.speaker)}`;
  }
  switch (line.track) {
    case 'mic':
      return `${base} ${base}--mic`;
    case 'system':
      return `${base} ${base}--system`;
    default:
      return base;
  }
}

/** How many colour slots the stylesheet defines for named speakers. */
const SPEAKER_SLOTS = 6;

/**
 * A stable slot per name, so the same person keeps the same colour every time
 * the note is opened. Names are not known ahead of time, so this hashes
 * rather than looking anything up.
 */
function speakerSlot(speaker: string): number {
  let hash = 0;
  for (const character of speaker) {
    hash = (hash * 31 + (character.codePointAt(0) ?? 0)) % 1_000_003;
  }
  return hash % SPEAKER_SLOTS;
}
