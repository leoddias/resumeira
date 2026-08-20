import { useState } from 'react';
import { formatSize, type ModelEntry } from '../ipc/models';
import { useModels, type DeleteState, type DownloadState } from '../state/useModels';

/**
 * Lets a user install a local Whisper model without knowing what a `.bin`
 * file is. Without this the local engine — the app's "never leaves this
 * machine" promise — is reachable only by someone who reads the source.
 *
 * Owns its own state via `useModels`; IPC is stubbed in tests with
 * `vi.mock('../ipc/models', ...)`.
 */
export default function ModelManager({ onChanged }: { onChanged?: () => void } = {}) {
  const { load, downloads, deletes, download, remove, openFolder, refresh } = useModels();

  // Installing or deleting a model can be exactly what unblocks (or blocks)
  // recording, so whoever is showing this needs to hear about it.
  async function changed(action: Promise<void>) {
    await action;
    onChanged?.();
  }

  return (
    <div className="model-manager">
      <div className="model-manager__header">
        <h2>Local models</h2>
        <button type="button" onClick={() => void openFolder()}>
          Open models folder
        </button>
      </div>

      {load.status === 'loading' && <p className="status">Loading models…</p>}

      {load.status === 'failed' && (
        <div className="model-manager__error">
          <p>Could not load models: {load.error}</p>
          <button type="button" onClick={() => void refresh()}>
            Retry
          </button>
        </div>
      )}

      {load.status === 'ready' && (
        <ul className="model-manager__list">
          {load.models.map((model) => (
            <ModelRow
              key={model.id}
              model={model}
              downloadState={downloads[model.id] ?? { status: 'idle' }}
              deleteState={deletes[model.id] ?? { status: 'idle' }}
              onDownload={() => void changed(download(model.id))}
              onDelete={() => void changed(remove(model.id))}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

interface RowProps {
  model: ModelEntry;
  downloadState: DownloadState;
  deleteState: DeleteState;
  onDownload: () => void;
  onDelete: () => void;
}

function ModelRow({ model, downloadState, deleteState, onDownload, onDelete }: RowProps) {
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const downloading = downloadState.status === 'downloading';

  return (
    <li className="model-manager__item">
      <div className="model-manager__info">
        <span className="model-manager__name">{model.displayName}</span>
        <span className="model-manager__size">{formatSize(model.sizeBytes)}</span>
        <span className="model-manager__status">
          {model.installed ? 'Installed' : 'Not installed'}
        </span>
      </div>

      {downloading && (
        <div
          className="model-manager__progress"
          role="progressbar"
          aria-label={`${model.displayName} download progress`}
          aria-valuenow={downloadState.percent}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div
            className="model-manager__progress-bar"
            style={{ width: `${downloadState.percent}%` }}
          />
          <span className="model-manager__progress-label">
            {downloadState.percent}% ({formatSize(downloadState.downloadedBytes)} of{' '}
            {formatSize(downloadState.totalBytes)})
          </span>
        </div>
      )}

      {downloadState.status === 'failed' && (
        <p className="model-manager__download-error" role="alert">
          Download failed: {downloadState.error}
        </p>
      )}

      {deleteState.status === 'failed' && (
        <p className="model-manager__delete-error" role="alert">
          Delete failed: {deleteState.error}
        </p>
      )}

      <div className="model-manager__actions">
        {!model.installed && !downloading && (
          <button type="button" onClick={onDownload}>
            {downloadState.status === 'failed' ? 'Retry download' : 'Download'}
          </button>
        )}

        {model.installed && !confirmingDelete && (
          <button type="button" onClick={() => setConfirmingDelete(true)}>
            Delete
          </button>
        )}

        {model.installed && confirmingDelete && (
          <div className="model-manager__confirm">
            <span>
              Delete {model.displayName}? You will need to download it again (
              {formatSize(model.sizeBytes)}) to use it.
            </span>
            <button
              type="button"
              onClick={() => {
                setConfirmingDelete(false);
                onDelete();
              }}
            >
              Confirm delete
            </button>
            <button type="button" onClick={() => setConfirmingDelete(false)}>
              Cancel
            </button>
          </div>
        )}
      </div>
    </li>
  );
}
