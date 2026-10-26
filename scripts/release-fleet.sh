#!/usr/bin/env bash
# Build reviewable Fleet release assets. Publishing is a separate manual step.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/release-fleet.sh <40-character-commit-sha> [--output DIR] [--universal]
       scripts/release-fleet.sh --stage-fixture <sha> <arm64-zoe> <x64-zoe> <output-dir>

The normal path clones the fork at the exact commit and builds both macOS targets.
--stage-fixture is for the archive-layout test; it skips builds and binary checks.
No mode creates a tag or uploads a release.
USAGE
  exit 2
}

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$script_dir/.." && pwd -P)
fixture=false
universal=false
if [ "${1:-}" = --stage-fixture ]; then
  [ "$#" -eq 5 ] || usage
  fixture=true
  sha=$2
  arm_binary=$3
  x64_binary=$4
  output=$5
  source_tree=$repo
else
  [ "$#" -ge 1 ] || usage
  sha=$1
  shift
  output="$PWD/dist/fleet-${sha:0:12}"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --output) [ "$#" -ge 2 ] || usage; output=$2; shift 2 ;;
      --universal) universal=true; shift ;;
      *) usage ;;
    esac
  done
fi
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'Pass a full lowercase 40-character commit SHA.' >&2; exit 2; }
[ ! -e "$output" ] || { echo "Output already exists: $output" >&2; exit 2; }
command -v shasum >/dev/null || { echo 'shasum is required.' >&2; exit 2; }
command -v tar >/dev/null || { echo 'tar is required.' >&2; exit 2; }
short=${sha:0:12}
version=0.2.0
tag="fleet-v${version}-${short}"
work=$(mktemp -d "${TMPDIR:-/tmp}/zoe-fleet-release.XXXXXX")
trap 'rm -rf -- "$work"' EXIT

if [ "$fixture" = false ]; then
  [ "$(uname -s)" = Darwin ] || { echo 'macOS is required for the Darwin builds.' >&2; exit 2; }
  for tool in cargo rustc git arch lipo; do
    command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 2; }
  done
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target list --installed | grep -Fxq "$target" || {
      echo "Missing Rust target $target; install it with rustup target add $target" >&2; exit 2;
    }
  done
  git clone --no-checkout https://github.com/pikenguyen227/zoetrope.git "$work/source"
  git -C "$work/source" checkout --detach "$sha"
  source_tree="$work/source"
  [ "$(git -C "$source_tree" rev-parse HEAD)" = "$sha" ] || { echo 'Checkout SHA mismatch.' >&2; exit 1; }
  [ -z "$(git -C "$source_tree" status --porcelain)" ] || { echo 'Checkout is not pristine.' >&2; exit 1; }
  epoch=$(git -C "$source_tree" show -s --format=%ct "$sha")
  export SOURCE_DATE_EPOCH=$epoch
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd "$source_tree" && cargo build --locked --release --bin zoe --target "$target")
  done
  arm_binary="$source_tree/target/aarch64-apple-darwin/release/zoe"
  x64_binary="$source_tree/target/x86_64-apple-darwin/release/zoe"
  for pair in "arm64:$arm_binary" "x86_64:$x64_binary"; do
    slice=${pair%%:*}
    binary=${pair#*:}
    [ -x "$binary" ] || { echo "Missing binary: $binary" >&2; exit 1; }
    lipo -verify_arch "$slice" "$binary"
    version_line=$(arch -"$slice" "$binary" --version)
    case "$version_line" in *"($short, built "*) ;; *) echo "Build stamp mismatch: $version_line" >&2; exit 1 ;; esac
    arch -"$slice" "$binary" fleet "$source_tree/assets/fleet/crew/fleet.json" --inspect >/dev/null
  done
  [ -z "$(git -C "$source_tree" status --porcelain)" ] || { echo 'Build made source checkout dirty.' >&2; exit 1; }
else
  [ -f "$arm_binary" ] && [ -f "$x64_binary" ] || { echo 'Fixture binaries are missing.' >&2; exit 2; }
  epoch=$(git -C "$source_tree" show -s --format=%ct "$sha")
fi
rustc_version=$(rustc --version 2>/dev/null || printf 'fixture build')

stage_binary() {
  local target=$1 binary=$2 stage="$work/stage-$1" asset
  asset="zoe-fleet-${version}-${short}-${target}.tar.gz"
  mkdir -p "$stage"
  cp "$binary" "$stage/zoe-fleet"
  chmod 755 "$stage/zoe-fleet"
  cp "$source_tree/LICENSE" "$stage/LICENSE"
  cp "$source_tree/NOTICE" "$stage/NOTICE"
  printf '\nBuild commit: %s\n' "$sha" >> "$stage/NOTICE"
  printf 'version=%s\ncommit=%s\ntarget=%s\nrustc=%s\nSOURCE_DATE_EPOCH=%s\n' \
    "$version" "$sha" "$target" "$rustc_version" "$epoch" > "$stage/BUILD.txt"
  tar -czf "$work/$asset" -C "$stage" zoe-fleet LICENSE NOTICE BUILD.txt
}
stage_binary aarch64-apple-darwin "$arm_binary"
stage_binary x86_64-apple-darwin "$x64_binary"

runtime="$work/runtime"
mkdir -p "$runtime/scripts" "$runtime/fleet-plugin" "$runtime/docs"
for name in firstmate-fleet.py claude-telemetry.py fleet-demo.py; do
  cp "$source_tree/scripts/$name" "$runtime/scripts/$name"
done
for name in open.sh pane.sh captain.py herdr-plugin.toml; do
  cp "$source_tree/fleet-plugin/$name" "$runtime/fleet-plugin/$name"
done
for name in FLEET-USAGE.md FLEET-PACKAGING.md; do
  cp "$source_tree/docs/$name" "$runtime/docs/$name"
done
cp "$source_tree/README.md" "$source_tree/LICENSE" "$runtime/"
cp "$source_tree/NOTICE" "$runtime/NOTICE"
printf '\nBuild commit: %s\n' "$sha" >> "$runtime/NOTICE"
launcher_present=false
if [ -f "$source_tree/scripts/zoe-fleet-tower" ]; then
  mkdir -p "$runtime/bin"
  cp "$source_tree/scripts/zoe-fleet-tower" "$runtime/bin/zoe-fleet-tower"
  launcher_present=true
elif [ -f "$source_tree/bin/zoe-fleet-tower" ]; then
  mkdir -p "$runtime/bin"
  cp "$source_tree/bin/zoe-fleet-tower" "$runtime/bin/zoe-fleet-tower"
  launcher_present=true
fi
runtime_asset="zoetrope-fleet-runtime-${version}-${short}.tar.gz"
paths=(scripts fleet-plugin docs LICENSE NOTICE README.md)
if [ "$launcher_present" = true ]; then paths+=(bin); fi
tar -czf "$work/$runtime_asset" -C "$runtime" "${paths[@]}"

if [ "$universal" = true ]; then
  universal_binary="$work/zoe-universal"
  lipo -create "$arm_binary" "$x64_binary" -output "$universal_binary"
  lipo -verify_arch arm64 x86_64 "$universal_binary"
  stage_binary universal-apple-darwin "$universal_binary"
fi

mkdir -p -- "$(dirname -- "$output")" && mkdir -- "$output"
cp "$work"/*.tar.gz "$output/"
(cd "$output" && shasum -a 256 ./*.tar.gz | sed 's@ \./@ @' > SHA256SUMS && shasum -a 256 -c SHA256SUMS)
printf 'Staged %s at %s\n' "$tag" "$output"
printf 'No tag or release was created.\n'
