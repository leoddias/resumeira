import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeetingListItem } from '../ipc/meetings';
import type { ModelEntry } from '../ipc/models';
import type { KeyStatus, Settings } from '../ipc/settings';

const { listMeetings, listModels, getKeyStatus, getSettings } = vi.hoisted(() => ({
  listMeetings: vi.fn(),
  listModels: vi.fn(),
  getKeyStatus: vi.fn(),
  getSettings: vi.fn(),
}));

vi.mock('../ipc/meetings', () => ({ listMeetings }));
vi.mock('../ipc/models', () => ({ listModels }));
vi.mock('../ipc/settings', () => ({ getKeyStatus, getSettings }));

// Imported after the mocks so the hook picks up the mocked modules.
const { useFirstRun } = await import('./useFirstRun');

const settings: Settings = {
  notesFolder: null,
  transcription: { engine: 'local', provider: 'groq', localModel: 'large-v3-turbo' },
  summaryProvider: 'anthropic',
  summaryModel: null,
  audioRetention: 'keep',
  telemetryOptIn: false,
};

const meeting: MeetingListItem = {
  folder: '/a',
  title: 'Standup',
  createdAt: '2026-01-01T09:00:00',
  preview: 'Talked about X',
};

const noKeys: KeyStatus[] = [
  { account: 'groq', configured: false },
  { account: 'openai', configured: false },
  { account: 'anthropic', configured: false },
];

const configuredKey: KeyStatus[] = [
  { account: 'groq', configured: true, hint: '••••wxyz' },
  { account: 'openai', configured: false },
  { account: 'anthropic', configured: false },
];

const noModels: ModelEntry[] = [
  {
    id: 'large-v3-turbo',
    displayName: 'Large v3 Turbo',
    sizeBytes: 1_600_000_000,
    installed: false,
  },
];

const installedModel: ModelEntry[] = [
  {
    id: 'large-v3-turbo',
    displayName: 'Large v3 Turbo',
    sizeBytes: 1_600_000_000,
    installed: true,
  },
];

beforeEach(() => {
  listMeetings.mockReset();
  listModels.mockReset();
  getKeyStatus.mockReset();
  getSettings.mockReset();
});

describe('useFirstRun', () => {
  it('starts loading', () => {
    listMeetings.mockReturnValue(new Promise(() => {}));
    listModels.mockReturnValue(new Promise(() => {}));
    getKeyStatus.mockReturnValue(new Promise(() => {}));
    getSettings.mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => useFirstRun());
    expect(result.current.state).toEqual({ status: 'loading' });
  });

  it('shows onboarding for a brand new install: no meetings, no key, no model', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    const { result } = renderHook(() => useFirstRun());

    await waitFor(() => expect(result.current.state.status).toBe('visible'));
    expect(result.current.state).toEqual({ status: 'visible', notesFolder: null });
  });

  it('never shows onboarding once a meeting exists', async () => {
    listMeetings.mockResolvedValue([meeting]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    const { result } = renderHook(() => useFirstRun());

    await waitFor(() => expect(result.current.state.status).toBe('hidden'));
  });

  it('never shows onboarding once an API key is configured', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(configuredKey);
    getSettings.mockResolvedValue(settings);

    const { result } = renderHook(() => useFirstRun());

    await waitFor(() => expect(result.current.state.status).toBe('hidden'));
  });

  it('never shows onboarding once a local model is installed', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(installedModel);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    const { result } = renderHook(() => useFirstRun());

    await waitFor(() => expect(result.current.state.status).toBe('hidden'));
  });

  it('stays hidden rather than blocking the app when a lookup fails', async () => {
    listMeetings.mockRejectedValue(new Error('index unreadable'));
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    const { result } = renderHook(() => useFirstRun());

    await waitFor(() => expect(result.current.state.status).toBe('hidden'));
  });

  it('skip dismisses without calling any configuration IPC', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    const { result } = renderHook(() => useFirstRun());
    await waitFor(() => expect(result.current.state.status).toBe('visible'));

    act(() => result.current.skip());

    expect(result.current.state).toEqual({ status: 'hidden' });
  });
});
