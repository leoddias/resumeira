import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeetingListItem } from '../ipc/meetings';

const { listMeetings, searchMeetings } = vi.hoisted(() => ({
  listMeetings: vi.fn(),
  searchMeetings: vi.fn(),
}));

vi.mock('../ipc/meetings', () => ({ listMeetings, searchMeetings }));

// Imported after the mock so the component picks up the mocked module.
const { default: Meetings } = await import('./Meetings');

const items: MeetingListItem[] = [
  {
    folder: '/meetings/standup',
    title: 'Daily Standup',
    createdAt: '2026-01-05T09:00:00',
    preview: 'Discussed the release plan.',
  },
  {
    folder: '/meetings/1-1',
    title: 'One on one',
    createdAt: '2026-01-04T15:00:00',
    preview: 'Career growth chat.',
  },
];

beforeEach(() => {
  listMeetings.mockReset();
  searchMeetings.mockReset();
});

describe('Meetings', () => {
  it('shows a loading state before the list arrives', () => {
    listMeetings.mockReturnValue(new Promise(() => {}));
    render(<Meetings onOpen={vi.fn()} />);
    expect(screen.getByRole('status')).toHaveTextContent('Loading meetings…');
  });

  it('lists every meeting, title date and preview, and opens the one picked', async () => {
    listMeetings.mockResolvedValue(items);
    const onOpen = vi.fn();
    render(<Meetings onOpen={onOpen} />);

    await waitFor(() => expect(screen.getByText('Daily Standup')).toBeInTheDocument());
    expect(screen.getByText('One on one')).toBeInTheDocument();
    expect(screen.getByText('Discussed the release plan.')).toBeInTheDocument();

    await userEvent.click(screen.getByText('Daily Standup'));
    expect(onOpen).toHaveBeenCalledWith('/meetings/standup');
  });

  it('says plainly when there are no meetings, instead of an empty list', async () => {
    listMeetings.mockResolvedValue([]);
    render(<Meetings onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.getByText('No meetings yet.')).toBeInTheDocument());
  });

  it('reports a failed load and offers a retry, never a blank list', async () => {
    listMeetings.mockRejectedValue(new Error('disk unreadable'));
    render(<Meetings onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByRole('alert')).toBeInTheDocument());
    expect(screen.getByText(/disk unreadable/)).toBeInTheDocument();
    expect(screen.queryByText('No meetings yet.')).not.toBeInTheDocument();

    listMeetings.mockResolvedValue(items);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.getByText('Daily Standup')).toBeInTheDocument());
  });

  it('filters via search and returns to the full list once cleared', async () => {
    listMeetings.mockResolvedValue(items);
    searchMeetings.mockResolvedValue([items[0]]);
    render(<Meetings onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.getByText('One on one')).toBeInTheDocument());

    await userEvent.type(screen.getByRole('searchbox', { name: 'Search meetings' }), 'standup');

    await waitFor(() => expect(searchMeetings).toHaveBeenCalledWith('standup'));
    await waitFor(() => expect(screen.queryByText('One on one')).not.toBeInTheDocument());
    expect(screen.getByText('Daily Standup')).toBeInTheDocument();

    await userEvent.clear(screen.getByRole('searchbox', { name: 'Search meetings' }));
    await waitFor(() => expect(screen.getByText('One on one')).toBeInTheDocument());
  });
});
