import { describe, expect, it } from 'vitest';
import {
  canStart,
  canStop,
  INITIAL_RECORDING_STATE,
  isCapturing,
  type RecordingState,
} from './types';

const recording: RecordingState = {
  status: 'recording',
  startedAt: 0,
  tracks: [{ track: 'mic', deviceName: 'Test Mic', live: true }],
};

describe('recording state predicates', () => {
  it('starts idle', () => {
    expect(INITIAL_RECORDING_STATE).toEqual({ status: 'idle' });
  });

  it('reports capturing while starting, not only while recording', () => {
    expect(isCapturing({ status: 'starting' })).toBe(true);
    expect(isCapturing(recording)).toBe(true);
  });

  it('never reports capturing once the session is over', () => {
    const notCapturing: RecordingState[] = [
      { status: 'idle' },
      { status: 'stopping' },
      { status: 'processing', stage: 'transcribing', startedAt: 0 },
      { status: 'failed', error: 'no device' },
    ];
    for (const state of notCapturing) {
      expect(isCapturing(state)).toBe(false);
    }
  });

  it('allows starting only from a settled state', () => {
    expect(canStart({ status: 'idle' })).toBe(true);
    expect(canStart({ status: 'failed', error: 'boom' })).toBe(true);
    expect(canStart({ status: 'starting' })).toBe(false);
    expect(canStart(recording)).toBe(false);
    expect(canStart({ status: 'processing', stage: 'saving', startedAt: 0 })).toBe(false);
  });

  it('allows stopping only while recording', () => {
    expect(canStop(recording)).toBe(true);
    expect(canStop({ status: 'starting' })).toBe(false);
    expect(canStop({ status: 'idle' })).toBe(false);
  });

  it('never allows starting and stopping at the same time', () => {
    const states: RecordingState[] = [
      { status: 'idle' },
      { status: 'starting' },
      recording,
      { status: 'stopping' },
      { status: 'processing', stage: 'summarizing', startedAt: 0 },
      { status: 'failed', error: 'boom' },
    ];
    for (const state of states) {
      expect(canStart(state) && canStop(state)).toBe(false);
    }
  });
});
