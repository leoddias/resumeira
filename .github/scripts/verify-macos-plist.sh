#!/usr/bin/env bash
#
# Asks the built .app whether it carries the two usage strings macOS requires.
#
# These are not advisory. macOS terminates a process that touches the
# microphone or ScreenCaptureKit without a purpose string in its bundle, so a
# dropped `Info.plist` merge does not degrade recording — it kills the app the
# instant the user presses Record, having worked perfectly until then.
#
# The merge is implicit: the Tauri bundler picks up `src-tauri/Info.plist` by
# convention, and nothing in `tauri.conf.json` names the file. A bundler
# upgrade or a config reshuffle can drop it with every other check still
# green, which is why the artefact is asked rather than the source.
set -euo pipefail

app=$(find src-tauri/target/release/bundle -maxdepth 3 -name '*.app' -type d | head -1)
if [ -z "$app" ]; then
  echo "No .app bundle was found to check"
  exit 1
fi

plist="$app/Contents/Info.plist"
echo "Checking $plist"

status=0
for key in NSMicrophoneUsageDescription NSScreenCaptureUsageDescription; do
  if value=$(plutil -extract "$key" raw -o - "$plist" 2>/dev/null) && [ -n "$value" ]; then
    echo "  ok: $key"
  else
    echo "  FAIL: $key is missing from the bundle — the app would be killed by"
    echo "        macOS the moment it tried to record. Check that"
    echo "        src-tauri/Info.plist is still being merged by the bundler."
    status=1
  fi
done

# The minimum system version is the other half of the same contract:
# ScreenCaptureKit audio needs macOS 13, and a bundle claiming less would let
# the app install where its main feature cannot run (ADR-0024).
minimum=$(plutil -extract LSMinimumSystemVersion raw -o - "$plist" 2>/dev/null || echo "")
case "$minimum" in
  13.* | 14.* | 15.* | 1[6-9].* | [2-9][0-9].*)
    echo "  ok: LSMinimumSystemVersion is $minimum"
    ;;
  *)
    echo "  FAIL: LSMinimumSystemVersion is '$minimum', but ScreenCaptureKit"
    echo "        audio needs macOS 13.0 or later"
    status=1
    ;;
esac

exit "$status"
