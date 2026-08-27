import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { RecordingState, TrackLevel } from '../ipc/types';
import RecordingBar from './RecordingBar';

function renderBar(
  state: RecordingState,
  blockedReason?: string,
  levels?: TrackLevel[],
  preview?: string[],
) {
  const onStart = vi.fn();
  const onStop = vi.fn();
  render(
    <RecordingBar
      state={state}
      onStart={onStart}
      onStop={onStop}
      blockedReason={blockedReason}
      levels={levels}
      preview={preview}
    />,
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
    renderBar({ status: 'processing', stage: 'transcribing', startedAt: 0 });
    expect(screen.getByText('Transcribing…')).toBeInTheDocument();
  });

  it('names the speaker step rather than leaving a silent gap', () => {
    renderBar({ status: 'processing', stage: 'identifying', startedAt: 0 });
    expect(screen.getByText('Identifying speakers…')).toBeInTheDocument();
  });

  it('draws a level meter per live track', () => {
    renderBar(recording, undefined, [
      { track: 'mic', level: 0.75, receiving: true },
      { track: 'system', level: 0, receiving: true },
    ]);

    const meters = screen.getAllByRole('meter', { name: 'Audio level' });
    expect(meters).toHaveLength(2);
    expect(meters[0]).toHaveAttribute('aria-valuenow', '75');
    expect(meters[1]).toHaveAttribute('aria-valuenow', '0');
  });

  it('tells a device delivering silence apart from one delivering nothing', () => {
    renderBar(recording, undefined, [
      { track: 'mic', level: 0, receiving: true },
      { track: 'system', level: 0, receiving: false },
    ]);

    const meters = screen.getAllByRole('meter', { name: 'Audio level' });
    expect(meters[0]).not.toHaveAttribute('title');
    expect(meters[1]).toHaveAttribute('title', 'No audio from this device yet');
  });

  it('reads as an empty meter when no level has arrived yet', () => {
    renderBar(recording);
    const meters = screen.getAllByRole('meter', { name: 'Audio level' });
    expect(meters).toHaveLength(2);
    for (const meter of meters) {
      expect(meter).toHaveAttribute('aria-valuenow', '0');
      expect(meter).toHaveAttribute('title', 'No audio from this device yet');
    }
  });

  it('shows no meter at all when nothing is recording', () => {
    renderBar({ status: 'idle' }, undefined, [{ track: 'mic', level: 0.9, receiving: true }]);
    expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  });

  it('shows how far transcription has got, and on which track', () => {
    renderBar({
      status: 'processing',
      stage: 'transcribing',
      startedAt: Date.now(),
      transcribing: { track: 'system', index: 2, total: 2, percent: 47 },
    });

    const bar = screen.getByRole('progressbar', { name: 'Transcribing…' });
    expect(bar).toHaveAttribute('aria-valuenow', '47');
    expect(screen.getByText('System audio (2/2)')).toBeInTheDocument();
  });

  it('shows an indeterminate bar when the engine cannot say how far it is', () => {
    // A cloud request is one call: there is nothing honest to put on a
    // percentage between sending it and getting an answer.
    renderBar({
      status: 'processing',
      stage: 'transcribing',
      startedAt: Date.now(),
      transcribing: { track: 'mic', index: 1, total: 1 },
    });

    const bar = screen.getByRole('progressbar', { name: 'Transcribing…' });
    expect(bar).not.toHaveAttribute('aria-valuenow');
  });

  it('does not count tracks when the meeting only recorded one', () => {
    renderBar({
      status: 'processing',
      stage: 'transcribing',
      startedAt: Date.now(),
      transcribing: { track: 'mic', index: 1, total: 1, percent: 10 },
    });
    expect(screen.queryByText(/Microphone \(/)).not.toBeInTheDocument();
  });

  it('shows the last line heard while transcribing', () => {
    renderBar(
      { status: 'processing', stage: 'transcribing', startedAt: Date.now() },
      undefined,
      undefined,
      ['first line', 'second line'],
    );
    expect(screen.getByText('second line')).toBeInTheDocument();
  });

  it('keeps a bar on screen through the stages that cannot report progress', () => {
    renderBar({ status: 'processing', stage: 'summarizing', startedAt: Date.now() });
    expect(screen.getByRole('progressbar', { name: 'Writing notes…' })).toBeInTheDocument();
  });

  it('counts the wait after the meeting, not just during it', () => {
    renderBar({ status: 'processing', stage: 'transcribing', startedAt: Date.now() - 65_000 });
    expect(screen.getByText('01:05')).toBeInTheDocument();
  });
});
