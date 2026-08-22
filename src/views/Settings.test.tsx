import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { KeyStatus, Settings as SettingsType } from '../ipc/settings';

const getSettings = vi.fn();
const saveSettings = vi.fn();
const getKeyStatus = vi.fn();
const setApiKey = vi.fn();
const deleteApiKey = vi.fn();

vi.mock('../ipc/settings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/settings')>('../ipc/settings');
  return {
    ...actual,
    getSettings: () => getSettings(),
    saveSettings: (settings: SettingsType) => saveSettings(settings),
    getKeyStatus: () => getKeyStatus(),
    setApiKey: (account: string, key: string) => setApiKey(account, key),
    deleteApiKey: (account: string) => deleteApiKey(account),
  };
});

// Imported after the mock so the module under test picks it up.
const { default: Settings } = await import('./Settings');

const localSettings: SettingsType = {
  notesFolder: null,
  transcription: { engine: 'local', provider: 'groq', localModel: 'large-v3-turbo' },
  summaryEngine: 'api',
  summaryProvider: 'anthropic',
  summaryCli: 'claude',
  summaryModel: null,
  identifySpeakers: true,
  audioRetention: 'keep',
  telemetryOptIn: false,
};

const noKeys: KeyStatus[] = [
  { account: 'groq', configured: false },
  { account: 'openai', configured: false },
  { account: 'anthropic', configured: false },
];

beforeEach(() => {
  getSettings.mockReset();
  saveSettings.mockReset();
  getKeyStatus.mockReset();
  setApiKey.mockReset();
  deleteApiKey.mockReset();
  getKeyStatus.mockResolvedValue(noKeys);
});

async function renderReady(settings: SettingsType = localSettings, keys: KeyStatus[] = noKeys) {
  getSettings.mockResolvedValue(settings);
  getKeyStatus.mockResolvedValue(keys);
  render(<Settings />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeInTheDocument());
}

