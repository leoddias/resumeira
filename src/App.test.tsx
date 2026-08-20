import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

// The app talks to Rust on mount; in tests there is no Tauri host, so the
// IPC layer is stubbed. RecordingBar itself is tested directly.
vi.mock('./ipc/recording', () => ({
  startRecording: vi.fn(),
  stopRecording: vi.fn(),
  getRecordingState: vi.fn(() => Promise.resolve({ status: 'idle' })),
  onRecordingState: vi.fn(() => Promise.resolve(() => {})),
}));

describe('App', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the app name', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: 'Resumeira' })).toBeInTheDocument();
  });

  it('tells the user where recording starts', () => {
    render(<App />);
    expect(screen.getByText(/tray icon/i)).toBeInTheDocument();
  });

  it('shows the recording control', () => {
    render(<App />);
    expect(screen.getByRole('region', { name: 'Recording' })).toBeInTheDocument();
  });
});
