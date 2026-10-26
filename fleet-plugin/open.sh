#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
. "$repo/scripts/fleet-config.sh"
fleet_load_config "$repo"
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
(umask 077 && mkdir -p "$ZOE_FLEET_STATE_DIR")
if [ ! -x "$ZOE_FLEET_BIN" ]; then
  printf 'Fleet binary unavailable: %s\n' "$ZOE_FLEET_BIN" >&2
  read -r -p "Press Enter to close " || true
  exit 1
fi
captain=()
context="${HERDR_PLUGIN_CONTEXT_JSON:-}"
[ -n "$context" ] || context='{}'
focused=$(printf '%s' "$context" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("focused_pane_id", ""))' || true)
# The Captain is the pane running this home's coordinator, never merely the
# focused one: the captain's own Shell tab is often focused. With none found,
# pass no --captain and the collector keeps the last Captain registered.
coordinator=$(python3 "$ZOE_FLEET_RUNTIME/fleet-plugin/captain.py" "${HERDR_BIN_PATH:-herdr}" \
  "$FM_HOME" "$focused" || true)
if [ -n "$coordinator" ]; then
  captain=(--captain "$coordinator")
fi
# Herdr runs this with macOS /bin/bash 3.2, where an empty "${captain[@]}" is
# unbound under set -u; expand it only when set.
python3 "$ZOE_FLEET_RUNTIME/scripts/firstmate-fleet.py" --home "$FM_HOME" \
  --output "$ZOE_FLEET_STATE_DIR/fleet.json" --watch --view "$ZOE_FLEET_BIN" ${captain[@]+"${captain[@]}"} || {
  read -r -p "Fleet stopped. Press Enter to close " || true
}
