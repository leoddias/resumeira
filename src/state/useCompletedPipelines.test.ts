import { renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { RecordingState } from '../ipc/types';

// `useRecording` in the same module talks to Rust on mount; only the pure
// counter below is under test here.
vi.mock('../ipc/recording', () => ({
  startRecording: vi.fn(),
  stopRecording: vi.fn(),
  getRecordingState: vi.fn(() => Promise.resolve({ status: 'idle' })),
  onRecordingState: vi.fn(() => Promise.resolve(() => {})),
}));

const { useCompletedPipelines } = await import('./useRecording');

const idle: RecordingState = { status: 'idle' };
const processing: RecordingState = { status: 'processing', stage: 'summarizing', startedAt: 0 };
const recording: RecordingState = { status: 'recording', startedAt: Date.now(), tracks: [] };

function track([first, ...rest]: [RecordingState, ...RecordingState[]]) {
  const { result, rerender } = renderHook(({ state }) => useCompletedPipelines(state), {
    initialProps: { state: first },
  });
  for (const state of rest) rerender({ state });
  return result;
}

describe('useCompletedPipelines', () => {
  it('counts the moment a meeting becomes a note', () => {
    // Nothing the user did marks this moment — the pipeline runs in the
    // background for minutes — so leaving `processing` is the only signal
    // that a new note exists.
    expect(track([idle, processing, idle]).current).toBe(1);
  });

  it('counts a failed pipeline too', () => {
    // A failure leaves no new note, but re-asking costs one query, while
    // missing a real note leaves the list saying "you have no meetings".
    const failed: RecordingState = { status: 'failed', error: 'no key' };
    expect(track([processing, failed]).current).toBe(1);
  });

  it('does not count a recording that has not finished processing', () => {
    expect(track([idle, recording, processing]).current).toBe(0);
  });

  it('counts each meeting separately', () => {
    expect(track([idle, processing, idle, recording, processing, idle]).current).toBe(2);
  });

  it('does not count the stages within one pipeline', () => {
    const transcribing: RecordingState = {
      status: 'processing',
      stage: 'transcribing',
      startedAt: 0,
    };
    const saving: RecordingState = { status: 'processing', stage: 'saving', startedAt: 0 };
    expect(track([transcribing, processing, saving, idle]).current).toBe(1);
  });
});
