import { renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { checkReadiness, type Readiness } from '../ipc/readiness';
import { isBlocked, useReadiness } from './useReadiness';

vi.mock('../ipc/readiness', () => ({
  checkReadiness: vi.fn(),
}));

function answer(canRecord: boolean): Readiness {
  return {
    transcription: { ready: canRecord, detail: 'Local · large-v3-turbo', leavesTheMachine: false },
    summary: { ready: true, detail: 'anthropic · claude-sonnet-5', leavesTheMachine: true },
    canRecord,
  };
}

describe('useReadiness', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('asks Rust on mount rather than deriving the answer from settings', async () => {
    vi.mocked(checkReadiness).mockResolvedValue(answer(true));
    const { result } = renderHook(() => useReadiness());

    await waitFor(() => {
      expect(result.current.state.status).toBe('ready');
    });
    expect(checkReadiness).toHaveBeenCalledTimes(1);
  });

  it('re-checks on demand, so fixing the cause does not need a restart', async () => {
    vi.mocked(checkReadiness).mockResolvedValueOnce(answer(false));
    const { result } = renderHook(() => useReadiness());

    await waitFor(() => {
      expect(isBlocked(result.current.state)).toBe(true);
    });

    vi.mocked(checkReadiness).mockResolvedValueOnce(answer(true));
    await result.current.recheck();

    await waitFor(() => {
      expect(isBlocked(result.current.state)).toBe(false);
    });
  });

  it('does not lock the window when the check itself fails', async () => {
    // Rust refuses the recording regardless, so an IPC hiccup should cost an
    // explanation, not access to the meetings the user already has.
    vi.mocked(checkReadiness).mockRejectedValue(new Error('ipc unavailable'));
    const { result } = renderHook(() => useReadiness());

    await waitFor(() => {
      expect(result.current.state.status).toBe('failed');
    });
    expect(isBlocked(result.current.state)).toBe(false);
  });

  it('does not block while the answer is still unknown', () => {
    vi.mocked(checkReadiness).mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => useReadiness());
    expect(isBlocked(result.current.state)).toBe(false);
  });
});
