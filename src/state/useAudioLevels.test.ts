import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RecordingState, TrackLevel } from '../ipc/types';

const { getRecordingLevels } = vi.hoisted(() => ({ getRecordingLevels: vi.fn() }));

vi.mock('../ipc/recording', () => ({ getRecordingLevels }));

// Imported after the mock so the hook picks up the mocked module.
const { useAudioLevels } = await import('./useAudioLevels');

const recording: RecordingState = {
  status: 'recording',
  startedAt: Date.now(),
  tracks: [{ track: 'mic', deviceName: 'Test Mic', live: true }],
};

const levels: TrackLevel[] = [{ track: 'mic', level: 0.6, receiving: true }];

beforeEach(() => {
  getRecordingLevels.mockReset();
  getRecordingLevels.mockResolvedValue(levels);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useAudioLevels', () => {
  it('reads the levels while capturing', async () => {
    const { result } = renderHook(() => useAudioLevels(recording));
    await waitFor(() => expect(result.current).toEqual(levels));
  });

  it('asks for nothing while idle', () => {
    const { result } = renderHook(() => useAudioLevels({ status: 'idle' }));
    expect(result.current).toEqual([]);
    expect(getRecordingLevels).not.toHaveBeenCalled();
  });

  it('empties the meter when capture ends, rather than freezing the last bar', async () => {
    const { result, rerender } = renderHook((state: RecordingState) => useAudioLevels(state), {
      initialProps: recording as RecordingState,
    });
    await waitFor(() => expect(result.current).toEqual(levels));

    rerender({ status: 'processing', stage: 'transcribing', startedAt: 0 });
    expect(result.current).toEqual([]);
  });

  it('keeps polling after a failed read', async () => {
    vi.useFakeTimers();
    getRecordingLevels.mockRejectedValueOnce(new Error('busy'));

    renderHook(() => useAudioLevels(recording));
    await vi.advanceTimersByTimeAsync(250);

    expect(getRecordingLevels.mock.calls.length).toBeGreaterThan(1);
  });

  it('stops polling once unmounted', async () => {
    vi.useFakeTimers();
    const { unmount } = renderHook(() => useAudioLevels(recording));
    await vi.advanceTimersByTimeAsync(150);
    const before = getRecordingLevels.mock.calls.length;

    unmount();
    await vi.advanceTimersByTimeAsync(500);

    expect(getRecordingLevels.mock.calls.length).toBe(before);
  });
});
