#!/usr/bin/env bash
# Read-only checks of the invoking account. Run only before replacing HOME.
set -euo pipefail

fail() { printf 'Phase B preflight: %s\n' "$*" >&2; exit 1; }
[ "$#" -eq 4 ] || fail 'expected: account-home herdr-binary plugin-manifest acknowledgement'
account_home=$1 herdr=$2 manifest=$3 acknowledged=$4
[ "$acknowledged" = 1 ] || fail 'pass --phase-b-dedicated-account only in a separate dedicated macOS account or teammate account'
[ -d "$account_home" ] || fail 'account HOME does not exist'
[ "$(cd "$account_home" && pwd -P)" = "$account_home" ] || fail 'account HOME must be canonical'
login_home=$(dscl . -read "/Users/$(id -un)" NFSHomeDirectory 2>/dev/null) \
  || fail 'cannot verify the macOS login home'
login_home=${login_home#NFSHomeDirectory: }
[ "$account_home" = "$login_home" ] || fail 'HOME is not the invoking macOS account home'
[ -x "$herdr" ] || fail 'Herdr binary is unavailable'
[ -z "${FM_HOME:-}" ] || fail 'FM_HOME is already set; this is not a clean dedicated account'
[ -z "${FM_ROOT_OVERRIDE:-}" ] || fail 'FM_ROOT_OVERRIDE is already set'
[ -z "${HERDR_SESSION:-}" ] || [ "$HERDR_SESSION" = default ] || fail 'HERDR_SESSION is not default'
[ -z "${ZOE_NO_MISTAKES_BIN:-}" ] || fail 'ZOE_NO_MISTAKES_BIN must be unset'
[ "${XDG_CONFIG_HOME:-$account_home/.config}" = "$account_home/.config" ] \
  || fail 'XDG_CONFIG_HOME points outside the account home'
[ "${XDG_STATE_HOME:-$account_home/.local/state}" = "$account_home/.local/state" ] \
  || fail 'XDG_STATE_HOME points outside the account home'
for path in "$account_home/.config/zoe-fleet/config" \
  "$account_home/.local/state/zoe-fleet" \
  "$account_home/.config/firstmate" "$account_home/.local/state/firstmate" \
  "$account_home/.firstmate" \
  "$account_home/Desktop/Agentic/firstmate" \
  "$account_home/firstmate/state" "$account_home/firstmate/config" \
  "${XDG_CONFIG_HOME:-$account_home/.config}/zoe-fleet/config" \
  "${XDG_STATE_HOME:-$account_home/.local/state}/zoe-fleet"; do
  [ ! -e "$path" ] || fail "existing fleet or Firstmate state: $path"
done
version=$("$herdr" --version 2>/dev/null) || fail 'cannot read Herdr version'
minimum=$(sed -n 's/^min_herdr_version = "\([0-9][0-9.]*\)"$/\1/p' "$manifest")
[ -n "$minimum" ] || fail 'plugin minimum Herdr version is unavailable'
python3 - "$version" "$minimum" <<'PY' || fail "Herdr $version is below plugin minimum $minimum"
import re, sys
found = re.search(r'(?<!\d)(\d+)\.(\d+)\.(\d+)(?!\d)', sys.argv[1])
minimum = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)', sys.argv[2])
if not found or not minimum or tuple(map(int, found.groups())) < tuple(map(int, minimum.groups())):
    sys.exit(1)
PY
python3 - "$herdr" <<'PY'
import json, subprocess, sys
herdr = sys.argv[1]
def records(kind):
    args = [herdr, kind, 'list']
    if kind != 'session':
        args += ['--session', 'default']
    try:
        output = subprocess.check_output(args, timeout=8, stderr=subprocess.DEVNULL)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise ValueError(f'cannot inspect Herdr {kind} list') from error
    data = json.loads(output)
    if data.get('error'):
        raise ValueError(f'Herdr {kind} list returned an error')
    value = data['result'][kind + 's']
    if not isinstance(value, list):
        raise ValueError(kind)
    return value
try:
    sessions = records('session')
    if len(sessions) != 1 or sessions[0].get('name') != 'default':
        raise ValueError('expected exactly one Herdr session named default')
    if records('workspace'):
        raise ValueError('default Herdr session has workspaces')
    if records('pane'):
        raise ValueError('default Herdr session has panes')
except (ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
    print(f'Phase B preflight: {error}', file=sys.stderr)
    sys.exit(1)
PY
printf 'Phase B preflight: dedicated account signals passed; default Herdr session is empty; Herdr >= %s\n' "$minimum"
