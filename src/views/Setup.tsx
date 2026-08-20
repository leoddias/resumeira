import { useState, type ReactNode } from 'react';
import type { Blocker, Readiness, StepReadiness } from '../ipc/readiness';
import { AGENT_CLIS, type Settings } from '../ipc/settings';
import { useSettings } from '../state/useSettings';
import ModelManager from './ModelManager';

interface Props {
  readiness: Readiness;
  /** Re-runs the readiness check after something here changed the answer. */
  onChanged: () => void;
}

/**
 * The screen that stands between a fresh install and a lost meeting
 * (ADR-0019).
 *
 * It is shown whenever the configured pipeline cannot finish, and it says
 * which half is missing and offers the fix inline — download the model, save
 * a key, or point the summary at an agent CLI the machine already has. The
 * alternative, and what this replaces, was recording a whole meeting and
 * only then discovering that the default local model was never downloaded.
 */
export default function Setup({ readiness, onChanged }: Props) {
  const { load, save, saveKey } = useSettings();

  if (load.status !== 'ready') {
    return <p className="status">Checking your setup…</p>;
  }

  const settings = load.settings;

  async function apply(next: Settings) {
    await save(next);
    onChanged();
  }

  async function storeKey(account: string, key: string) {
    await saveKey(account, key);
    onChanged();
  }

  return (
    <section className="setup" role="dialog" aria-label="Finish setting up Resumeira">
      <h2>Finish setting up Resumeira</h2>
      <p className="setup__why">
        Recording is disabled until both steps below can run. A meeting cannot be recorded a second
        time, so this is checked before you hit record rather than after.
      </p>

      <Step
        title="Transcription"
        step={readiness.transcription}
        fix={
          <TranscriptionFix
            blocker={readiness.transcription.blocker}
            settings={settings}
            onApply={apply}
            onSaveKey={storeKey}
            onChanged={onChanged}
          />
        }
      />

      <Step
        title="Notes"
        step={readiness.summary}
        fix={
          <SummaryFix
            blocker={readiness.summary.blocker}
            settings={settings}
            onApply={apply}
            onSaveKey={storeKey}
          />
        }
      />

      <p className="setup__cloud-notice" role="alert">
        Resumeira has no local note writer. Turning a transcript into notes always sends that
        transcript to a model — through your API key, or through an agent CLI signed in on this
        machine — even when transcription itself runs entirely offline. An agent CLI also keeps its
        own copy of what it was sent, in its own history, which Resumeira cannot delete for you.
      </p>

      <ul className="setup__facts">
        <li>
          Notes are saved as plain Markdown you own, in {settings.notesFolder ?? 'Documents'}.
        </li>
        <li>Recording can be started from the tray, so the window does not have to stay open.</li>
        <li>
          Windows will ask for microphone permission the first time — allow it, or nothing records.
        </li>
      </ul>
    </section>
  );
}

function Step({ title, step, fix }: { title: string; step: StepReadiness; fix: ReactNode }) {
  return (
    <div className={step.ready ? 'setup__step setup__step--ready' : 'setup__step'}>
      <h3>
        {title}: {step.ready ? 'ready' : 'not set up'}
      </h3>
      <p className="setup__detail">
        {step.detail}
        {step.leavesTheMachine && <span className="setup__leaves"> · leaves this machine</span>}
      </p>
      {!step.ready && fix}
    </div>
  );
}

interface FixProps {
  blocker: Blocker | undefined;
  settings: Settings;
  onApply: (settings: Settings) => Promise<void>;
  onSaveKey: (account: string, key: string) => Promise<void>;
}

function TranscriptionFix({
  blocker,
  settings,
  onApply,
  onSaveKey,
  onChanged,
}: FixProps & { onChanged: () => void }) {
  if (blocker === undefined) return null;

  if (blocker.kind === 'modelMissing') {
    return (
      <div className="setup__fix">
        <p>
          You chose to transcribe on this device, which needs the {blocker.model} model downloaded
          first. It is a large file, so it is never downloaded behind your back.
        </p>
        <ModelManager onChanged={onChanged} />
        <button
          type="button"
          onClick={() =>
            void onApply({
              ...settings,
              transcription: { ...settings.transcription, engine: 'api' },
            })
          }
        >
          Or transcribe through a cloud API instead
        </button>
      </div>
    );
  }

  if (blocker.kind === 'keyMissing') {
    return (
      <div className="setup__fix">
        <p>Cloud transcription needs a {blocker.account} API key.</p>
        <KeyForm account={blocker.account} onSaveKey={onSaveKey} />
        <button
          type="button"
          onClick={() =>
            void onApply({
              ...settings,
              transcription: { ...settings.transcription, engine: 'local' },
            })
          }
        >
          Or transcribe on this device instead
        </button>
      </div>
    );
  }

  return null;
}

function SummaryFix({ blocker, settings, onApply, onSaveKey }: FixProps) {
  if (blocker === undefined) return null;

  const cliChoices = (
    <div className="setup__clis">
      <p>
        No API key? If you already have a coding agent signed in on this machine, Resumeira can ask
        it to write the notes under your own subscription.
      </p>
      {AGENT_CLIS.map(({ id, label }) => (
        <button
          key={id}
          type="button"
          onClick={() => void onApply({ ...settings, summaryEngine: 'cli', summaryCli: id })}
        >
          Use {label}
        </button>
      ))}
    </div>
  );

  if (blocker.kind === 'keyMissing') {
    return (
      <div className="setup__fix">
        <p>Writing the notes needs a {blocker.account} API key.</p>
        <KeyForm account={blocker.account} onSaveKey={onSaveKey} />
        {cliChoices}
      </div>
    );
  }

  if (blocker.kind === 'cliMissing') {
    return (
      <div className="setup__fix">
        <p>
          {blocker.name} is not installed, or not on this machine's PATH. Install it and check
          again, or pick another way to write the notes. Resumeira only checks that the command
          exists — if it is installed but signed out, the first meeting will say so.
        </p>
        {AGENT_CLIS.filter(({ id }) => id !== blocker.cli).map(({ id, label }) => (
          <button
            key={id}
            type="button"
            onClick={() => void onApply({ ...settings, summaryEngine: 'cli', summaryCli: id })}
          >
            Use {label} instead
          </button>
        ))}
        <button type="button" onClick={() => void onApply({ ...settings, summaryEngine: 'api' })}>
          Or use an API key instead
        </button>
      </div>
    );
  }

  return null;
}

/**
 * Write-only key entry, same contract as Settings: the value travels inward
 * once and is cleared from the field afterwards, so nothing that looks like a
 * key is left on screen (ADR-0009).
 */
function KeyForm({
  account,
  onSaveKey,
}: {
  account: string;
  onSaveKey: (account: string, key: string) => Promise<void>;
}) {
  const [value, setValue] = useState('');
  const [saving, setSaving] = useState(false);

  async function handleSave() {
    if (value === '') return;
    setSaving(true);
    try {
      await onSaveKey(account, value);
      setValue('');
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="setup__key">
      <input
        type="password"
        autoComplete="off"
        aria-label={`${account} API key`}
        placeholder={`${account} API key`}
        value={value}
        onChange={(event) => setValue(event.target.value)}
      />
      <button type="button" disabled={saving || value === ''} onClick={() => void handleSave()}>
        {saving ? 'Saving…' : `Save ${account} key`}
      </button>
    </div>
  );
}
