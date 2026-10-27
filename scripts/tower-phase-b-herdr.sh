#!/usr/bin/env bash
# Lab-local Herdr command: every created pane receives the isolated workload env.
set -euo pipefail
self=${BASH_SOURCE[0]}
real=${self%/*}/herdr-real
lab_root=$(cd "${self%/*}/../.." && pwd -P)
if [ "$*" = 'plugin action invoke open --plugin pikenguyen227.zoetrope-fleet' ]; then
  HERDR_BIN_PATH=$self exec /bin/bash "$lab_root/runtime/fleet-plugin/pane.sh"
fi
if { { [ "${1:-}" = workspace ] || [ "${1:-}" = tab ]; } && [ "${2:-}" = create ]; } ||
  { [ "${1:-}" = plugin ] && [ "${2:-}" = pane ] && [ "${3:-}" = open ]; }; then
  [ -n "${CODEX_HOME:-}" ] || { echo 'Phase B: missing lab CODEX_HOME' >&2; exit 1; }
  exec "$real" "$@" --env "HOME=$HOME" --env "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" \
    --env "XDG_STATE_HOME=$XDG_STATE_HOME" --env "XDG_DATA_HOME=$XDG_DATA_HOME" \
    --env "TMPDIR=$TMPDIR" --env "CODEX_HOME=$CODEX_HOME" --env "PATH=$PATH" \
    --env "FM_HOME=$lab_root/firstmate" --env "ACCEPTANCE_LAB=$lab_root" \
    --env "HERDR_BIN_PATH=$self"
fi
exec "$real" "$@"
