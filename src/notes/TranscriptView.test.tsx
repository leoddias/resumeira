import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { TranscriptLine } from '../ipc/meetings';
import TranscriptView from './TranscriptView';

describe('TranscriptView', () => {
  it('says plainly when there is nothing to show', () => {
    render(<TranscriptView lines={[]} />);
    expect(screen.getByText('No transcript for this meeting.')).toBeInTheDocument();
  });

  it('shows the timestamp and the speaker for a mic line', () => {
    const lines: TranscriptLine[] = [{ start: 65, text: 'Hello there', track: 'mic' }];
    render(<TranscriptView lines={lines} />);
    expect(screen.getByText('01:05')).toBeInTheDocument();
    expect(screen.getByText('You')).toBeInTheDocument();
    expect(screen.getByText('Hello there')).toBeInTheDocument();
  });

  it('distinguishes system audio from the microphone', () => {
    const lines: TranscriptLine[] = [
      { start: 0, text: 'From me', track: 'mic' },
      { start: 5, text: 'From them', track: 'system' },
    ];
    render(<TranscriptView lines={lines} />);
    expect(screen.getByText('You')).toBeInTheDocument();
    expect(screen.getByText('Others')).toBeInTheDocument();
  });

  it('prefers the identified speaker over the track it arrived on', () => {
    const lines: TranscriptLine[] = [
      { start: 0, text: 'Morning', track: 'mic', speaker: 'Leo' },
      { start: 5, text: 'Morning Leo', track: 'system', speaker: 'Ana' },
    ];
    render(<TranscriptView lines={lines} />);
    expect(screen.getByText('Leo')).toBeInTheDocument();
    expect(screen.getByText('Ana')).toBeInTheDocument();
    expect(screen.queryByText('You')).not.toBeInTheDocument();
  });

  it('gives one person one colour, however often they speak', () => {
    const lines: TranscriptLine[] = [
      { start: 0, text: 'first', track: 'system', speaker: 'Ana' },
      { start: 5, text: 'second', track: 'system', speaker: 'Bruno' },
      { start: 9, text: 'third', track: 'system', speaker: 'Ana' },
    ];
    render(<TranscriptView lines={lines} />);

    const slot = (text: string) =>
      [...(screen.getByText(text).closest('li')?.classList ?? [])].find((name) =>
        name.startsWith('transcript__line--speaker-'),
      );

    expect(slot('first')).toBe(slot('third'));
    expect(slot('first')).not.toBe(slot('second'));
  });

  it('falls back to the track when the speaker step named nobody', () => {
    const lines: TranscriptLine[] = [{ start: 0, text: 'From them', track: 'system' }];
    render(<TranscriptView lines={lines} />);
    expect(screen.getByText('Others')).toBeInTheDocument();
  });

  it('renders a line with no track without breaking', () => {
    const lines: TranscriptLine[] = [{ start: 12, text: 'Not attributed' }];
    render(<TranscriptView lines={lines} />);
    expect(screen.getByText('Not attributed')).toBeInTheDocument();
    expect(screen.getByText('Unknown speaker')).toBeInTheDocument();
  });
});
