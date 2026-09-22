#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
agentic_root=$(dirname -- "$repo")
if [ "${HERDR_ENV:-}" != 1 ]; then
  echo "Open Fleet from a genuine Herdr pane." >&2
  exit 1
fi
# The pane title and tab label are separate in Herdr. Rename this newly
# created tab, never the focused Captain tab from the invocation context.
if [ -n "${HERDR_TAB_ID:-}" ]; then
  "${HERDR_BIN_PATH:-herdr}" tab rename "$HERDR_TAB_ID" Team >/dev/null ||
    printf 'Fleet: could not rename its tab to Team.\n' >&2
fi
# Configuration stays outside the repository; no user's session IDs ship here.
config="${ZOE_FLEET_STATE_DIR:-$agentic_root/.tools/state/zoe-fleet}"
mkdir -p "$config"
export ZOE_TELEMETRY_DIR="${ZOE_TELEMETRY_DIR:-$agentic_root/.tools/state/zoe-telemetry}"
firstmate_home="${FM_HOME:-$agentic_root/firstmate}"
binary="${ZOE_FLEET_BIN:-$agentic_root/.tools/bin/zoe-fleet}"
if [ ! -x "$binary" ]; then
  echo "Build the Fleet binary first: bash scripts/install-fleet.sh" >&2
  read -r -p "Press Enter to close " || true
  exit 1
fi
captain=()
context="${HERDR_PLUGIN_CONTEXT_JSON:-}"
[ -n "$context" ] || context='{}'
focused=$(printf '%s' "$context" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("focused_pane_id", ""))')
if [ -n "$focused" ]; then
  captain=(--captain "$focused")
fi
# Herdr runs this with macOS /bin/bash 3.2, where an empty "${captain[@]}" is
# unbound under set -u; expand it only when set.
python3 "$repo/scripts/firstmate-fleet.py" --home "$firstmate_home" \
  --output "$config/fleet.json" --watch --view "$binary" ${captain[@]+"${captain[@]}"} || {
  read -r -p "Fleet stopped. Press Enter to close " || true
}
