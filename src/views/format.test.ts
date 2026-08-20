import { describe, expect, it } from 'vitest';
import { formatElapsed } from './format';

describe('formatElapsed', () => {
  it('shows minutes and seconds for a short meeting', () => {
    expect(formatElapsed(0)).toBe('00:00');
    expect(formatElapsed(9_000)).toBe('00:09');
    expect(formatElapsed(65_000)).toBe('01:05');
  });

  it('adds an hour component only once there is one', () => {
    expect(formatElapsed(59 * 60 * 1000 + 59_000)).toBe('59:59');
    expect(formatElapsed(60 * 60 * 1000)).toBe('1:00:00');
    expect(formatElapsed(2 * 60 * 60 * 1000 + 3 * 60 * 1000 + 4_000)).toBe('2:03:04');
  });

  it('treats a clock that went backwards as zero', () => {
    expect(formatElapsed(-5_000)).toBe('00:00');
  });

  it('truncates rather than rounding up a partial second', () => {
    expect(formatElapsed(1_999)).toBe('00:01');
  });
});
