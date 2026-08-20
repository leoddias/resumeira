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

  it('renders a line with no track without breaking', () => {
    const lines: TranscriptLine[] = [{ start: 12, text: 'Not attributed' }];
    render(<TranscriptView lines={lines} />);
    expect(screen.getByText('Not attributed')).toBeInTheDocument();
    expect(screen.getByText('Unknown speaker')).toBeInTheDocument();
  });
});
