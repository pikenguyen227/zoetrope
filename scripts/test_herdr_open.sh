#!/usr/bin/env bash
# Stubs only: never runs a real zoe or Herdr.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
scratch=$(mktemp -d /tmp/zoe-herdr-open-test.XXXXXX)
scratch=$(cd "$scratch" && pwd -P)
trap 'python3 -c '\''import shutil,sys; shutil.rmtree(sys.argv[1])'\'' "$scratch"' EXIT
mkdir -p "$scratch/plugin/herdr" "$scratch/home/.cargo/bin" "$scratch/pathbin" "$scratch/empty"
cat >"$scratch/plugin/herdr/resolve.sh" <<'SH'
#!/usr/bin/env bash
echo "claude fake-session"
SH
stub() {
  printf '#!/bin/sh\necho "%s $*"\n' "$2" >"$1"
  chmod 755 "$1"
}
# The system-wide fallback locations are redirected into the scratch dir, so a zoe
# really installed on this machine can never be picked up or run.
ln -s "$(command -v bash)" "$scratch/empty/bash"
ln -s "$(command -v env)" "$scratch/empty/env"
mkdir -p "$scratch/opt-homebrew" "$scratch/usr-local"
sed -e "s|/opt/homebrew/bin|$scratch/opt-homebrew|" -e "s|/usr/local/bin|$scratch/usr-local|" \
  "$root/herdr-plugin/herdr/open.sh" >"$scratch/open.sh"
grep -Fq "$scratch/opt-homebrew/zoe" "$scratch/open.sh"
grep -Fq "$scratch/usr-local/zoe" "$scratch/open.sh"
run_open() {
  env -i PATH="$1" HOME="$scratch/home" HERDR_PLUGIN_ROOT="$scratch/plugin" \
    "$(command -v bash)" "$scratch/open.sh" </dev/null
}
expect() {
  grep -Fq -- "$1" "$scratch/out" 2>&1 || { echo "missing '$1' in:" >&2; cat "$scratch/out" >&2; exit 1; }
}

stub "$scratch/home/.cargo/bin/zoe" cargo
run_open "$scratch/empty" >"$scratch/out" 2>&1
expect "cargo --provider claude --follow fake-session"

ln -s "$(command -v bash)" "$scratch/pathbin/bash"
ln -s "$(command -v env)" "$scratch/pathbin/env"
stub "$scratch/pathbin/zoe" path
run_open "$scratch/pathbin" >"$scratch/out" 2>&1
expect "path --provider claude --follow fake-session"

mv "$scratch/home/.cargo/bin/zoe" "$scratch/home/.cargo/bin/zoe.off"
stub "$scratch/usr-local/zoe" usrlocal
run_open "$scratch/empty" >"$scratch/out" 2>&1
expect "usrlocal --provider claude --follow fake-session"
mv "$scratch/usr-local/zoe" "$scratch/usr-local/zoe.off"
if run_open "$scratch/empty" >"$scratch/out" 2>&1; then
  echo "open.sh passed with no zoe" >&2
  exit 1
fi
expect "zoe is not on PATH or in a standard install location"
echo ok
