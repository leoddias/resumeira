#!/usr/bin/env bash
#
# Asks the built binary whether it actually contains the frontend that was
# just compiled.
#
# A binary built without `tauri/custom-protocol` loads `build.devUrl` at
# runtime instead of the embedded `dist/`. That build succeeds, passes every
# test, and works on any machine with `npm run dev` running — and shows
# "connection refused" on every other machine. There is no compile-time signal
# for it, so the artefact is inspected directly: the hashed asset filename Vite
# just produced must appear as a byte string inside the executable.
set -euo pipefail

asset=$(ls dist/assets/index-*.js | head -1 | xargs -n1 basename)
echo "Looking for /assets/$asset inside the built binaries"

found=0
for binary in \
  src-tauri/target/release/resumeira \
  src-tauri/target/release/resumeira.exe; do
  [ -f "$binary" ] || continue
  if grep -qF "/assets/$asset" "$binary"; then
    echo "  ok: $binary"
    found=1
  else
    dev_url=$(node -p "require('./src-tauri/tauri.conf.json').build.devUrl")
    echo "  FAIL: $binary does not embed the frontend — it would open $dev_url at runtime"
    exit 1
  fi
done

if [ "$found" -eq 0 ]; then
  echo "No binary was found to check"
  exit 1
fi
