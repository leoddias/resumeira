import { useEffect, useState, type FormEvent } from 'react';
import {
  accountFor,
  AGENT_CLIS,
  requiredTranscriptionAccount,
  sendsAudioToTheCloud,
  type AgentCli,
  type AudioRetention,
  type KeyStatus,
  type Settings,
  type SummaryEngine,
  type SummaryProvider,
  type TranscriptionEngine,
  type WhisperApiProvider,
} from '../ipc/settings';
import ModelManager from './ModelManager';
import { useSettings } from '../state/useSettings';

const KEY_PROVIDERS: { id: WhisperApiProvider | SummaryProvider; label: string }[] = [
  { id: 'groq', label: 'Groq' },
  { id: 'openAi', label: 'OpenAI' },
  { id: 'anthropic', label: 'Anthropic' },
];

/**
 * Where the user decides how their meetings are handled: where notes go,
 * which engine transcribes, which model summarizes, and API keys.
 *
 * Owns its own state via `useSettings`; IPC is stubbed in tests with
 * `vi.mock('../ipc/settings', ...)`.
 */
export default function Settings({ onChanged }: { onChanged?: () => void } = {}) {
  const { load, keys, saveState, save, saveKey, removeKey, reload } = useSettings();

  // Anything saved here can decide whether the app is able to record at all,
  // so whoever owns the readiness check has to be told (ADR-0019).
  async function announce<T>(action: Promise<T>): Promise<T> {
    const result = await action;
    onChanged?.();
    return result;
  }

  if (load.status === 'loading') {
    return <p className="status">Loading settings…</p>;
  }

  if (load.status === 'failed') {
    return (
      <div className="settings-error">
        <p>Could not load settings: {load.error}</p>
        <button type="button" onClick={reload}>
          Retry
        </button>
      </div>
    );
  }

  return (
    <SettingsForm
      settings={load.settings}
      keys={keys}
      saveState={saveState}
      onSave={(next) => void announce(save(next))}
      onSaveKey={(account, key) => announce(saveKey(account, key))}
      onRemoveKey={(account) => announce(removeKey(account))}
      onModelsChanged={() => onChanged?.()}
    />
  );
}

interface FormProps {
  settings: Settings;
  keys: KeyStatus[];
  saveState: ReturnType<typeof useSettings>['saveState'];
  onSave: (settings: Settings) => void;
  onSaveKey: (account: string, key: string) => Promise<KeyStatus>;
  onRemoveKey: (account: string) => Promise<KeyStatus>;
  onModelsChanged: () => void;
}

