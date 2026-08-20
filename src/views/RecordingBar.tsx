import { useEffect, useState } from 'react';
import { canStart, canStop, type RecordingState, type TrackStatus } from '../ipc/types';
import { formatElapsed } from './format';

interface Props {
  state: RecordingState;
  onStart: () => void;
  onStop: () => void;
  /**
   * Why recording is unavailable, when it is. Rust refuses to start in this
   * case anyway (ADR-0019); showing it here means the user reads the reason
   * instead of clicking a button that appears to do nothing.
   */
  blockedReason?: string;
}

/**
 * The always-visible recording control. Presentational on purpose: state
 * comes from Rust via `useRecording`, so this renders without any IPC and
 * can be tested directly.
 */
export default function RecordingBar({ state, onStart, onStop, blockedReason }: Props) {
  const elapsed = useElapsed(state);

  return (
    <div className="recording-bar" role="region" aria-label="Recording">
      <StatusDot state={state} />
      <span className="recording-bar__label">{describe(state)}</span>
      {elapsed !== undefined && (
        <span className="recording-bar__elapsed">{formatElapsed(elapsed)}</span>
      )}
      {state.status === 'recording' && <TrackList tracks={state.tracks} />}
      {blockedReason !== undefined && (
        <span className="recording-bar__blocked" role="alert">
          {blockedReason}
        </span>
      )}
      <div className="recording-bar__actions">
        <button
          type="button"
          onClick={onStart}
          disabled={!canStart(state) || blockedReason !== undefined}
          title={blockedReason}
        >
          Start Recording
        </button>
        <button type="button" onClick={onStop} disabled={!canStop(state)}>
          Stop
        </button>
      </div>
    </div>
  );
}

function StatusDot({ state }: { state: RecordingState }) {
  const live = state.status === 'recording';
  return (
    <span
      className={live ? 'dot dot--live' : 'dot'}
      role="img"
      aria-label={live ? 'Microphone live' : 'Microphone off'}
    />
  );
}

function TrackList({ tracks }: { tracks: TrackStatus[] }) {
  return (
    <ul className="recording-bar__tracks">
      {tracks.map((track) => (
        <li key={track.track} className={track.live ? 'track' : 'track track--stopped'}>
          {track.track === 'mic' ? 'Microphone' : 'System audio'}
          {track.error !== undefined && <span className="track__error"> — {track.error}</span>}
        </li>
      ))}
    </ul>
  );
}

function describe(state: RecordingState): string {
  switch (state.status) {
    case 'idle':
      return 'Not recording';
    case 'starting':
      return 'Starting…';
    case 'recording':
      return 'Recording';
    case 'stopping':
      return 'Stopping…';
    case 'processing':
      return processingLabel(state.stage);
    case 'failed':
      return `Failed: ${state.error}`;
  }
}

function processingLabel(stage: 'transcribing' | 'summarizing' | 'saving'): string {
  switch (stage) {
    case 'transcribing':
      return 'Transcribing…';
    case 'summarizing':
      return 'Writing notes…';
    case 'saving':
      return 'Saving…';
  }
}

/** Ticks once a second while recording, so the elapsed time stays honest. */
function useElapsed(state: RecordingState): number | undefined {
  const startedAt = state.status === 'recording' ? state.startedAt : undefined;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (startedAt === undefined) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [startedAt]);

  return startedAt === undefined ? undefined : now - startedAt;
}
