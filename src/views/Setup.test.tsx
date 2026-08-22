import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Readiness } from '../ipc/readiness';
import { getSettings, saveSettings, setApiKey, type Settings } from '../ipc/settings';
import Setup from './Setup';

vi.mock('../ipc/settings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/settings')>('../ipc/settings');
  return {
    ...actual,
    getSettings: vi.fn(),
    saveSettings: vi.fn(() => Promise.resolve()),
    getKeyStatus: vi.fn(() => Promise.resolve([])),
    setApiKey: vi.fn(() => Promise.resolve({ account: 'anthropic', configured: true })),
    deleteApiKey: vi.fn(),
  };
});

vi.mock('../ipc/models', () => ({
  listModels: vi.fn(() => Promise.resolve([])),
  downloadModel: vi.fn(),
  deleteModel: vi.fn(),
  openModelsFolder: vi.fn(),
  onModelProgress: vi.fn(() => Promise.resolve(() => {})),
  formatSize: (bytes: number) => `${bytes} B`,
  MODEL_PROGRESS_EVENT: 'model-progress',
}));

const SETTINGS: Settings = {
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

const READY_STEP = { ready: true, detail: 'Local · large-v3-turbo', leavesTheMachine: false };

function readiness(overrides: Partial<Readiness>): Readiness {
  return {
    transcription: READY_STEP,
    summary: { ready: true, detail: 'anthropic · claude-sonnet-5', leavesTheMachine: true },
    canRecord: false,
    ...overrides,
  };
}

describe('Setup', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getSettings).mockResolvedValue(SETTINGS);
  });

  it('says which step is missing and why it blocks recording', async () => {
    render(
      <Setup
        readiness={readiness({
          transcription: {
            ready: false,
            detail: 'Local · large-v3-turbo',
            leavesTheMachine: false,
            blocker: { kind: 'modelMissing', model: 'large-v3-turbo' },
          },
        })}
        onChanged={vi.fn()}
      />,
    );

    expect(await screen.findByText(/large-v3-turbo model downloaded/)).toBeInTheDocument();
    // The reason it blocks rather than warns is the whole argument (ADR-0019).
    expect(screen.getByText(/cannot be recorded a second time/)).toBeInTheDocument();
  });

  it('offers a cloud engine as the alternative to a 1.6 GB download', async () => {
    const onChanged = vi.fn();
    render(
      <Setup
        readiness={readiness({
          transcription: {
            ready: false,
            detail: 'Local · large-v3-turbo',
            leavesTheMachine: false,
            blocker: { kind: 'modelMissing', model: 'large-v3-turbo' },
          },
        })}
        onChanged={onChanged}
      />,
    );

    await userEvent.click(
      await screen.findByRole('button', { name: /transcribe through a cloud API/i }),
    );

    await waitFor(() => {
      expect(saveSettings).toHaveBeenCalledWith({
        ...SETTINGS,
        transcription: { ...SETTINGS.transcription, engine: 'api' },
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it('offers an installed agent CLI to a user with no API key', async () => {
    const onChanged = vi.fn();
    render(
      <Setup
        readiness={readiness({
          summary: {
            ready: false,
            detail: 'anthropic · claude-sonnet-5',
            leavesTheMachine: true,
            blocker: { kind: 'keyMissing', account: 'anthropic' },
          },
        })}
        onChanged={onChanged}
      />,
    );

    await userEvent.click(await screen.findByRole('button', { name: 'Use Claude Code' }));

    await waitFor(() => {
      expect(saveSettings).toHaveBeenCalledWith({
        ...SETTINGS,
        summaryEngine: 'cli',
        summaryCli: 'claude',
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it('takes a key without sending the user to another screen', async () => {
    render(
      <Setup
        readiness={readiness({
          summary: {
            ready: false,
            detail: 'anthropic · claude-sonnet-5',
            leavesTheMachine: true,
            blocker: { kind: 'keyMissing', account: 'anthropic' },
          },
        })}
        onChanged={vi.fn()}
      />,
    );

    const field = await screen.findByLabelText('anthropic API key');
    await userEvent.type(field, 'sk-test-key');
    await userEvent.click(screen.getByRole('button', { name: 'Save anthropic key' }));

    await waitFor(() => {
      expect(setApiKey).toHaveBeenCalledWith('anthropic', 'sk-test-key');
    });
    // Nothing that looks like a key is left on screen (ADR-0009).
    expect(field).toHaveValue('');
  });

  it('offers the other CLIs when the chosen one is not installed', async () => {
    render(
      <Setup
        readiness={readiness({
          summary: {
            ready: false,
            detail: 'Gemini CLI (CLI)',
            leavesTheMachine: true,
            blocker: { kind: 'cliMissing', cli: 'gemini', name: 'Gemini CLI' },
          },
        })}
        onChanged={vi.fn()}
      />,
    );

    expect(await screen.findByText(/Gemini CLI is not installed/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Use Claude Code instead' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Use Gemini CLI instead' })).toBeNull();
    expect(screen.getByRole('button', { name: /use an API key instead/i })).toBeInTheDocument();
  });

  it('never implies that a CLI keeps the transcript on the machine', async () => {
    render(
      <Setup
        readiness={readiness({
          summary: {
            ready: false,
            detail: 'Claude Code (CLI)',
            leavesTheMachine: true,
            blocker: { kind: 'cliMissing', cli: 'claude', name: 'Claude Code' },
          },
        })}
        onChanged={vi.fn()}
      />,
    );

    expect(await screen.findByRole('alert')).toHaveTextContent(
      /always sends that transcript to a model/i,
    );
  });
});
