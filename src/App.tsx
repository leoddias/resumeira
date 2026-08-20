import { useRecording } from './state/useRecording';
import RecordingBar from './views/RecordingBar';

export default function App() {
  const { state, start, stop } = useRecording();

  return (
    <main className="app">
      <header className="app__header">
        <h1>Resumeira</h1>
        <p className="tagline">
          Local-first meeting notes. Nothing leaves this machine unless you ask it to.
        </p>
      </header>

      <RecordingBar state={state} onStart={start} onStop={stop} />

      <p className="status">No meetings yet. Start a recording here or from the tray icon.</p>
    </main>
  );
}
