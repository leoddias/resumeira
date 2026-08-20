import Markdown from '../notes/markdown';
import TranscriptView from '../notes/TranscriptView';
import { useNote } from '../notes/useNote';
import { openMeetingFolder, type MeetingNote } from '../ipc/meetings';

interface Props {
  folder: string;
}

/**
 * Reads one meeting: summary first, transcript below a divider.
 *
 * State comes from `useNote`, so this stays a thin, testable view — loading,
 * error and ready are the only shapes it has to render.
 */
export default function Note({ folder }: Props) {
  const { state, reload } = useNote(folder);

  if (state.status === 'loading') {
    return (
      <p className="note__status" role="status">
        Loading…
      </p>
    );
  }

  if (state.status === 'error') {
    return (
      <div className="note__status note__status--error" role="alert">
        <p>Could not load this meeting: {state.error}</p>
        <button type="button" onClick={reload}>
          Retry
        </button>
      </div>
    );
  }

  return <NoteContent note={state.note} />;
}

function NoteContent({ note }: { note: MeetingNote }) {
  const handleOpenFolder = () => {
    openMeetingFolder(note.folder).catch(() => {
      // Best effort — the user can still find the folder manually.
    });
  };

  return (
    <article className="note">
      <header className="note__header">
        <h1>{note.title}</h1>
        <p className="note__meta">
          {formatDate(note.createdAt)} · {describeSource(note)}
          {note.language !== undefined && ` · ${note.language}`}
        </p>
        <p className="note__audio">
          {note.audioKept ? 'Audio is still on disk.' : 'Audio has been deleted.'}
        </p>
        <button type="button" onClick={handleOpenFolder}>
          Open folder
        </button>
      </header>

      <section className="note__summary">
        <Markdown source={note.summary} />
      </section>

      <hr className="note__divider" />

      <section className="note__transcript">
        <h2>Transcript</h2>
        <TranscriptView lines={note.transcript} />
      </section>
    </article>
  );
}

function describeSource(note: MeetingNote): string {
  const engine = note.engine === 'local' ? 'Local transcription' : 'API transcription';
  return `${engine} · ${note.model}`;
}

function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}
