#!/usr/bin/env bash
# Lab-local Herdr command: every created pane receives the isolated workload env.
set -euo pipefail
real=${BASH_SOURCE[0]%/*}/herdr-real
if { [ "${1:-}" = workspace ] || [ "${1:-}" = tab ]; } && [ "${2:-}" = create ]; then
  [ -n "${CODEX_HOME:-}" ] || { echo 'Phase B: missing lab CODEX_HOME' >&2; exit 1; }
  lab_root=$(cd "${BASH_SOURCE[0]%/*}/../.." && pwd -P)
  exec "$real" "$@" --env "HOME=$HOME" --env "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" \
    --env "XDG_STATE_HOME=$XDG_STATE_HOME" --env "XDG_DATA_HOME=$XDG_DATA_HOME" \
    --env "TMPDIR=$TMPDIR" --env "CODEX_HOME=$CODEX_HOME" --env "PATH=$PATH" \
    --env "FM_HOME=$lab_root/firstmate" --env "ACCEPTANCE_LAB=$lab_root" \
    --env "HERDR_BIN_PATH=${BASH_SOURCE[0]}"
fi
exec "$real" "$@"
