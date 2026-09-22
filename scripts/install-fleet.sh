#!/usr/bin/env bash
# Install a separate development binary plus two launchers in Agentic/.tools/bin.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
agentic_root=$(dirname -- "$repo")
destination="$agentic_root/.tools/bin"
cd "$repo"
cargo build --locked --release
mkdir -p "$destination"
install -m 755 target/release/zoe "$destination/zoe-fleet.new"
mv -f "$destination/zoe-fleet.new" "$destination/zoe-fleet"
# Bash %q encodes even spaces/quotes in the checkout path without interpolation.
{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'exec python3 %q --play %q\n' "$repo/scripts/fleet-demo.py" "$destination/zoe-fleet"
} > "$destination/zoe-fleet-demo"
{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'if [ "${HERDR_ENV:-}" != 1 ]; then echo "Run this inside Herdr." >&2; exit 1; fi\n'
  printf '"${HERDR_BIN_PATH:-herdr}" plugin link %q\n' "$repo/fleet-plugin"
  printf 'exec "${HERDR_BIN_PATH:-herdr}" plugin action invoke open --plugin pikenguyen227.zoetrope-fleet\n'
} > "$destination/zoe-fleet-open"
chmod 755 "$destination/zoe-fleet-demo" "$destination/zoe-fleet-open"
printf 'Installed Fleet launchers in %s\n' "$destination"
