import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { checkReadiness } from './ipc/readiness';
import { getSettings, type Settings } from './ipc/settings';

const SETTINGS: Settings = {
  notesFolder: null,
  transcription: { engine: 'local', provider: 'groq', localModel: 'large-v3-turbo' },
  summaryEngine: 'api',
  summaryProvider: 'anthropic',
  summaryCli: 'claude',
  summaryModel: null,
  audioRetention: 'keep',
  telemetryOptIn: false,
};

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

const READY = {
  transcription: { ready: true, detail: 'Local · large-v3-turbo', leavesTheMachine: false },
  summary: { ready: true, detail: 'anthropic · claude-sonnet-5', leavesTheMachine: true },
  canRecord: true,
};

vi.mock('./ipc/readiness', () => ({
  checkReadiness: vi.fn(() => Promise.resolve(READY)),
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

  it('holds a half-configured app on the setup screen and refuses to record', async () => {
    // The regression this whole change exists for: recording used to start
    // happily and only fail once the meeting was over and unrepeatable.
    vi.mocked(getSettings).mockResolvedValue(SETTINGS);
    vi.mocked(checkReadiness).mockResolvedValueOnce({
      transcription: {
        ready: false,
        detail: 'Local · large-v3-turbo',
        leavesTheMachine: false,
        blocker: { kind: 'modelMissing', model: 'large-v3-turbo' },
      },
      summary: { ready: true, detail: 'anthropic · claude-sonnet-5', leavesTheMachine: true },
      canRecord: false,
    });

    render(<App />);

    expect(
      await screen.findByRole('dialog', { name: 'Finish setting up Resumeira' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start Recording' })).toBeDisabled();
    expect(screen.queryByRole('region', { name: 'Meetings' })).not.toBeInTheDocument();
  });

  it('does not show the setup screen once the pipeline can run', async () => {
    render(<App />);
    await screen.findByRole('button', { name: 'Start Recording' });
    expect(screen.queryByRole('dialog', { name: 'Finish setting up Resumeira' })).toBeNull();
  });

  it('keeps the recording bar visible while on settings', async () => {
    render(<App />);
    await userEvent.click(screen.getByRole('button', { name: 'Settings' }));
    // A user must never lose sight of whether a microphone is live.
    expect(screen.getByRole('region', { name: 'Recording' })).toBeInTheDocument();
  });
});
