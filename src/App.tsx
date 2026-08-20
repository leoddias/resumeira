import { useState } from 'react';
import { useRecording } from './state/useRecording';
import FirstRun from './views/FirstRun';
import Meetings from './views/Meetings';
import Note from './views/Note';
import RecordingBar from './views/RecordingBar';
import Settings from './views/Settings';

type View = { name: 'meetings' } | { name: 'note'; folder: string } | { name: 'settings' };

export default function App() {
  const { state, start, stop } = useRecording();
  const [view, setView] = useState<View>({ name: 'meetings' });

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

      <FirstRun />

      <RecordingBar state={state} onStart={start} onStop={stop} />

      {view.name === 'meetings' && (
        <Meetings onOpen={(folder) => setView({ name: 'note', folder })} />
      )}

      {view.name === 'note' && (
        <>
          <button type="button" className="app__back" onClick={() => setView({ name: 'meetings' })}>
            ← All meetings
          </button>
          <Note folder={view.folder} />
        </>
      )}

      {view.name === 'settings' && <Settings />}
    </main>
  );
}
