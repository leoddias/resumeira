import { describe, expect, it } from 'vitest';
import { formatSize, MODEL_PROGRESS_EVENT } from './models';

describe('model helpers', () => {
  it('shows gigabyte sizes with one decimal', () => {
    expect(formatSize(1_600_000_000)).toBe('1.6 GB');
  });

  it('shows smaller models in megabytes', () => {
    expect(formatSize(77_691_713)).toBe('78 MB');
  });

  it('has something to show for an unknown size', () => {
    expect(formatSize(0)).toBe('—');
    expect(formatSize(-1)).toBe('—');
  });

  it('agrees with the Rust event name', () => {
    expect(MODEL_PROGRESS_EVENT).toBe('model-progress');
  });
});
