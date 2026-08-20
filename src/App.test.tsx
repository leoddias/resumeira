import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

// The app talks to Rust on mount; in tests there is no Tauri host, so every
// IPC module is stubbed. Each view is tested directly in its own file.
vi.mock('./ipc/recording', () => ({
  startRecording: vi.fn(),
  stopRecording: vi.fn(),
  getRecordingState: vi.fn(() => Promise.resolve({ status: 'idle' })),
  onRecordingState: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock('./ipc/meetings', () => ({
  listMeetings: vi.fn(() => Promise.resolve([])),
  searchMeetings: vi.fn(() => Promise.resolve([])),
  readMeeting: vi.fn(() => Promise.resolve(null)),
  openMeetingFolder: vi.fn(),
  rebuildIndex: vi.fn(),
}));

vi.mock('./ipc/settings', async () => {
  const actual = await vi.importActual<typeof import('./ipc/settings')>('./ipc/settings');
  return {
    ...actual,
    getSettings: vi.fn(() => new Promise(() => {})),
    saveSettings: vi.fn(),
    getKeyStatus: vi.fn(() => Promise.resolve([])),
    setApiKey: vi.fn(),
    deleteApiKey: vi.fn(),
  };
});

describe('App', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the app name', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: 'Resumeira' })).toBeInTheDocument();
  });

  it('shows the recording control on every screen', () => {
    render(<App />);
    expect(screen.getByRole('region', { name: 'Recording' })).toBeInTheDocument();
  });

  it('opens on the meetings list', () => {
    render(<App />);
    expect(screen.getByRole('button', { name: 'Meetings' })).toHaveAttribute(
      'aria-current',
      'page',
    );
  });

  it('switches to settings and back', async () => {
    render(<App />);

    await userEvent.click(screen.getByRole('button', { name: 'Settings' }));
    expect(screen.getByRole('button', { name: 'Settings' })).toHaveAttribute(
      'aria-current',
      'page',
    );

    await userEvent.click(screen.getByRole('button', { name: 'Meetings' }));
    expect(screen.getByRole('button', { name: 'Meetings' })).toHaveAttribute(
      'aria-current',
      'page',
    );
  });

  it('keeps the recording bar visible while on settings', async () => {
    render(<App />);
    await userEvent.click(screen.getByRole('button', { name: 'Settings' }));
    // A user must never lose sight of whether a microphone is live.
    expect(screen.getByRole('region', { name: 'Recording' })).toBeInTheDocument();
  });
});
