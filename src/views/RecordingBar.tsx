import { useEffect, useState } from 'react';
import {
  canStart,
  canStop,
  type ProcessingStage,
  type RecordingState,
  type TrackLevel,
  type TrackStatus,
  type TranscribeProgress,
} from '../ipc/types';
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
  /**
   * Live loudness per track. Drives the activity indicator, which is the
   * only thing on screen that can tell a live microphone from a muted one
   * while there is still time to fix it.
   */
  levels?: TrackLevel[];
  /**
   * The last few lines transcription has produced. Turning an hour of audio
   * into a note takes minutes, and a label that never changes is
   * indistinguishable from a hang.
   */
  preview?: string[];
}

/**
 * The always-visible recording control. Presentational on purpose: state
 * comes from Rust via `useRecording`, so this renders without any IPC and
 * can be tested directly.
 */
export default function RecordingBar({
  state,
  onStart,
  onStop,
  blockedReason,
  levels = [],
  preview = [],
}: Props) {
  const elapsed = useElapsed(state);

  return (
    <div className="recording-bar" role="region" aria-label="Recording">
      <StatusDot state={state} levels={levels} />
      <span className="recording-bar__label">{describe(state)}</span>
      {elapsed !== undefined && (
        <span className="recording-bar__elapsed">{formatElapsed(elapsed)}</span>
      )}
      {state.status === 'recording' && <TrackList tracks={state.tracks} levels={levels} />}
      {state.status === 'processing' && (
        <Progress stage={state.stage} transcribing={state.transcribing} preview={preview} />
      )}
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

function StatusDot({ state, levels }: { state: RecordingState; levels: TrackLevel[] }) {
  const live = state.status === 'recording';
  const loudest = live ? peakOf(levels) : 0;

  return (
    <span
      className={live ? 'dot dot--live' : 'dot'}
      role="img"
      aria-label={live ? 'Microphone live' : 'Microphone off'}
    >
      {live && (
        // A halo that grows with the loudest track, so the indicator moves
        // with the room instead of blinking on a timer. Decorative: the
        // numbers themselves are on the per-track meters below.
        <span className="dot__halo" style={{ transform: `scale(${1 + loudest * 1.4})` }} />
      )}
    </span>
  );
}

/** The loudest track right now, in 0..1. */
function peakOf(levels: TrackLevel[]): number {
  return levels.reduce((loudest, entry) => Math.max(loudest, entry.level), 0);
}

function TrackList({ tracks, levels }: { tracks: TrackStatus[]; levels: TrackLevel[] }) {
  return (
    <ul className="recording-bar__tracks">
      {tracks.map((track) => (
        <li key={track.track} className={track.live ? 'track' : 'track track--stopped'}>
          <span className="track__name">{trackName(track.track)}</span>
          {track.live && <Meter level={levels.find((l) => l.track === track.track)} />}
          {track.error !== undefined && <span className="track__error"> — {track.error}</span>}
        </li>
      ))}
    </ul>
  );
}

/**
 * One track's loudness as a bar.
 *
 * A live device delivering silence is drawn as an empty bar; a device that
 * has not delivered anything at all says so, because "silent" and "not
 * there" are the two cases a user actually needs to tell apart.
 */
function Meter({ level }: { level?: TrackLevel }) {
  const height = level?.level ?? 0;
  const waiting = level === undefined || !level.receiving;

  return (
    <span
      className={waiting ? 'meter meter--waiting' : 'meter'}
      role="meter"
      aria-label="Audio level"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(height * 100)}
      title={waiting ? 'No audio from this device yet' : undefined}
    >
      <span className="meter__fill" style={{ width: `${Math.round(height * 100)}%` }} />
    </span>
  );
}

function trackName(track: TrackStatus['track']): string {
  return track === 'mic' ? 'Microphone' : 'System audio';
}

/**
 * What the pipeline is doing, in more than one word.
 *
 * A determinate bar when the engine can say how far it is, an indeterminate
 * one when it cannot — a cloud request is one call with nothing to report
 * between sending and receiving, and a bar crawling on a timer would be an
 * invention.
 */
function Progress({
  stage,
  transcribing,
  preview,
}: {
  stage: ProcessingStage;
  transcribing?: TranscribeProgress;
  preview: string[];
}) {
  const percent = transcribing?.percent;

  return (
    <div className="progress">
      {transcribing !== undefined && transcribing.total > 1 && (
        <span className="progress__track">
          {trackName(transcribing.track)} ({transcribing.index}/{transcribing.total})
        </span>
      )}
      <span
        className={percent === undefined ? 'progress__bar progress__bar--waiting' : 'progress__bar'}
        role="progressbar"
        aria-label={processingLabel(stage)}
        {...(percent === undefined
          ? {}
          : { 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': percent })}
      >
        <span className="progress__fill" style={{ width: `${percent ?? 100}%` }} />
      </span>
      {preview.length > 0 && (
        <p className="progress__preview" aria-label="Transcript preview" aria-live="polite">
          {preview[preview.length - 1]}
        </p>
      )}
    </div>
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

function processingLabel(stage: ProcessingStage): string {
  switch (stage) {
    case 'transcribing':
      return 'Transcribing…';
    case 'identifying':
      return 'Identifying speakers…';
    case 'summarizing':
      return 'Writing notes…';
    case 'saving':
      return 'Saving…';
  }
}

/**
 * Ticks once a second while recording *or* processing, so the elapsed time
 * stays honest. The wait after the meeting is the longer of the two, and the
 * one where a user starts wondering whether anything is happening at all.
 */
function useElapsed(state: RecordingState): number | undefined {
  const startedAt =
    state.status === 'recording' || state.status === 'processing' ? state.startedAt : undefined;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (startedAt === undefined) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [startedAt]);

  return startedAt === undefined ? undefined : now - startedAt;
}
