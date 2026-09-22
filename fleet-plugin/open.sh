#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
agentic_root=$(dirname -- "$repo")
if [ "${HERDR_ENV:-}" != 1 ]; then
  echo "Open Fleet from a genuine Herdr pane." >&2
  exit 1
fi
# Configuration stays outside the repository; no user's session IDs ship here.
config="${ZOE_FLEET_STATE_DIR:-$agentic_root/.tools/state/zoe-fleet}"
mkdir -p "$config"
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
python3 "$repo/scripts/firstmate-fleet.py" --home "$firstmate_home" \
  --output "$config/fleet.json" --watch --view "$binary" "${captain[@]}" || {
  read -r -p "Fleet stopped. Press Enter to close " || true
}
