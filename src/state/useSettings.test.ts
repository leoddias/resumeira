import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Settings } from '../ipc/settings';

const getSettings = vi.fn();
const saveSettings = vi.fn();
const getKeyStatus = vi.fn();
const setApiKey = vi.fn();
const deleteApiKey = vi.fn();

vi.mock('../ipc/settings', () => ({
  getSettings: () => getSettings(),
  saveSettings: (settings: Settings) => saveSettings(settings),
  getKeyStatus: () => getKeyStatus(),
  setApiKey: (account: string, key: string) => setApiKey(account, key),
  deleteApiKey: (account: string) => deleteApiKey(account),
}));

const { useSettings } = await import('./useSettings');

const settings: Settings = {
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

beforeEach(() => {
  getSettings.mockReset();
  saveSettings.mockReset();
  getKeyStatus.mockReset();
  setApiKey.mockReset();
  deleteApiKey.mockReset();
  getKeyStatus.mockResolvedValue([]);
});

describe('useSettings', () => {
  it('starts loading and then reports the loaded settings', async () => {
    getSettings.mockResolvedValue(settings);
    const { result } = renderHook(() => useSettings());

    expect(result.current.load.status).toBe('loading');
    await waitFor(() => expect(result.current.load.status).toBe('ready'));
    expect(result.current.load).toEqual({ status: 'ready', settings });
  });

  it('reports a load failure', async () => {
    getSettings.mockRejectedValue(new Error('disk unavailable'));
    const { result } = renderHook(() => useSettings());

    await waitFor(() => expect(result.current.load.status).toBe('failed'));
    expect(result.current.load).toEqual({ status: 'failed', error: 'disk unavailable' });
  });

  it('goes through saving to idle on a successful save', async () => {
    getSettings.mockResolvedValue(settings);
    saveSettings.mockResolvedValue(undefined);
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    const updated: Settings = { ...settings, telemetryOptIn: true };
    await act(async () => {
      await result.current.save(updated);
    });

    expect(saveSettings).toHaveBeenCalledWith(updated);
    expect(result.current.saveState).toEqual({ status: 'idle' });
    expect(result.current.load).toEqual({ status: 'ready', settings: updated });
  });

  it('reports a save failure without losing the loaded settings', async () => {
    getSettings.mockResolvedValue(settings);
    saveSettings.mockRejectedValue(new Error('permission denied'));
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    await act(async () => {
      await result.current.save({ ...settings, telemetryOptIn: true });
    });

    expect(result.current.saveState).toEqual({ status: 'failed', error: 'permission denied' });
    expect(result.current.load).toEqual({ status: 'ready', settings });
  });

  it('never exposes a key value, only the write-only status Rust returns', async () => {
    getSettings.mockResolvedValue(settings);
    setApiKey.mockResolvedValue({ account: 'groq', configured: true, hint: '••••wxyz' });
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    await act(async () => {
      await result.current.saveKey('groq', 'sk-a-real-secret');
    });

    expect(setApiKey).toHaveBeenCalledWith('groq', 'sk-a-real-secret');
    expect(result.current.keys).toEqual([{ account: 'groq', configured: true, hint: '••••wxyz' }]);
    expect(JSON.stringify(result.current)).not.toContain('sk-a-real-secret');
  });
});
