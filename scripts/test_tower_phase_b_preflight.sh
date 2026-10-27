#!/usr/bin/env bash
# Stubs only: never connects to a real Herdr session.
set -euo pipefail
unset FM_HOME FM_ROOT_OVERRIDE HERDR_SESSION ZOE_NO_MISTAKES_BIN XDG_CONFIG_HOME XDG_STATE_HOME
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
scratch=$(mktemp -d /tmp/zoe-phase-b-preflight-test.XXXXXX)
scratch=$(cd "$scratch" && pwd -P)
trap 'python3 -c '\''import shutil,sys; shutil.rmtree(sys.argv[1])'\'' "$scratch"' EXIT
mkdir -p "$scratch/bin" "$scratch/account"
cat >"$scratch/bin/dscl" <<'SH'
#!/bin/sh
printf 'NFSHomeDirectory: %s\n' "$TEST_ACCOUNT_HOME"
SH
cat >"$scratch/bin/herdr" <<'SH'
#!/bin/sh
case "$*" in
  --version) printf 'herdr %s\n' "${TEST_HERDR_VERSION:-0.9.1}" ;;
  'session list') printf '{"result":{"sessions":[{"name":"%s"}]}}\n' "${TEST_SESSION:-default}" ;;
  'workspace list --session default') printf '{"result":{"workspaces":%s}}\n' "${TEST_WORKSPACES:-[]}" ;;
  'pane list --session default') printf '{"result":{"panes":%s}}\n' "${TEST_PANES:-[]}" ;;
  *) exit 1 ;;
esac
SH
chmod 755 "$scratch/bin/dscl" "$scratch/bin/herdr"
export PATH="$scratch/bin:$PATH" TEST_ACCOUNT_HOME="$scratch/account"
check() {
  local expected=$1 actual=0
  shift
  "$@" >"$scratch/out" 2>"$scratch/err" || actual=$?
  if [ "$expected" = pass ]; then
    [ "$actual" -eq 0 ] || { cat "$scratch/err" >&2; exit 1; }
  else
    [ "$actual" -ne 0 ] || { echo "unexpected preflight pass: $expected" >&2; exit 1; }
    grep -Fq "$expected" "$scratch/err" || { cat "$scratch/err" >&2; exit 1; }
  fi
}
preflight() {
  "$root/scripts/tower-phase-b-preflight.sh" "$scratch/account" "$scratch/bin/herdr" \
    "$root/fleet-plugin/herdr-plugin.toml" "$1"
}
check 'pass --phase-b-dedicated-account' preflight 0
check pass preflight 1
check 'Herdr binary is unavailable' "$root/scripts/tower-phase-b-preflight.sh" \
  "$scratch/account" "$scratch/bin/missing-herdr" "$root/fleet-plugin/herdr-plugin.toml" 1
TEST_ACCOUNT_HOME="$scratch/other" check 'HOME is not the invoking' preflight 1
FM_HOME="$scratch/owner-fm" check 'FM_HOME is already set' preflight 1
FM_ROOT_OVERRIDE="$scratch/owner-fm" check 'FM_ROOT_OVERRIDE is already set' preflight 1
HERDR_SESSION=other check 'HERDR_SESSION is not default' preflight 1
ZOE_NO_MISTAKES_BIN=/stub check 'ZOE_NO_MISTAKES_BIN must be unset' preflight 1
XDG_CONFIG_HOME="$scratch/elsewhere" check 'XDG_CONFIG_HOME points outside' preflight 1
XDG_STATE_HOME="$scratch/elsewhere" check 'XDG_STATE_HOME points outside' preflight 1
mkdir -p "$scratch/account/.config/zoe-fleet"
: >"$scratch/account/.config/zoe-fleet/config"
check 'existing fleet or Firstmate state' preflight 1
rm "$scratch/account/.config/zoe-fleet/config"
mkdir -p "$scratch/account/.local/state/zoe-fleet"
check 'existing fleet or Firstmate state' preflight 1
rmdir "$scratch/account/.local/state/zoe-fleet"
mkdir -p "$scratch/account/firstmate/state"
check 'existing fleet or Firstmate state' preflight 1
rmdir "$scratch/account/firstmate/state"
TEST_HERDR_VERSION=0.7.3 check 'below plugin minimum' preflight 1
TEST_SESSION=other check 'expected exactly one Herdr session named default' preflight 1
TEST_WORKSPACES='[{"id":"w1"}]' check 'default Herdr session has workspaces' preflight 1
TEST_PANES='[{"pane_id":"p1"}]' check 'default Herdr session has panes' preflight 1
# The staged command injects the lab environment into both creation paths.
mkdir -p "$scratch/lab/kit/bin" "$scratch/lab/firstmate"
cp "$root/scripts/tower-phase-b-herdr.sh" "$scratch/lab/kit/bin/herdr"
cat >"$scratch/lab/kit/bin/herdr-real" <<'SH'
#!/bin/sh
printf '%s\n' "$@"
SH
chmod 755 "$scratch/lab/kit/bin/herdr" "$scratch/lab/kit/bin/herdr-real"
export HOME="$scratch/lab/home" XDG_CONFIG_HOME="$scratch/lab/home/.config"
export XDG_STATE_HOME="$scratch/lab/home/.local/state" XDG_DATA_HOME="$scratch/lab/home/.local/share"
export TMPDIR="$scratch/lab/tmp" CODEX_HOME="$scratch/lab/home/.codex"
for kind in workspace tab; do
  "$scratch/lab/kit/bin/herdr" "$kind" create --cwd "$scratch/lab/firstmate" >"$scratch/wrapper-out"
  grep -Fxq "HOME=$HOME" "$scratch/wrapper-out"
  grep -Fxq "CODEX_HOME=$CODEX_HOME" "$scratch/wrapper-out"
  grep -Fxq "FM_HOME=$scratch/lab/firstmate" "$scratch/wrapper-out"
  grep -Fxq "ACCEPTANCE_LAB=$scratch/lab" "$scratch/wrapper-out"
  grep -Fxq "HERDR_BIN_PATH=$scratch/lab/kit/bin/herdr" "$scratch/wrapper-out"
done
echo 'Phase B preflight stub cases: passed'
