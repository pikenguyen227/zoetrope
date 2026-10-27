#!/usr/bin/env bash
# Build reviewable Fleet release assets. Publishing is a separate manual step.
set -euo pipefail
export COPYFILE_DISABLE=1

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/release-fleet.sh <40-character-commit-sha> [--output DIR] [--targets comma-list] [--universal]
       scripts/release-fleet.sh --stage-fixture <sha> <darwin-arm64> <darwin-x64> <linux-arm64> <linux-x64> <output-dir> [--targets comma-list]

The normal path clones the fork at the exact commit and builds selected targets.
Targets default to all four. A Linux-only run stages only its two binary archives.
--stage-fixture is for the archive-layout test; it skips builds and binary checks.
No mode creates a tag or uploads a release.
USAGE
  exit 2
}

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$script_dir/.." && pwd -P)
fixture=false
universal=false
targets_csv=aarch64-apple-darwin,x86_64-apple-darwin,aarch64-unknown-linux-gnu,x86_64-unknown-linux-gnu
if [ "${1:-}" = --stage-fixture ]; then
  [ "$#" -ge 7 ] || usage
  fixture=true
  sha=$2
  arm_binary=$3
  x64_binary=$4
  linux_arm_binary=$5
  linux_x64_binary=$6
  output=$7
  shift 7
  source_tree=$repo
else
  [ "$#" -ge 1 ] || usage
  sha=$1
  shift
  output="$PWD/dist/fleet-${sha:0:12}"
fi
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) [ "$fixture" = false ] && [ "$#" -ge 2 ] || usage; output=$2; shift 2 ;;
    --targets) [ "$#" -ge 2 ] || usage; targets_csv=$2; shift 2 ;;
    --universal) [ "$fixture" = false ] || usage; universal=true; shift ;;
    *) usage ;;
  esac
done
IFS=, read -r -a targets <<< "$targets_csv"
[ "${#targets[@]}" -gt 0 ] || usage
seen=,
for target in "${targets[@]}"; do
  case "$target" in
    aarch64-apple-darwin|x86_64-apple-darwin|aarch64-unknown-linux-gnu|x86_64-unknown-linux-gnu) ;;
    *) echo "Unknown target: $target" >&2; usage ;;
  esac
  case "$seen" in *",$target,"*) echo "Duplicate target: $target" >&2; usage ;; esac
  seen="$seen$target,"
done
has_target() { case "$seen" in *",$1,"*) return 0 ;; *) return 1 ;; esac; }
if [ "$universal" = true ]; then
  if ! has_target aarch64-apple-darwin || ! has_target x86_64-apple-darwin; then
    echo '--universal requires both Darwin targets.' >&2; exit 2;
  fi
fi
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'Pass a full lowercase 40-character commit SHA.' >&2; exit 2; }
[ ! -e "$output" ] || { echo "Output already exists: $output" >&2; exit 2; }
command -v shasum >/dev/null || { echo 'shasum is required.' >&2; exit 2; }
command -v tar >/dev/null || { echo 'tar is required.' >&2; exit 2; }
short=${sha:0:12}
version=0.2.0
tag="fleet-v${version}-${short}"
if ! has_target aarch64-apple-darwin && ! has_target x86_64-apple-darwin; then
  tag="${tag}-linux"
fi
if [ "$fixture" = false ]; then
  [ "$(uname -s)" = Darwin ] || { echo 'macOS is required as the release host.' >&2; exit 2; }
  # Keep Docker bind mounts under the host's shared /private/tmp path.
  work_parent=/private/tmp
else
  work_parent=${TMPDIR:-/tmp}
fi
work=$(mktemp -d "$work_parent/zoe-fleet-release.XXXXXX")
trap 'rm -rf -- "$work"' EXIT

