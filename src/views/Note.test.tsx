import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeetingNote } from '../ipc/meetings';
import Note from './Note';

const { readMeeting, openMeetingFolder } = vi.hoisted(() => ({
  readMeeting: vi.fn(),
  openMeetingFolder: vi.fn(),
}));

vi.mock('../ipc/meetings', () => ({
  readMeeting,
  openMeetingFolder,
}));

const baseNote: MeetingNote = {
  folder: '/notes/2026-08-19-standup',
  title: 'Team standup',
  createdAt: '2026-08-19T09:00:00.000Z',
  summary: '# Standup\n- Shipped the note view\n- Next: settings screen',
  transcript: [
    { start: 0, text: 'Good morning everyone.', track: 'mic' },
    { start: 8, text: "Let's get started.", track: 'system' },
  ],
  engine: 'local',
  model: 'whisper-base',
  audioKept: true,
};

beforeEach(() => {
  readMeeting.mockReset();
  openMeetingFolder.mockReset();
  openMeetingFolder.mockResolvedValue(undefined);
});

describe('Note', () => {
  it('shows a loading state before the note arrives', () => {
    readMeeting.mockReturnValue(new Promise(() => {}));
    render(<Note folder={baseNote.folder} />);
    expect(screen.getByRole('status')).toHaveTextContent('Loading');
  });

  it('renders the summary above the transcript, separated by a divider', async () => {
    readMeeting.mockResolvedValue(baseNote);
    const { container } = render(<Note folder={baseNote.folder} />);

    await screen.findByText('Team standup');
    expect(screen.getByRole('heading', { level: 1, name: 'Standup' })).toBeInTheDocument();
    expect(screen.getByText('Shipped the note view')).toBeInTheDocument();
    expect(container.querySelector('.note__divider')).toBeInTheDocument();
    expect(screen.getByText('Good morning everyone.')).toBeInTheDocument();
  });

  it('states the engine and model that produced the note', async () => {
    readMeeting.mockResolvedValue(baseNote);
    render(<Note folder={baseNote.folder} />);
    await screen.findByText('Team standup');
    expect(screen.getByText(/Local transcription/)).toBeInTheDocument();
    expect(screen.getByText(/whisper-base/)).toBeInTheDocument();
  });

  it('says when the audio has been deleted', async () => {
    readMeeting.mockResolvedValue({ ...baseNote, audioKept: false });
    render(<Note folder={baseNote.folder} />);
    await screen.findByText('Team standup');
    expect(screen.getByText('Audio has been deleted.')).toBeInTheDocument();
  });

  it('says when the audio is still on disk', async () => {
    readMeeting.mockResolvedValue(baseNote);
    render(<Note folder={baseNote.folder} />);
    await screen.findByText('Team standup');
    expect(screen.getByText('Audio is still on disk.')).toBeInTheDocument();
  });

  it('renders an empty transcript honestly', async () => {
    readMeeting.mockResolvedValue({ ...baseNote, transcript: [] });
    render(<Note folder={baseNote.folder} />);
    await screen.findByText('Team standup');
    expect(screen.getByText('No transcript for this meeting.')).toBeInTheDocument();
  });

  it('reports a load failure and offers a retry', async () => {
    readMeeting.mockRejectedValueOnce(new Error('folder not found'));
    render(<Note folder={baseNote.folder} />);

    expect(await screen.findByRole('alert')).toHaveTextContent('folder not found');

    readMeeting.mockResolvedValueOnce(baseNote);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await screen.findByText('Team standup');
  });

  it('opens the meeting folder on request', async () => {
    readMeeting.mockResolvedValue(baseNote);
    render(<Note folder={baseNote.folder} />);
    await screen.findByText('Team standup');

    await userEvent.click(screen.getByRole('button', { name: 'Open folder' }));
    await waitFor(() => expect(openMeetingFolder).toHaveBeenCalledWith(baseNote.folder));
  });
});
