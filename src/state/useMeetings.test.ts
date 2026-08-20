import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeetingListItem } from '../ipc/meetings';

const { listMeetings, searchMeetings } = vi.hoisted(() => ({
  listMeetings: vi.fn(),
  searchMeetings: vi.fn(),
}));

vi.mock('../ipc/meetings', () => ({ listMeetings, searchMeetings }));

// Imported after the mock so the hook picks up the mocked module.
const { useMeetings } = await import('./useMeetings');

const items: MeetingListItem[] = [
  { folder: '/a', title: 'Standup', createdAt: '2026-01-01T09:00:00', preview: 'Talked about X' },
];

beforeEach(() => {
  listMeetings.mockReset();
  searchMeetings.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useMeetings', () => {
  it('loads the full list on mount', async () => {
    listMeetings.mockResolvedValue(items);
    const { result } = renderHook(() => useMeetings());

    expect(result.current.state.status).toBe('loading');
    await waitFor(() => expect(result.current.state.status).toBe('loaded'));
    expect(result.current.state).toEqual({ status: 'loaded', items });
    expect(searchMeetings).not.toHaveBeenCalled();
  });

  it('reports a failed load as an error, not an empty list', async () => {
    listMeetings.mockRejectedValue(new Error('index unreadable'));
    const { result } = renderHook(() => useMeetings());

    await waitFor(() => expect(result.current.state.status).toBe('error'));
    expect(result.current.state).toEqual({ status: 'error', error: 'index unreadable' });
  });

  it('debounces search so it does not query per keystroke', async () => {
    vi.useFakeTimers();
    listMeetings.mockResolvedValue([]);
    searchMeetings.mockResolvedValue(items);
    const { result } = renderHook(() => useMeetings());

    await act(async () => {
      await Promise.resolve();
    });

    act(() => result.current.setQuery('s'));
    act(() => result.current.setQuery('st'));
    act(() => result.current.setQuery('sta'));

    expect(searchMeetings).not.toHaveBeenCalled();

    await act(async () => {
      vi.advanceTimersByTime(300);
      await Promise.resolve();
    });

    expect(searchMeetings).toHaveBeenCalledTimes(1);
    expect(searchMeetings).toHaveBeenCalledWith('sta');
  });

  it('returns to the full list once the search box is cleared', async () => {
    vi.useFakeTimers();
    listMeetings.mockResolvedValue(items);
    searchMeetings.mockResolvedValue([]);
    const { result } = renderHook(() => useMeetings());

    await act(async () => {
      await Promise.resolve();
    });

    act(() => result.current.setQuery('sta'));
    await act(async () => {
      vi.advanceTimersByTime(300);
      await Promise.resolve();
    });
    expect(result.current.state).toEqual({ status: 'loaded', items: [] });

    act(() => result.current.setQuery(''));
    await act(async () => {
      await Promise.resolve();
    });
    expect(result.current.state).toEqual({ status: 'loaded', items });
  });

  it('retries the same query on demand', async () => {
    listMeetings.mockRejectedValueOnce(new Error('offline'));
    const { result } = renderHook(() => useMeetings());
    await waitFor(() => expect(result.current.state.status).toBe('error'));

    listMeetings.mockResolvedValue(items);
    act(() => result.current.retry());
    await waitFor(() => expect(result.current.state).toEqual({ status: 'loaded', items }));
  });
});
