import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ModelEntry, ModelProgress } from '../ipc/models';

const listModels = vi.fn();
const downloadModel = vi.fn();
const deleteModel = vi.fn();
const openModelsFolder = vi.fn();
const onModelProgress = vi.fn();

vi.mock('../ipc/models', async () => {
  const actual = await vi.importActual<typeof import('../ipc/models')>('../ipc/models');
  return {
    ...actual,
    listModels: () => listModels(),
    downloadModel: (id: string) => downloadModel(id),
    deleteModel: (id: string) => deleteModel(id),
    openModelsFolder: () => openModelsFolder(),
    onModelProgress: (handler: (progress: ModelProgress) => void) => onModelProgress(handler),
  };
});

const { useModels } = await import('./useModels');

const catalogue: ModelEntry[] = [
  { id: 'base', displayName: 'Base', sizeBytes: 150_000_000, installed: false },
  {
    id: 'large-v3-turbo',
    displayName: 'Large v3 Turbo',
    sizeBytes: 1_600_000_000,
    installed: true,
  },
];

let progressHandler: ((progress: ModelProgress) => void) | undefined;

beforeEach(() => {
  listModels.mockReset();
  downloadModel.mockReset();
  deleteModel.mockReset();
  openModelsFolder.mockReset();
  onModelProgress.mockReset();
  progressHandler = undefined;
  onModelProgress.mockImplementation((handler: (progress: ModelProgress) => void) => {
    progressHandler = handler;
    return Promise.resolve(() => {});
  });
});

describe('useModels', () => {
  it('starts loading and then reports the catalogue', async () => {
    listModels.mockResolvedValue(catalogue);
    const { result } = renderHook(() => useModels());

    expect(result.current.load.status).toBe('loading');
    await waitFor(() => expect(result.current.load.status).toBe('ready'));
    expect(result.current.load).toEqual({ status: 'ready', models: catalogue });
  });

  it('reports a load failure', async () => {
    listModels.mockRejectedValue(new Error('network unreachable'));
    const { result } = renderHook(() => useModels());

    await waitFor(() => expect(result.current.load.status).toBe('failed'));
    expect(result.current.load).toEqual({ status: 'failed', error: 'network unreachable' });
  });

  it('tracks real progress from the model-progress event stream', async () => {
    listModels.mockResolvedValue(catalogue);
    downloadModel.mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => useModels());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    act(() => {
      void result.current.download('base');
    });
    expect(result.current.downloads.base).toEqual({
      status: 'downloading',
      downloadedBytes: 0,
      totalBytes: 0,
      percent: 0,
    });

    act(() => {
      progressHandler?.({
        id: 'base',
        downloadedBytes: 75_000_000,
        totalBytes: 150_000_000,
        percent: 50,
      });
    });

    expect(result.current.downloads.base).toEqual({
      status: 'downloading',
      downloadedBytes: 75_000_000,
      totalBytes: 150_000_000,
      percent: 50,
    });
  });

  it('leaves the model uninstalled and reports the reason when a download fails', async () => {
    listModels.mockResolvedValueOnce(catalogue);
    listModels.mockResolvedValueOnce(catalogue);
    downloadModel.mockRejectedValue(new Error('checksum mismatch'));
    const { result } = renderHook(() => useModels());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    await act(async () => {
      await result.current.download('base');
    });

    expect(result.current.downloads.base).toEqual({
      status: 'failed',
      error: 'checksum mismatch',
    });
    expect(listModels).toHaveBeenCalledTimes(2);
  });

  it('goes back to idle and refreshes the catalogue on a successful download', async () => {
    listModels.mockResolvedValueOnce(catalogue);
    const installed = catalogue.map((model) =>
      model.id === 'base' ? { ...model, installed: true } : model,
    );
    listModels.mockResolvedValueOnce(installed);
    downloadModel.mockResolvedValue(undefined);
    const { result } = renderHook(() => useModels());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    await act(async () => {
      await result.current.download('base');
    });

    expect(result.current.downloads.base).toEqual({ status: 'idle' });
    expect(result.current.load).toEqual({ status: 'ready', models: installed });
  });

  it('deletes a model and refreshes the catalogue', async () => {
    listModels.mockResolvedValueOnce(catalogue);
    const uninstalled = catalogue.map((model) =>
      model.id === 'large-v3-turbo' ? { ...model, installed: false } : model,
    );
    listModels.mockResolvedValueOnce(uninstalled);
    deleteModel.mockResolvedValue(undefined);
    const { result } = renderHook(() => useModels());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    await act(async () => {
      await result.current.remove('large-v3-turbo');
    });

    expect(deleteModel).toHaveBeenCalledWith('large-v3-turbo');
    expect(result.current.load).toEqual({ status: 'ready', models: uninstalled });
  });

  it('opens the models folder', async () => {
    listModels.mockResolvedValue(catalogue);
    openModelsFolder.mockResolvedValue(undefined);
    const { result } = renderHook(() => useModels());
    await waitFor(() => expect(result.current.load.status).toBe('ready'));

    await act(async () => {
      await result.current.openFolder();
    });

    expect(openModelsFolder).toHaveBeenCalledOnce();
  });
});
