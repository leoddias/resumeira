/** Formatting helpers for the recording UI. */

/**
 * Elapsed time as `MM:SS`, or `H:MM:SS` once a meeting passes an hour.
 *
 * Meetings routinely run long, so the hour component appears only when it
 * exists rather than padding every short meeting with a leading `00:`.
 */
export function formatElapsed(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1000));
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);

  const mm = String(minutes).padStart(2, '0');
  const ss = String(seconds).padStart(2, '0');

  return hours > 0 ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`;
}
