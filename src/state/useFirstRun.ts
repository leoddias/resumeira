import { useCallback, useEffect, useState } from 'react';
import { listMeetings } from '../ipc/meetings';
import { listModels } from '../ipc/models';
import { getKeyStatus, getSettings } from '../ipc/settings';

export type FirstRunState =
  { status: 'loading' } | { status: 'hidden' } | { status: 'visible'; notesFolder: string | null };

/**
 * Whether to show the first-run onboarding screen.
 *
 * Shows only for a genuinely new install: no meetings recorded yet, no API
 * key stored, and no local model installed. Any one of those means the app
 * has already been used or set up, so a returning user must never see this
 * screen again — it is checked fresh on every mount rather than cached, so a
 * "skip" only dismisses the current session, never writes anything to disk.
 */
export function useFirstRun() {
  const [state, setState] = useState<FirstRunState>({ status: 'loading' });
  const [skipped, setSkipped] = useState(false);

  useEffect(() => {
    if (skipped) return;
    let active = true;

    Promise.all([listMeetings(), getKeyStatus(), listModels(), getSettings()])
      .then(([meetings, keys, models, settings]) => {
        if (!active) return;
        const hasMeetings = meetings.length > 0;
        const hasConfiguredKey = keys.some((key) => key.configured);
        const hasInstalledModel = models.some((model) => model.installed);
        if (hasMeetings || hasConfiguredKey || hasInstalledModel) {
          setState({ status: 'hidden' });
        } else {
          setState({ status: 'visible', notesFolder: settings.notesFolder });
        }
      })
      .catch(() => {
        // Any of the four calls can fail independently, and we cannot tell
        // "new install" from "disk temporarily unavailable" here. Staying
        // hidden is the safer default: a returning user never sees onboarding
        // again just because a lookup hiccuped, and a new user still reaches
        // the rest of the app instead of being stuck behind a broken screen.
        if (active) setState({ status: 'hidden' });
      });

    return () => {
      active = false;
    };
  }, [skipped]);

  const skip = useCallback(() => {
    setSkipped(true);
    setState({ status: 'hidden' });
  }, []);

  return { state, skip };
}
