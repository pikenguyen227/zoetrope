#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/zoe-fleet-test.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
sha=$(git -C "$repo" rev-parse HEAD)
printf '#!/bin/sh\nexit 0\n' > "$work/arm"
printf '#!/bin/sh\nexit 0\n' > "$work/x64"
printf '#!/bin/sh\nexit 0\n' > "$work/linux-arm"
printf '#!/bin/sh\nexit 0\n' > "$work/linux-x64"
chmod +x "$work/arm" "$work/x64" "$work/linux-arm" "$work/linux-x64"
"$repo/scripts/release-fleet.sh" --stage-fixture "$sha" "$work/arm" "$work/x64" "$work/linux-arm" "$work/linux-x64" "$work/out"
short=${sha:0:12}
(cd "$work/out" && shasum -a 256 -c SHA256SUMS >/dev/null)
[ "$(find "$work/out" -type f | wc -l | tr -d ' ')" = 6 ]
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  archive="$work/out/zoe-fleet-0.2.0-$short-$target.tar.gz"
  [ -f "$archive" ]
  [ "$(tar -tzf "$archive" | LC_ALL=C sort | tr '\n' ' ')" = 'BUILD.txt LICENSE NOTICE zoe-fleet ' ]
  mkdir "$work/$target"
  tar -xzf "$archive" -C "$work/$target"
  [ "$(find "$work/$target" -type f | wc -l | tr -d ' ')" = 4 ]
  [ "$(find "$work/$target" -type f -name zoe-fleet | wc -l | tr -d ' ')" = 1 ]
  [ -x "$work/$target/zoe-fleet" ]
  for name in LICENSE NOTICE BUILD.txt; do [ -f "$work/$target/$name" ]; done
  grep -Fxq "commit=$sha" "$work/$target/BUILD.txt"
  grep -Fq "Build commit: $sha" "$work/$target/NOTICE"
  case "$target" in *-unknown-linux-gnu) grep -Fxq 'libc=glibc 2.35' "$work/$target/BUILD.txt" ;; esac
done
runtime="$work/out/zoetrope-fleet-runtime-0.2.0-$short.tar.gz"
mkdir "$work/runtime"
tar -xzf "$runtime" -C "$work/runtime"
if tar -tzf "$runtime" | grep -Eq '(^|/)\._'; then echo 'AppleDouble entry in runtime archive' >&2; exit 1; fi
grep -Fxq 'rustc=fixture build' "$work/aarch64-apple-darwin/BUILD.txt"
for name in scripts/firstmate-fleet.py scripts/claude-telemetry.py scripts/fleet-demo.py scripts/fleet-config.sh \
  fleet-plugin/open.sh fleet-plugin/pane.sh fleet-plugin/captain.py \
  fleet-plugin/herdr-plugin.toml docs/FLEET-USAGE.md docs/FLEET-PACKAGING.md \
  README.md LICENSE NOTICE; do
  [ -f "$work/runtime/$name" ] || { echo "Missing runtime file: $name" >&2; exit 1; }
done
if [ -f "$repo/bin/zoe-fleet-tower" ]; then
  [ -f "$work/runtime/bin/zoe-fleet-tower" ]
fi
"$repo/scripts/release-fleet.sh" --stage-fixture "$sha" "$work/arm" "$work/x64" "$work/linux-arm" "$work/linux-x64" "$work/linux-out" \
  --targets aarch64-unknown-linux-gnu,x86_64-unknown-linux-gnu
(cd "$work/linux-out" && shasum -a 256 -c SHA256SUMS >/dev/null)
[ "$(find "$work/linux-out" -type f | wc -l | tr -d ' ')" = 3 ]
[ "$(wc -l < "$work/linux-out/SHA256SUMS" | tr -d ' ')" = 2 ]
[ ! -e "$work/linux-out/zoetrope-fleet-runtime-0.2.0-$short.tar.gz" ]
for target in aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  [ -f "$work/linux-out/zoe-fleet-0.2.0-$short-$target.tar.gz" ]
done
printf 'Fleet archive fixture passed.\n'
