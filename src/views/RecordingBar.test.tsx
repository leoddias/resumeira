import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { RecordingState } from '../ipc/types';
import RecordingBar from './RecordingBar';

function renderBar(state: RecordingState, blockedReason?: string) {
  const onStart = vi.fn();
  const onStop = vi.fn();
  render(
    <RecordingBar state={state} onStart={onStart} onStop={onStop} blockedReason={blockedReason} />,
  );
  return { onStart, onStop };
}

const recording: RecordingState = {
  status: 'recording',
  startedAt: Date.now(),
  tracks: [
    { track: 'mic', deviceName: 'Test Mic', live: true },
    { track: 'system', deviceName: 'Speakers', live: true },
  ],
};

describe('RecordingBar', () => {
  it('says plainly when nothing is being recorded', () => {
    renderBar({ status: 'idle' });
    expect(screen.getByText('Not recording')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Microphone off' })).toBeInTheDocument();
  });

  it('marks the microphone as live while recording', () => {
    renderBar(recording);
    expect(screen.getByRole('img', { name: 'Microphone live' })).toBeInTheDocument();
    expect(screen.getByText('Recording')).toBeInTheDocument();
  });

  it('lists both tracks while recording', () => {
    renderBar(recording);
    expect(screen.getByText('Microphone')).toBeInTheDocument();
    expect(screen.getByText('System audio')).toBeInTheDocument();
  });

  it('shows why a track dropped without hiding the other', () => {
    renderBar({
      status: 'recording',
      startedAt: Date.now(),
      tracks: [
        { track: 'mic', deviceName: 'Test Mic', live: true },
        { track: 'system', deviceName: 'Speakers', live: false, error: 'device lost' },
      ],
    });
    expect(screen.getByText('Microphone')).toBeInTheDocument();
    expect(screen.getByText(/device lost/)).toBeInTheDocument();
  });

  it('only offers Start when a recording can actually start', async () => {
    const { onStart } = renderBar({ status: 'idle' });
    await userEvent.click(screen.getByRole('button', { name: 'Start Recording' }));
    expect(onStart).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
  });

  it('only offers Stop while recording', async () => {
    const { onStop } = renderBar(recording);
    expect(screen.getByRole('button', { name: 'Start Recording' })).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(onStop).toHaveBeenCalledOnce();
  });

  it('reports a failure instead of silently going idle', () => {
    renderBar({ status: 'failed', error: 'no input device' });
    expect(screen.getByText(/no input device/)).toBeInTheDocument();
  });

  it('refuses to start, with the reason visible, while setup is unfinished', async () => {
    const { onStart } = renderBar({ status: 'idle' }, 'Finish setup before recording');

    const start = screen.getByRole('button', { name: 'Start Recording' });
    expect(start).toBeDisabled();
    await userEvent.click(start);
    expect(onStart).not.toHaveBeenCalled();
    expect(screen.getByText('Finish setup before recording')).toBeInTheDocument();
  });

  it('says what it is doing after the meeting ends', () => {
    renderBar({ status: 'processing', stage: 'transcribing' });
    expect(screen.getByText('Transcribing…')).toBeInTheDocument();
  });
});
