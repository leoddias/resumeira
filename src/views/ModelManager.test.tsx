import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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

// Imported after the mock so the module under test picks it up.
const { default: ModelManager } = await import('./ModelManager');

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

async function renderReady(models: ModelEntry[] = catalogue) {
  listModels.mockResolvedValue(models);
  render(<ModelManager />);
  await waitFor(() => expect(screen.getByText('Base')).toBeInTheDocument());
}

describe('ModelManager', () => {
  it('shows a loading state before the catalogue arrives', () => {
    listModels.mockReturnValue(new Promise(() => {}));
    render(<ModelManager />);
    expect(screen.getByText('Loading models…')).toBeInTheDocument();
  });

  it('lists every catalogue model with its size and installed status', async () => {
    await renderReady();
    expect(screen.getByText('Base')).toBeInTheDocument();
    expect(screen.getByText('150 MB')).toBeInTheDocument();
    expect(screen.getByText('Not installed')).toBeInTheDocument();

    expect(screen.getByText('Large v3 Turbo')).toBeInTheDocument();
    expect(screen.getByText('1.6 GB')).toBeInTheDocument();
    expect(screen.getByText('Installed')).toBeInTheDocument();
  });

  it('reports a load failure and offers a retry', async () => {
    listModels.mockRejectedValueOnce(new Error('offline'));
    render(<ModelManager />);
    await waitFor(() =>
      expect(screen.getByText('Could not load models: offline')).toBeInTheDocument(),
    );

    listModels.mockResolvedValueOnce(catalogue);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.getByText('Base')).toBeInTheDocument());
  });

  it('shows an empty progress state right when a download starts, before any event arrives', async () => {
    downloadModel.mockReturnValue(new Promise(() => {}));
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Download' }));

    expect(screen.getByRole('progressbar', { name: 'Base download progress' })).toHaveAttribute(
      'aria-valuenow',
      '0',
    );
    expect(screen.getByText(/0% \(— of —\)/)).toBeInTheDocument();
  });

  it('shows real download progress from the model-progress event, not a spinner', async () => {
    downloadModel.mockReturnValue(new Promise(() => {}));
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Download' }));

    progressHandler?.({
      id: 'base',
      downloadedBytes: 75_000_000,
      totalBytes: 150_000_000,
      percent: 50,
    });

    await waitFor(() =>
      expect(screen.getByRole('progressbar', { name: 'Base download progress' })).toHaveAttribute(
        'aria-valuenow',
        '50',
      ),
    );
    expect(screen.getByText(/75 MB of 150 MB/)).toBeInTheDocument();
  });

  it('reports a download failure and leaves the model not installed', async () => {
    listModels.mockResolvedValue(catalogue);
    downloadModel.mockRejectedValue(new Error('checksum mismatch'));
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Download' }));

    await waitFor(() =>
      expect(screen.getByText('Download failed: checksum mismatch')).toBeInTheDocument(),
    );
    expect(screen.getByText('Not installed')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry download' })).toBeInTheDocument();
  });

  it('asks for confirmation before deleting an installed model', async () => {
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(deleteModel).not.toHaveBeenCalled();
    expect(screen.getByText(/Delete Large v3 Turbo\?/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(deleteModel).not.toHaveBeenCalled();
    expect(screen.queryByText(/Delete Large v3 Turbo\?/)).not.toBeInTheDocument();
  });

  it('deletes the model after the confirmation is accepted', async () => {
    deleteModel.mockResolvedValue(undefined);
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await userEvent.click(screen.getByRole('button', { name: 'Confirm delete' }));

    await waitFor(() => expect(deleteModel).toHaveBeenCalledWith('large-v3-turbo'));
  });

  it('reports a delete failure and leaves the model shown as installed', async () => {
    deleteModel.mockRejectedValue(new Error('file in use'));
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await userEvent.click(screen.getByRole('button', { name: 'Confirm delete' }));

    await waitFor(() => expect(screen.getByText('Delete failed: file in use')).toBeInTheDocument());
    expect(screen.getByText('Installed')).toBeInTheDocument();
  });

  it('opens the models folder for a manual install', async () => {
    openModelsFolder.mockResolvedValue(undefined);
    await renderReady();

    await userEvent.click(screen.getByRole('button', { name: 'Open models folder' }));

    expect(openModelsFolder).toHaveBeenCalledOnce();
  });
});
