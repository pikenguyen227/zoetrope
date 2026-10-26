#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/zoe-fleet-test.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
sha=$(git -C "$repo" rev-parse HEAD)
printf '#!/bin/sh\nexit 0\n' > "$work/arm"
printf '#!/bin/sh\nexit 0\n' > "$work/x64"
chmod +x "$work/arm" "$work/x64"
"$repo/scripts/release-fleet.sh" --stage-fixture "$sha" "$work/arm" "$work/x64" "$work/out"
short=${sha:0:12}
(cd "$work/out" && shasum -a 256 -c SHA256SUMS >/dev/null)
[ "$(find "$work/out" -type f | wc -l | tr -d ' ')" = 4 ]
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  archive="$work/out/zoe-fleet-0.2.0-$short-$target.tar.gz"
  [ -f "$archive" ]
  mkdir "$work/$target"
  tar -xzf "$archive" -C "$work/$target"
  [ "$(find "$work/$target" -type f | wc -l | tr -d ' ')" = 4 ]
  [ "$(find "$work/$target" -type f -name zoe-fleet | wc -l | tr -d ' ')" = 1 ]
  [ -x "$work/$target/zoe-fleet" ]
  for name in LICENSE NOTICE BUILD.txt; do [ -f "$work/$target/$name" ]; done
  grep -Fxq "commit=$sha" "$work/$target/BUILD.txt"
  grep -Fq "Build commit: $sha" "$work/$target/NOTICE"
done
runtime="$work/out/zoetrope-fleet-runtime-0.2.0-$short.tar.gz"
mkdir "$work/runtime"
tar -xzf "$runtime" -C "$work/runtime"
for name in scripts/firstmate-fleet.py scripts/claude-telemetry.py scripts/fleet-demo.py \
  fleet-plugin/open.sh fleet-plugin/pane.sh fleet-plugin/captain.py \
  fleet-plugin/herdr-plugin.toml docs/FLEET-USAGE.md docs/FLEET-PACKAGING.md \
  README.md LICENSE NOTICE; do
  [ -f "$work/runtime/$name" ] || { echo "Missing runtime file: $name" >&2; exit 1; }
done
if [ -f "$repo/bin/zoe-fleet-tower" ]; then
  [ -f "$work/runtime/bin/zoe-fleet-tower" ]
fi
printf 'Fleet archive fixture passed.\n'
