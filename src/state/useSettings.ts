import { useCallback, useEffect, useState } from 'react';
import {
  deleteApiKey,
  getKeyStatus,
  getSettings,
  saveSettings,
  setApiKey,
  type KeyStatus,
  type Settings,
} from '../ipc/settings';

export type SettingsLoadState =
  | { status: 'loading' }
  | { status: 'ready'; settings: Settings }
  | { status: 'failed'; error: string };

export type SaveState =
  { status: 'idle' } | { status: 'saving' } | { status: 'failed'; error: string };

/**
 * Settings state, kept separate from the view so the view can stay a plain
 * form. Key material never lives here as a value — only the write-only
 * `KeyStatus` Rust reports back (ADR-0009).
 */
export function useSettings() {
  const [load, setLoad] = useState<SettingsLoadState>({ status: 'loading' });
  const [keys, setKeys] = useState<KeyStatus[]>([]);
  const [saveState, setSaveState] = useState<SaveState>({ status: 'idle' });
  const [reloadToken, setReloadToken] = useState(0);

  const refreshKeys = useCallback(() => {
    getKeyStatus()
      .then(setKeys)
      .catch(() => {
        // Key status is secondary; a failed refresh keeps the previous
        // (possibly stale) list rather than blanking the whole screen.
      });
  }, []);

  useEffect(() => {
    let active = true;
    setLoad({ status: 'loading' });
    getSettings()
      .then((settings) => {
        if (active) setLoad({ status: 'ready', settings });
      })
      .catch((error: unknown) => {
        if (active) setLoad({ status: 'failed', error: describe(error) });
      });
    refreshKeys();
    return () => {
      active = false;
    };
  }, [refreshKeys, reloadToken]);

  const reload = useCallback(() => {
    setReloadToken((token) => token + 1);
  }, []);

  const save = useCallback(async (settings: Settings) => {
    setSaveState({ status: 'saving' });
    try {
      await saveSettings(settings);
      setLoad({ status: 'ready', settings });
      setSaveState({ status: 'idle' });
    } catch (error: unknown) {
      setSaveState({ status: 'failed', error: describe(error) });
    }
  }, []);

  const saveKey = useCallback(async (account: string, key: string) => {
    const status = await setApiKey(account, key);
    setKeys((previous) => [...previous.filter((entry) => entry.account !== account), status]);
    return status;
  }, []);

  const removeKey = useCallback(async (account: string) => {
    const status = await deleteApiKey(account);
    setKeys((previous) => [...previous.filter((entry) => entry.account !== account), status]);
    return status;
  }, []);

  return { load, keys, saveState, save, saveKey, removeKey, reload };
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