describe('Settings', () => {
  it('shows a loading state before settings arrive', () => {
    getSettings.mockReturnValue(new Promise(() => {}));
    render(<Settings />);
    expect(screen.getByText('Loading settings…')).toBeInTheDocument();
  });

  it('reports a failed load and offers a retry', async () => {
    getSettings.mockRejectedValueOnce(new Error('disk unavailable'));
    render(<Settings />);
    await waitFor(() =>
      expect(screen.getByText(/Could not load settings: disk unavailable/)).toBeInTheDocument(),
    );

    getSettings.mockResolvedValueOnce(localSettings);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeInTheDocument());
  });

  it('never renders a key value, only configured status and hint', async () => {
    await renderReady(localSettings, [
      { account: 'anthropic', configured: true, hint: '••••cdef' },
    ]);
    expect(screen.getByText(/Configured \(••••cdef\)/)).toBeInTheDocument();
    expect(screen.queryByDisplayValue(/cdef/)).not.toBeInTheDocument();
  });

  it('clears the key input after a successful save', async () => {
    setApiKey.mockResolvedValue({ account: 'groq', configured: true, hint: '••••wxyz' });
    await renderReady();

    const input = screen.getByLabelText('Groq API key');
    await userEvent.type(input, 'sk-secret-value');
    await userEvent.click(screen.getByRole('button', { name: 'Save Groq key' }));

    await waitFor(() => expect(setApiKey).toHaveBeenCalledWith('groq', 'sk-secret-value'));
    expect(input).toHaveValue('');
  });

  it('states plainly that audio leaves the machine when the API engine is chosen', async () => {
    await renderReady();
    expect(screen.queryByText(/sends your meeting audio to the cloud/)).not.toBeInTheDocument();

    await userEvent.selectOptions(
      screen.getByRole('combobox', { name: /Transcription engine/i }),
      'api',
    );

    expect(screen.getByText(/sends your meeting audio to the cloud/)).toBeInTheDocument();
  });

  it('removes the cloud warning when switching back to the local engine', async () => {
    await renderReady({
      ...localSettings,
      transcription: { engine: 'api', provider: 'groq', localModel: 'x' },
    });
    expect(screen.getByText(/sends your meeting audio to the cloud/)).toBeInTheDocument();

    await userEvent.selectOptions(
      screen.getByRole('combobox', { name: /Transcription engine/i }),
      'local',
    );

    expect(screen.queryByText(/sends your meeting audio to the cloud/)).not.toBeInTheDocument();
  });

  it('warns before saving the API engine without a key for that provider', async () => {
    await renderReady();
    await userEvent.selectOptions(
      screen.getByRole('combobox', { name: /Transcription engine/i }),
      'api',
    );
    await userEvent.selectOptions(
      screen.getByRole('combobox', { name: /Transcription provider/i }),
      'openAi',
    );

    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(screen.getByText(/No key saved for openai/)).toBeInTheDocument();
    expect(saveSettings).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('button', { name: 'Save anyway' }));
    expect(saveSettings).toHaveBeenCalledOnce();
  });

  it('saves without a warning once the required key is configured', async () => {
    await renderReady(
      { ...localSettings, transcription: { engine: 'api', provider: 'groq', localModel: 'x' } },
      [{ account: 'groq', configured: true, hint: '••••ab12' }],
    );

    saveSettings.mockResolvedValue(undefined);
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(screen.queryByText(/No key saved for/)).not.toBeInTheDocument();
    await waitFor(() => expect(saveSettings).toHaveBeenCalledOnce());
  });

  it('reports a failed save', async () => {
    await renderReady();
    saveSettings.mockRejectedValueOnce(new Error('permission denied'));

    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(screen.getByText(/Could not save settings: permission denied/)).toBeInTheDocument(),
    );
  });

  it('says the CLI route leaves the machine, because "installed here" reads as private', async () => {
    // ADR-0020: the CLI is labelled wherever the API route is. A user who
    // pairs Local transcription with an agent CLI is otherwise told "local"
    // twice for a path that uploads the whole transcript.
    await renderReady({ ...localSettings, summaryEngine: 'cli' });

    const warnings = screen.getAllByRole('alert').map((node) => node.textContent ?? '');
    expect(warnings.join(' ')).toMatch(/not local/i);
    expect(warnings.join(' ')).toMatch(/keeps its own copy/i);
  });

  it('offers the agent CLI instead of a provider and model when that engine is chosen', async () => {
    await renderReady({ ...localSettings, summaryEngine: 'cli' });

    expect(screen.getByLabelText('Agent CLI')).toHaveValue('claude');
    // The API-only fields would be dead controls under this engine.
    expect(screen.queryByLabelText('Summary provider')).toBeNull();
    expect(screen.queryByLabelText('Summary model')).toBeNull();
  });

  it('saves speaker identification being turned off', async () => {
    saveSettings.mockResolvedValue(undefined);
    await renderReady();

    await userEvent.click(screen.getByLabelText(/Identify who spoke/i));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({ identifySpeakers: false }),
      ),
    );
  });

  it('says the extra request happens, so nobody is surprised by the bill', async () => {
    await renderReady();
    expect(screen.getByText(/one extra request to the summary engine/i)).toBeInTheDocument();
  });

  it('saves the chosen agent CLI', async () => {
    saveSettings.mockResolvedValue(undefined);
    await renderReady({ ...localSettings, summaryEngine: 'cli' });

    await userEvent.selectOptions(screen.getByLabelText('Agent CLI'), 'gemini');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith({
        ...localSettings,
        summaryEngine: 'cli',
        summaryCli: 'gemini',
      }),
    );
  });

  it('shows a saving state while the request is in flight', async () => {
    const deferred: { resolve: () => void } = { resolve: () => {} };
    saveSettings.mockReturnValue(
      new Promise<void>((resolve) => {
        deferred.resolve = resolve;
      }),
    );
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();

    deferred.resolve();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeInTheDocument());
  });
});