function SettingsForm({
  settings,
  keys,
  saveState,
  onSave,
  onSaveKey,
  onRemoveKey,
  onModelsChanged,
}: FormProps) {
  const [draft, setDraft] = useState<Settings>(settings);
  const [missingKeyWarning, setMissingKeyWarning] = useState<string | undefined>(undefined);

  // The saved settings changed under us (a fresh load, or a save that
  // succeeded) — pick up the new baseline instead of fighting it.
  useEffect(() => {
    setDraft(settings);
  }, [settings]);

  const cloudTranscription = sendsAudioToTheCloud(draft);
  const requiredAccount = requiredTranscriptionAccount(draft);
  const requiredKeyConfigured = keys.some(
    (status) => status.account === requiredAccount && status.configured,
  );

  // The warning is a pre-save check, not a fact about the current draft —
  // once the draft no longer needs it (engine switched back, or the key got
  // configured), stop showing it instead of waiting for the next submit.
  useEffect(() => {
    if (requiredAccount === undefined || requiredKeyConfigured) {
      setMissingKeyWarning(undefined);
    }
  }, [requiredAccount, requiredKeyConfigured]);

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (requiredAccount !== undefined && !requiredKeyConfigured) {
      setMissingKeyWarning(requiredAccount);
      return;
    }
    setMissingKeyWarning(undefined);
    onSave(draft);
  }

  function handleSaveAnyway() {
    setMissingKeyWarning(undefined);
    onSave(draft);
  }

  return (
    <form className="settings" onSubmit={handleSubmit}>
      {cloudTranscription && (
        <p className="settings__cloud-warning" role="alert">
          This configuration sends your meeting audio to the cloud for transcription.
        </p>
      )}

      {/*
       * There is no local note writer, and an agent CLI is not one either: it
       * sends the transcript to a frontier model under the user's own
       * subscription, and keeps its own copy of it. "Installed here" reads as
       * "private" next to a Local transcription engine, so this says
       * otherwise — ADR-0020 requires the CLI route to be labelled wherever
       * the API route is.
       */}
      <p className="settings__summary-warning" role="alert">
        {draft.summaryEngine === 'cli'
          ? 'Writing the notes sends the transcript to the agent CLI you pick. That is not local: it goes to that CLI’s provider under your account, and the CLI keeps its own copy in its history.'
          : 'Writing the notes sends the transcript to your summary provider. Resumeira has no local note writer.'}
      </p>

      <label className="settings__field">
        <span>Notes folder</span>
        <input
          type="text"
          placeholder="Default location"
          value={draft.notesFolder ?? ''}
          onChange={(event) =>
            setDraft({
              ...draft,
              notesFolder: event.target.value === '' ? null : event.target.value,
            })
          }
        />
      </label>

      <label className="settings__field">
        <span>Transcription engine</span>
        <select
          value={draft.transcription.engine}
          onChange={(event) =>
            setDraft({
              ...draft,
              transcription: {
                ...draft.transcription,
                engine: event.target.value as TranscriptionEngine,
              },
            })
          }
        >
          <option value="local">Local (on this device)</option>
          <option value="api">API (cloud)</option>
        </select>
      </label>

      {draft.transcription.engine === 'api' ? (
        <label className="settings__field">
          <span>Transcription provider</span>
          <select
            value={draft.transcription.provider}
            onChange={(event) =>
              setDraft({
                ...draft,
                transcription: {
                  ...draft.transcription,
                  provider: event.target.value as WhisperApiProvider,
                },
              })
            }
          >
            <option value="groq">Groq</option>
            <option value="openAi">OpenAI</option>
          </select>
        </label>
      ) : (
        <label className="settings__field">
          <span>Local Whisper model</span>
          <input
            type="text"
            value={draft.transcription.localModel}
            onChange={(event) =>
              setDraft({
                ...draft,
                transcription: { ...draft.transcription, localModel: event.target.value },
              })
            }
          />
        </label>
      )}

      <label className="settings__field">
        <span>Notes written by</span>
        <select
          value={draft.summaryEngine}
          onChange={(event) =>
            setDraft({ ...draft, summaryEngine: event.target.value as SummaryEngine })
          }
        >
          <option value="api">A cloud API (needs a key)</option>
          <option value="cli">An agent CLI installed here (uses your subscription)</option>
        </select>
      </label>

      {draft.summaryEngine === 'cli' ? (
        <label className="settings__field">
          <span>Agent CLI</span>
          <select
            value={draft.summaryCli}
            onChange={(event) => setDraft({ ...draft, summaryCli: event.target.value as AgentCli })}
          >
            {AGENT_CLIS.map(({ id, label }) => (
              <option key={id} value={id}>
                {label}
              </option>
            ))}
          </select>
        </label>
      ) : (
        <>
          <label className="settings__field">
            <span>Summary provider</span>
            <select
              value={draft.summaryProvider}
              onChange={(event) =>
                setDraft({ ...draft, summaryProvider: event.target.value as SummaryProvider })
              }
            >
              <option value="anthropic">Anthropic</option>
              <option value="openAi">OpenAI</option>
              <option value="groq">Groq</option>
            </select>
          </label>

          <label className="settings__field">
            <span>Summary model</span>
            <input
              type="text"
              placeholder="Provider default"
              value={draft.summaryModel ?? ''}
              onChange={(event) =>
                setDraft({
                  ...draft,
                  summaryModel: event.target.value === '' ? null : event.target.value,
                })
              }
            />
          </label>
        </>
      )}

      <label className="settings__field">
        <span>Audio retention</span>
        <select
          value={draft.audioRetention}
          onChange={(event) =>
            setDraft({ ...draft, audioRetention: event.target.value as AudioRetention })
          }
        >
          <option value="keep">Keep audio after transcription</option>
          <option value="deleteAfterTranscription">Delete audio after transcription</option>
        </select>
      </label>

      <label className="settings__field settings__field--checkbox">
        <input
          type="checkbox"
          checked={draft.telemetryOptIn}
          onChange={(event) => setDraft({ ...draft, telemetryOptIn: event.target.checked })}
        />
        <span>Share anonymous usage telemetry</span>
      </label>

      <ApiKeys keys={keys} onSaveKey={onSaveKey} onRemoveKey={onRemoveKey} />

      <ModelManager onChanged={onModelsChanged} />

      {missingKeyWarning !== undefined && (
        <div className="settings__key-warning" role="alert">
          <p>No key saved for {missingKeyWarning} — transcription will fail without one.</p>
          <button type="button" onClick={handleSaveAnyway}>
            Save anyway
          </button>
        </div>
      )}

      {saveState.status === 'failed' && (
        <p className="settings__save-error" role="alert">
          Could not save settings: {saveState.error}
        </p>
      )}

      <button type="submit" disabled={saveState.status === 'saving'}>
        {saveState.status === 'saving' ? 'Saving…' : 'Save'}
      </button>
    </form>
  );
}

interface ApiKeysProps {
  keys: KeyStatus[];
  onSaveKey: (account: string, key: string) => Promise<KeyStatus>;
  onRemoveKey: (account: string) => Promise<KeyStatus>;
}

/**
 * Keys are write-only: Rust never returns a value, only `configured` and a
 * masked `hint` (ADR-0009). The input clears itself after a successful save
 * so nothing that looks like a key lingers on screen.
 */
function ApiKeys({ keys, onSaveKey, onRemoveKey }: ApiKeysProps) {
  const [drafts, setDrafts] = useState<Record<string, string>>({});

  async function handleSave(account: string) {
    const value = drafts[account];
    if (value === undefined || value === '') return;
    await onSaveKey(account, value);
    setDrafts((previous) => ({ ...previous, [account]: '' }));
  }

  return (
    <fieldset className="settings__keys">
      <legend>API keys</legend>
      {KEY_PROVIDERS.map(({ id, label }) => {
        const account = accountFor(id);
        const status = keys.find((entry) => entry.account === account);
        return (
          <div className="settings__key" key={account}>
            <span className="settings__key-label">{label}</span>
            <span className="settings__key-status">
              {status?.configured === true
                ? `Configured (${status.hint ?? 'saved'})`
                : 'Not configured'}
            </span>
            <input
              type="password"
              autoComplete="off"
              placeholder={`${label} API key`}
              aria-label={`${label} API key`}
              value={drafts[account] ?? ''}
              onChange={(event) =>
                setDrafts((previous) => ({ ...previous, [account]: event.target.value }))
              }
            />
            <button type="button" onClick={() => void handleSave(account)}>
              Save {label} key
            </button>
            {status?.configured === true && (
              <button type="button" onClick={() => void onRemoveKey(account)}>
                Remove
              </button>
            )}
          </div>
        );
      })}
    </fieldset>
  );
}
