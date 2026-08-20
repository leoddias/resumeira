import { useFirstRun } from '../state/useFirstRun';

/** Where meetings are written when the user has not chosen a folder. */
const DEFAULT_NOTES_FOLDER_LABEL = 'the default Documents location';

/**
 * The first-run onboarding screen.
 *
 * `useFirstRun` decides whether this renders at all — a returning user, or
 * anyone who already has a key or a local model installed, never sees it.
 * Everything here is informational; nothing on this screen writes to
 * settings. Configuration happens on the Settings screen afterwards.
 */
export default function FirstRun() {
  const { state, skip } = useFirstRun();

  if (state.status !== 'visible') return null;

  return (
    <div className="first-run" role="dialog" aria-label="Welcome to Resumeira">
      <h2>Welcome to Resumeira</h2>

      <ol className="first-run__steps">
        <li>
          Your notes are saved as plain Markdown in{' '}
          <strong>{state.notesFolder ?? DEFAULT_NOTES_FOLDER_LABEL}</strong>. You can change this in
          Settings.
        </li>
        <li>
          Recording starts from the tray icon, not from this window — you don't need to keep
          Resumeira open.
        </li>
        <li>
          Before you can transcribe a meeting, choose an engine in Settings: download a local
          Whisper model, or paste an API key for a cloud provider.
        </li>
      </ol>

      <p className="first-run__cloud-notice" role="alert">
        Summaries always leave your machine. Resumeira has no local summarizer, so turning a
        transcript into notes sends that transcript to a cloud LLM using your own API key — even
        when transcription itself runs fully locally.
      </p>

      <p className="first-run__mic-notice">
        When you start your first recording, Windows will ask for permission to use the microphone.
        That prompt is expected — allow it, or recording cannot start.
      </p>

      <button type="button" className="first-run__skip" onClick={skip}>
        Skip for now
      </button>
    </div>
  );
}
