import { describe, expect, it, vi } from 'vitest';
import type { RecordingState } from '../ipc/types';

// `useRecording` in the same module talks to Rust on mount; only the pure
// preview reducer below is under test here.
vi.mock('../ipc/recording', () => ({
  startRecording: vi.fn(),
  stopRecording: vi.fn(),
  getRecordingState: vi.fn(() => Promise.resolve({ status: 'idle' })),
  getRecordingLevels: vi.fn(() => Promise.resolve([])),
  onRecordingState: vi.fn(() => Promise.resolve(() => {})),
}));

const { nextPreview } = await import('./useRecording');

function transcribing(line?: string): RecordingState {
  return {
    status: 'processing',
    stage: 'transcribing',
    startedAt: 0,
    transcribing: { track: 'mic', index: 1, total: 1, line },
  };
}

describe('nextPreview', () => {
  it('collects the lines as they arrive', () => {
    let lines = nextPreview([], transcribing('one'));
    lines = nextPreview(lines, transcribing('two'));
    expect(lines).toEqual(['one', 'two']);
  });

  it('keeps a repeated line rather than swallowing it', () => {
    // People do say the same short thing twice, and a preview that drops
    // the second one looks stuck.
    const lines = nextPreview(nextPreview([], transcribing('yeah')), transcribing('yeah'));
    expect(lines).toEqual(['yeah', 'yeah']);
  });

  it('keeps only the last few lines', () => {
    const lines = ['one', 'two', 'three', 'four'].reduce(
      (acc, line) => nextPreview(acc, transcribing(line)),
      [] as string[],
    );
    expect(lines).toEqual(['two', 'three', 'four']);
  });

  it('leaves the preview alone on an update that carries no line', () => {
    const lines = nextPreview(['one'], transcribing(undefined));
    expect(lines).toEqual(['one']);
  });

  it('drops everything once the step is over', () => {
    expect(nextPreview(['one', 'two'], { status: 'idle' })).toEqual([]);
    expect(
      nextPreview(['one'], { status: 'processing', stage: 'summarizing', startedAt: 0 }),
    ).toEqual([]);
  });

  it('does not churn an already empty preview', () => {
    const empty: string[] = [];
    expect(nextPreview(empty, { status: 'idle' })).toBe(empty);
  });
});
