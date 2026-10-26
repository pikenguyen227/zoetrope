#!/usr/bin/env bash
# Developer build and one-time migration; runtime behavior lives in zoe-fleet-tower.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
parent=$(dirname "$repo")
legacy=0
if [ -x "$parent/firstmate/bin/fm-fleet-snapshot.sh" ] && [ -d "$parent/.tools/bin" ]; then
  if [ -z "${FM_HOME:-}" ]; then
    legacy=1
  elif [ -d "$FM_HOME" ] &&
    [ "$(cd "$FM_HOME" && pwd -P)" = "$(cd "$parent/firstmate" && pwd -P)" ]; then
    legacy=1
  fi
fi
if [ "$legacy" = 1 ]; then
  destination=${ZOE_FLEET_INSTALL_DIR:-$parent/.tools/bin}
else
  destination=${ZOE_FLEET_INSTALL_DIR:-$HOME/.local/bin}
fi
cd "$repo"
cargo build --locked --release --bin zoe
mkdir -p "$destination"
install -m 755 target/release/zoe "$destination/zoe-fleet.new"
mv -f "$destination/zoe-fleet.new" "$destination/zoe-fleet"
ln -sfn "$repo/bin/zoe-fleet-tower" "$destination/zoe-fleet-tower"
# Keep the old command names; both dispatch through the same launcher.
for action in open demo; do
  wrapper="$destination/zoe-fleet-$action"
  # shellcheck disable=SC2016 # the wrapper expands $0 and $@ when it runs
  { printf '#!/usr/bin/env bash\nset -euo pipefail\n';
    printf 'exec "$(dirname "$0")/zoe-fleet-tower" %s "$@"\n' "$action"; } > "$wrapper"
  chmod 755 "$wrapper"
done
config="${XDG_CONFIG_HOME:-$HOME/.config}/zoe-fleet/config"
if [ ! -f "$config" ]; then
  home=${FM_HOME:-}
  if [ -z "$home" ] && [ "$legacy" = 1 ]; then home="$parent/firstmate"; fi
  if [ -z "$home" ]; then
    printf 'Fleet binary installed. Configure it with zoe-fleet-tower setup --home <Firstmate home>.\n'
  elif [ "$legacy" = 1 ]; then
    args=(setup --home "$home" --bin "$destination/zoe-fleet" \
      --state "$parent/.tools/state/zoe-fleet" --telemetry "$parent/.tools/state/zoe-telemetry")
    if [ -x "$destination/no-mistakes" ]; then args+=(--no-mistakes "$destination/no-mistakes"); fi
    "$repo/bin/zoe-fleet-tower" "${args[@]}"
  else
    "$repo/bin/zoe-fleet-tower" setup --home "$home" --bin "$destination/zoe-fleet"
  fi
fi
printf 'Installed Fleet commands in %s\n' "$destination"
printf 'An already-open Team tab keeps its previous build until reopened.\n'
