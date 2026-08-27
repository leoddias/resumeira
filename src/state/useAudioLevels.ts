import { useEffect, useState } from 'react';
import { getRecordingLevels } from '../ipc/recording';
import { isCapturing, type RecordingState, type TrackLevel } from '../ipc/types';

/**
 * How often the meter asks Rust for fresh levels.
 *
 * Fast enough that a bar tracks a voice rather than lagging behind it, slow
 * enough that it stays a handful of tiny IPC calls a second. The decay in
 * `audio::level` covers the gaps between polls, so this does not have to
 * match the audio callback rate.
 */
const POLL_INTERVAL_MS = 100;

/**
 * Live per-track loudness while capturing, for the recording bar's meter.
 *
 * Polled rather than pushed: a state event per frame would drive the tray
 * and every listener too, to move a bar a few pixels. Polling also stops on
 * its own the moment capture ends, so nothing can leave a meter running.
 *
 * Returns an empty list whenever nothing is being captured, so a bar is
 * never left standing after the recording stopped.
 */
export function useAudioLevels(state: RecordingState): TrackLevel[] {
  const [levels, setLevels] = useState<TrackLevel[]>([]);
  const capturing = isCapturing(state);

  useEffect(() => {
    if (!capturing) {
      setLevels([]);
      return;
    }

    let active = true;
    const poll = () => {
      getRecordingLevels()
        .then((next) => {
          if (active) setLevels(next);
        })
        .catch(() => {
          // A dropped poll is one missed frame of a meter. The next tick
          // tries again, and the recording is unaffected either way.
        });
    };

    poll();
    const timer = setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [capturing]);

  return levels;
}
