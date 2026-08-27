import { useState } from 'react';
import { useReadiness, isBlocked } from './state/useReadiness';
import { useAudioLevels } from './state/useAudioLevels';
import { useCompletedPipelines, useRecording } from './state/useRecording';
import Meetings from './views/Meetings';
import Note from './views/Note';
import RecordingBar from './views/RecordingBar';
import Settings from './views/Settings';
import Setup from './views/Setup';

type View = { name: 'meetings' } | { name: 'note'; folder: string } | { name: 'settings' };

export default function App() {
  const { state, start, stop, preview } = useRecording();
  const { state: readiness, recheck } = useReadiness();
  const [view, setView] = useState<View>({ name: 'meetings' });
  // A note lands minutes after the recording stops, with the user watching a
  // list that has no reason to ask again.
  const completed = useCompletedPipelines(state);
  // Proof that audio is actually arriving, while the meeting can still be
  // saved by unmuting something.
  const levels = useAudioLevels(state);

  // Recording is refused in Rust when the pipeline cannot finish (ADR-0019);
  // this is the same verdict, shown before the click rather than after.
  const blocked = isBlocked(readiness);
  const showSetup = blocked && view.name !== 'settings';

  return (
    <main className="app">
      <header className="app__header">
        <h1>Resumeira</h1>
        <nav className="app__nav">
          <button
            type="button"
            onClick={() => setView({ name: 'meetings' })}
            aria-current={view.name !== 'settings' ? 'page' : undefined}
          >
            Meetings
          </button>
          <button
            type="button"
            onClick={() => setView({ name: 'settings' })}
            aria-current={view.name === 'settings' ? 'page' : undefined}
          >
            Settings
          </button>
        </nav>
      </header>

      <RecordingBar
        state={state}
        onStart={start}
        onStop={stop}
        blockedReason={blocked ? 'Finish setup before recording' : undefined}
        levels={levels}
        preview={preview}
      />

      {showSetup && readiness.status === 'ready' && (
        <Setup readiness={readiness.readiness} onChanged={() => void recheck()} />
      )}

      {!showSetup && view.name === 'meetings' && (
        <Meetings onOpen={(folder) => setView({ name: 'note', folder })} reloadKey={completed} />
      )}

      {!showSetup && view.name === 'note' && (
        <>
          <button type="button" className="app__back" onClick={() => setView({ name: 'meetings' })}>
            ← All meetings
          </button>
          <Note folder={view.folder} />
        </>
      )}

      {view.name === 'settings' && <Settings onChanged={() => void recheck()} />}
    </main>
  );
}