if [ "$fixture" = false ]; then
  for tool in rustc git; do
    command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 2; }
  done
  if has_target aarch64-unknown-linux-gnu || has_target x86_64-unknown-linux-gnu; then
    for tool in file docker; do
      command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 2; }
    done
  fi
  if has_target aarch64-apple-darwin || has_target x86_64-apple-darwin; then
    for tool in cargo rustup arch lipo; do
      command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 2; }
    done
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
      if has_target "$target"; then
        rustup target list --installed | grep -Fxq "$target" || {
          echo "Missing Rust target $target; install it with rustup target add $target" >&2; exit 2;
        }
      fi
    done
  fi
  git clone --no-checkout https://github.com/pikenguyen227/zoetrope.git "$work/source"
  git -C "$work/source" checkout --detach "$sha"
  source_tree="$work/source"
  [ "$(git -C "$source_tree" rev-parse HEAD)" = "$sha" ] || { echo 'Checkout SHA mismatch.' >&2; exit 1; }
  [ -z "$(git -C "$source_tree" status --porcelain)" ] || { echo 'Checkout is not pristine.' >&2; exit 1; }
  epoch=$(git -C "$source_tree" show -s --format=%ct "$sha")
  export SOURCE_DATE_EPOCH=$epoch
  arm_binary="$source_tree/target/aarch64-apple-darwin/release/zoe"
  x64_binary="$source_tree/target/x86_64-apple-darwin/release/zoe"
  linux_arm_binary="$work/linux-arm64/zoe"
  linux_x64_binary="$work/linux-amd64/zoe"
  rustc_version=$(rustc --version)
  toolchain=${rustc_version#rustc }
  toolchain=${toolchain%% *}
  for target in "${targets[@]}"; do
    case "$target" in
      *-apple-darwin)
        (cd "$source_tree" && cargo build --locked --release --bin zoe --target "$target")
        if [ "$target" = aarch64-apple-darwin ]; then slice=arm64; binary=$arm_binary; else slice=x86_64; binary=$x64_binary; fi
        [ -x "$binary" ] || { echo "Missing binary: $binary" >&2; exit 1; }
        lipo -verify_arch "$slice" "$binary"
        version_line=$(arch -"$slice" "$binary" --version)
        case "$version_line" in "zoe $version ($short, built "*) ;; *) echo "Build stamp mismatch: $version_line" >&2; exit 1 ;; esac
        arch -"$slice" "$binary" fleet "$source_tree/assets/fleet/crew/fleet.json" --inspect >/dev/null
        ;;
      *-unknown-linux-gnu)
        if [ "$target" = aarch64-unknown-linux-gnu ]; then
          platform=arm64; binary=$linux_arm_binary; arch_pattern='ARM aarch64'
          image=ubuntu:22.04@sha256:a8fb2ba88181746111cbbe2969826d6546b71226785b196c93d2c3445a25000d
        else
          platform=amd64; binary=$linux_x64_binary; arch_pattern='x86-64'
          image=ubuntu:22.04@sha256:8a2321a92b438238f93b4346fff61bc4259c99fdd4d9b52ee86f153fee0c344f
        fi
        docker run --rm --platform "linux/$platform" \
          -v "$source_tree:/src:ro" -v "$work:/out" \
          -e SOURCE_DATE_EPOCH="$epoch" -e RUST_TOOLCHAIN="$toolchain" \
          -e RELEASE_SHORT="$short" -e RELEASE_VERSION="$version" \
          -e RELEASE_PLATFORM="$platform" \
          "$image" \
          bash -ceu '
            export DEBIAN_FRONTEND=noninteractive
            apt-get update
            apt-get install -y --no-install-recommends ca-certificates curl build-essential git
            curl --proto "=https" --tlsv1.2 -fsS https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
            . "$HOME/.cargo/env"
            git config --global --add safe.directory /src
            [ "$(getconf GNU_LIBC_VERSION)" = "glibc 2.35" ]
            export CARGO_TARGET_DIR="/out/target-$RELEASE_PLATFORM"
            cd /src
            cargo build --locked --release --bin zoe
            mkdir -p "/out/linux-$RELEASE_PLATFORM"
            cp "$CARGO_TARGET_DIR/release/zoe" "/out/linux-$RELEASE_PLATFORM/zoe"
            version_line=$("/out/linux-$RELEASE_PLATFORM/zoe" --version)
            case "$version_line" in "zoe $RELEASE_VERSION ($RELEASE_SHORT, built "*) ;; *) echo "Build stamp mismatch: $version_line" >&2; exit 1 ;; esac
            "/out/linux-$RELEASE_PLATFORM/zoe" fleet /src/assets/fleet/crew/fleet.json --inspect >/dev/null
          '
        [ -x "$binary" ] || { echo "Missing binary: $binary" >&2; exit 1; }
        file "$binary" | grep -Fq "$arch_pattern" || { echo "Wrong ELF architecture: $(file "$binary")" >&2; exit 1; }
        file "$binary" | grep -Fq ELF || { echo "Not an ELF binary: $binary" >&2; exit 1; }
        ;;
    esac
  done
  [ -z "$(git -C "$source_tree" status --porcelain)" ] || { echo 'Build made source checkout dirty.' >&2; exit 1; }
else
  for target in "${targets[@]}"; do
    case "$target" in
      aarch64-apple-darwin) binary=$arm_binary ;;
      x86_64-apple-darwin) binary=$x64_binary ;;
      aarch64-unknown-linux-gnu) binary=$linux_arm_binary ;;
      x86_64-unknown-linux-gnu) binary=$linux_x64_binary ;;
    esac
    [ -f "$binary" ] || { echo "Fixture binary is missing: $binary" >&2; exit 2; }
  done
  epoch=$(git -C "$source_tree" show -s --format=%ct "$sha")
  rustc_version='fixture build'
fi

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
  case "$target" in *-unknown-linux-gnu) printf 'libc=glibc 2.35\n' >> "$stage/BUILD.txt" ;; esac
  tar -czf "$work/$asset" -C "$stage" zoe-fleet LICENSE NOTICE BUILD.txt
}
for target in "${targets[@]}"; do
  case "$target" in
    aarch64-apple-darwin) binary=$arm_binary ;;
    x86_64-apple-darwin) binary=$x64_binary ;;
    aarch64-unknown-linux-gnu) binary=$linux_arm_binary ;;
    x86_64-unknown-linux-gnu) binary=$linux_x64_binary ;;
  esac
  stage_binary "$target" "$binary"
done

if has_target aarch64-apple-darwin || has_target x86_64-apple-darwin; then
  runtime="$work/runtime"
  mkdir -p "$runtime/scripts" "$runtime/fleet-plugin" "$runtime/docs"
  for name in firstmate-fleet.py claude-telemetry.py fleet-demo.py fleet-config.sh; do
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
  if [ -f "$source_tree/bin/zoe-fleet-tower" ]; then
    mkdir -p "$runtime/bin"
    cp "$source_tree/bin/zoe-fleet-tower" "$runtime/bin/zoe-fleet-tower"
    launcher_present=true
  fi
  runtime_asset="zoetrope-fleet-runtime-${version}-${short}.tar.gz"
  paths=(scripts fleet-plugin docs LICENSE NOTICE README.md)
  if [ "$launcher_present" = true ]; then paths+=(bin); fi
  tar -czf "$work/$runtime_asset" -C "$runtime" "${paths[@]}"
fi

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
