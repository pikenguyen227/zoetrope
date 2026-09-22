#!/usr/bin/env bash
set -euo pipefail
if [ "${HERDR_ENV:-}" != 1 ]; then
  echo "Open Fleet from a genuine Herdr pane." >&2
  exit 1
fi
# Tab placement must not pass --target-pane (Herdr 0.9.1 rejects that pairing).
exec "${HERDR_BIN_PATH:-herdr}" plugin pane open \
  --plugin pikenguyen227.zoetrope-fleet --entrypoint fleet --placement tab
