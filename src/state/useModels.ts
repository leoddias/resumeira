import { useCallback, useEffect, useState } from 'react';
import {
  deleteModel as deleteModelIpc,
  downloadModel as downloadModelIpc,
  listModels,
  onModelProgress,
  openModelsFolder as openModelsFolderIpc,
  type ModelEntry,
} from '../ipc/models';

export type ModelsLoadState =
  | { status: 'loading' }
  | { status: 'ready'; models: ModelEntry[] }
  | { status: 'failed'; error: string };

export type DownloadState =
  | { status: 'idle' }
  | { status: 'downloading'; downloadedBytes: number; totalBytes: number; percent: number }
  | { status: 'failed'; error: string };

const IDLE_DOWNLOAD: DownloadState = { status: 'idle' };

/**
 * Local Whisper model catalogue and downloads, kept separate from the view
 * so the view stays a plain list. Progress comes from the real
 * `model-progress` event stream — the default model is ~1.6 GB, and an
 * indeterminate spinner for that long reads as frozen.
 */
export function useModels() {
  const [load, setLoad] = useState<ModelsLoadState>({ status: 'loading' });
  const [downloads, setDownloads] = useState<Record<string, DownloadState>>({});

  const refresh = useCallback(async () => {
    setLoad({ status: 'loading' });
    try {
      const models = await listModels();
      setLoad({ status: 'ready', models });
    } catch (error: unknown) {
      setLoad({ status: 'failed', error: describe(error) });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    onModelProgress((progress) => {
      if (!active) return;
      setDownloads((previous) => ({
        ...previous,
        [progress.id]: {
          status: 'downloading',
          downloadedBytes: progress.downloadedBytes,
          totalBytes: progress.totalBytes,
          percent: progress.percent,
        },
      }));
    })
      .then((fn) => {
        if (active) unlisten = fn;
        else fn();
      })
      .catch(() => {
        // Losing the subscription is not worth blanking the UI over; the
        // download call below still resolves or rejects on its own.
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const download = useCallback(
    async (id: string) => {
      setDownloads((previous) => ({
        ...previous,
        [id]: { status: 'downloading', downloadedBytes: 0, totalBytes: 0, percent: 0 },
      }));
      try {
        await downloadModelIpc(id);
        setDownloads((previous) => ({ ...previous, [id]: IDLE_DOWNLOAD }));
        await refresh();
      } catch (error: unknown) {
        // A failed or interrupted download never lands on disk as installed
        // — Rust stages the file and renames it only after the checksum
        // matches — so the UI must not show a stale progress bar as if the
        // model were still on its way in.
        setDownloads((previous) => ({
          ...previous,
          [id]: { status: 'failed', error: describe(error) },
        }));
        await refresh();
      }
    },
    [refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      await deleteModelIpc(id);
      await refresh();
    },
    [refresh],
  );

  const openFolder = useCallback(() => openModelsFolderIpc(), []);

  return { load, downloads, download, remove, openFolder, refresh };
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Unknown error';
}
