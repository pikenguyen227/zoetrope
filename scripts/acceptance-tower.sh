#!/usr/bin/env bash
# shellcheck disable=SC2016
# Isolated Fleet acceptance. Phase B requires a person and is never implicit.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/acceptance-tower.sh --firstmate-ref <40-char SHA> \
  (--assets <release directory> | --local-bin <zoe binary> --local-runtime <checkout>) \
  [--record <Markdown path>] [--phase-b]

Phase A runs under sandbox-exec and launches no Herdr, agent, or viewer.
--phase-b opens an interactive lab shell for one person to launch Herdr and one
secondmate; its manifest assertions run after that person returns to the runner.
USAGE
  exit 2
}

die() { printf 'Acceptance: %s\n' "$*" >&2; exit 1; }
safe_path() {
  case "$1" in
    *Desktop/Agentic*|*/.config/zoe-fleet|*/.config/zoe-fleet/*)
      die 'input or output path points at live tower state' ;;
  esac
}
need() { command -v "$1" >/dev/null 2>&1 || die "missing tool: $1"; }

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd "$script_dir/.." && pwd -P)
if [ "${1:-}" = --inside-lab ]; then
  [ "${ACCEPTANCE_INSIDE:-}" = 1 ] || die 'internal mode requires the sandbox runner'
  shift
  lab=$1 pin=$2 phase_b=$3 source_kind=$4
  shift 4
  [ "$#" -eq 0 ] || usage
  [ "$HOME" = "$lab/home" ] || die 'lab HOME mismatch'
  [ "$XDG_CONFIG_HOME" = "$lab/home/.config" ] || die 'lab config mismatch'
  [ "$XDG_STATE_HOME" = "$lab/home/.local/state" ] || die 'lab state mismatch'
  [ "$TMPDIR" = "$lab/tmp" ] || die 'lab TMPDIR mismatch'
  case "$PATH" in "$lab/kit/bin:/usr/bin:/bin:/usr/sbin:/sbin") ;; *) die 'non-minimal PATH' ;; esac
  if env | grep -Fq 'Desktop/Agentic'; then die 'inherited environment points at Desktop/Agentic'; fi
  for name in FM_HOME FM_ROOT_OVERRIDE; do
    [ "${!name+x}" != x ] || die "inherited $name"
  done
  while IFS= read -r entry; do
    name=${entry%%=*}
    case "$name" in ZOE_*|HERDR_*|FM_*_OVERRIDE) die "inherited $name" ;; esac
  done < <(env)
  [ -z "${FM_HOME:-}" ] || die 'FM_HOME was inherited'

  fm="$lab/firstmate"
  mate="$lab/mate"
  report="$lab/record.md"
  git clone --no-checkout https://github.com/pikenguyen227/firstmate.git "$fm" >"$lab/clone.log" 2>&1 || {
    cat "$lab/clone.log" >&2; die 'Firstmate clone failed';
  }
  git -C "$fm" checkout --detach "$pin" >>"$lab/clone.log" 2>&1 || {
    cat "$lab/clone.log" >&2; die 'Firstmate pin unavailable';
  }
  [ "$(git -C "$fm" rev-parse HEAD)" = "$pin" ] || die 'Firstmate checkout is not at the pin'
  mkdir -p "$fm/data" "$fm/state" "$fm/config" "$fm/projects"
  "$lab/runtime/bin/zoe-fleet-tower" setup --home "$fm" --bin "$lab/runtime/bin/zoe-fleet" >"$lab/setup.txt"
  config="$XDG_CONFIG_HOME/zoe-fleet/config"
  [ "$(stat -f %Lp "$config")" = 600 ] || die 'config is not mode 0600'
  if grep -Fq 'Desktop/Agentic' "$config"; then die 'config contains owner path'; fi
  "$lab/runtime/bin/zoe-fleet-tower" doctor >"$lab/doctor.txt"

  FM_HOME="$fm" "$fm/bin/fm-fleet-snapshot.sh" --json >"$lab/snapshot-before.json"
  (cd "$fm" && FM_HOME="$fm" \
    FM_SECONDMATE_CHARTER='Acceptance lab mate; local fleet visibility only.' \
    FM_SECONDMATE_SCOPE='Acceptance lab only.' \
    "$fm/bin/fm-home-seed.sh" lab-mate "$mate" --no-projects) >"$lab/seed.txt" 2>&1
  FM_HOME="$fm" "$fm/bin/fm-fleet-snapshot.sh" --json >"$lab/snapshot-after.json"
  python3 - "$lab" <<'PY'
import json, pathlib, sys
lab = pathlib.Path(sys.argv[1])
before = json.loads((lab / 'snapshot-before.json').read_text())
after = json.loads((lab / 'snapshot-after.json').read_text())
fm = str(lab / 'firstmate')
mate = str(lab / 'mate')
assert before['schema'] == after['schema'] == 'fm-fleet-snapshot.v1'
assert before['fm_home'] == after['fm_home'] == fm
assert before['tasks'] == []
records = after['secondmate_current']['records']
assert any(r['id'] == 'lab-mate' and r['home'] == mate and r['registered'] for r in records), records
assert all(str(r['home']).startswith(str(lab) + '/') for r in records), records
assert after['tasks'] == [], 'Phase A must not launch agents'
PY
  "$lab/runtime/bin/zoe-fleet" fleet "$lab/runtime/assets/fleet/crew/fleet.json" --inspect >"$lab/inspect.txt"
  # Immutable pinned source may mention an owner's example path in documentation.
  # Scan all generated lab state and output, excluding the two source checkouts.
  if grep -r -F -l 'Desktop/Agentic' "$lab/home" "$lab/setup.txt" "$lab/doctor.txt" \
      "$lab/seed.txt" "$lab/snapshot-before.json" "$lab/snapshot-after.json" \
      "$lab/inspect.txt" 2>/dev/null | grep -q .; then
    die 'generated lab data contains Desktop/Agentic'
  fi
  {
    printf '# Fleet tower acceptance — Phase A\n\n'
    printf 'Input: %s.\n\n' "$source_kind"
    printf 'Firstmate pin: `%s`\n\n' "$pin"
    printf 'Zoetrope binary: `%s`\n\n' "$("$lab/runtime/bin/zoe-fleet" --version)"
    printf 'Isolation: `sandbox-exec -f scripts/tower-acceptance.sb`; fresh lab HOME, XDG dirs, TMPDIR and minimal PATH.\n\n'
    printf '## Commands and results\n\n'
    printf '1. Clone Firstmate at the pin; create empty `data/`, `state/`, `config/`, `projects/`.\n'
    printf '2. `zoe-fleet-tower setup`, `doctor`, snapshot, `fm-home-seed.sh lab-mate --no-projects`, snapshot.\n'
    printf '3. `zoe-fleet fleet assets/fleet/crew/fleet.json --inspect`.\n\n'
    printf 'Assertions passed: exact source pin; private config; fresh-home snapshot; lab-only registered mate; zero tasks; binary inspect; generated state has no `Desktop/Agentic`.\n\n'
    printf '### Doctor\n\n```text\n'; cat "$lab/doctor.txt"; printf '```\n\n'
    printf '### Binary inspect\n\n```text\n'; cat "$lab/inspect.txt"; printf '```\n\n'
    printf '### Snapshot assertions\n\n```json\n'
    python3 - "$lab/snapshot-after.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
print(json.dumps({k: s[k] for k in ('schema', 'fm_home', 'tasks', 'secondmate_current')}, indent=2))
PY
    printf '```\n\nPhase B: not run; requires one person to launch a real Herdr and agent in the lab.\n'
  } >"$report"
  if [ "$phase_b" = 1 ]; then
    cat <<PROMPT

PHASE B — PERSON REQUIRED. This shell remains inside the disposable sandbox.
Lab: $lab
Provision a real Herdr and an agent inside $lab/kit/bin (replace the inert
Herdr doctor probe there). Use only this lab HOME. Start the lab Herdr and a
pane; from that pane run $lab/runtime/bin/zoe-fleet-tower open. From the lab
Firstmate home, launch only fm-spawn.sh --secondmate lab-mate. Wait for one
collection, then stop that agent and the lab Herdr before typing exit. Never
use a live owner's Herdr or Firstmate. The lab shell opens now.
PROMPT
    /bin/bash --noprofile --norc
    "$lab/runtime/bin/zoe-fleet-tower" inspect >"$lab/phase-b-inspect.txt"
    FM_HOME="$fm" "$fm/bin/fm-fleet-snapshot.sh" --json >"$lab/phase-b-snapshot.json"
    python3 - "$lab" <<'PY'
import json, pathlib, sys
lab = pathlib.Path(sys.argv[1])
state = next((lab / 'home/.local/state/zoe-fleet').glob('*/fleet.json'))
manifest = json.loads(state.read_text())
snapshot = json.loads((lab / 'phase-b-snapshot.json').read_text())
assert manifest['fleet_id'] == 'firstmate:' + str(lab / 'firstmate')
expected = {task['id'] for task in snapshot['tasks']}
actual = {task['id'] for task in manifest['tasks']}
assert expected and expected == actual, (expected, actual)
assert len(expected) == len(snapshot['tasks']) == len(manifest['tasks']), 'unexpected task count'
def check_paths(value, key=''):
    if isinstance(value, dict):
        for child_key, child in value.items():
            check_paths(child, child_key)
    elif isinstance(value, list):
        for child in value:
            check_paths(child, key)
    elif isinstance(value, str) and key in {'file', 'home', 'project', 'root', 'path'}:
        if value.startswith('/'):
            assert value.startswith(str(lab) + '/'), (key, value)
check_paths(manifest)
for path in (state, state.with_name('fleet.events.jsonl')):
    data = path.read_text()
    assert 'Desktop/Agentic' not in data
assert 'Desktop/Agentic' not in (lab / 'phase-b-inspect.txt').read_text()
PY
    {
      printf '\n## Phase B inspect\n\n```json\n'
      cat "$lab/phase-b-inspect.txt"
      printf '```\n\nPhase B assertions passed: fleet ID, exact spawned task set, lab-only paths, no owner path in manifest or journal.\n'
    } >>"$report"
  fi
  exit 0
fi

pin='' assets='' local_bin='' local_runtime='' phase_b=0 record="$PWD/acceptance-tower-record.md"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --firstmate-ref|--assets|--local-bin|--local-runtime|--record)
      [ "$#" -ge 2 ] || usage
      case "$1" in
        --firstmate-ref) pin=$2 ;; --assets) assets=$2 ;; --local-bin) local_bin=$2 ;;
        --local-runtime) local_runtime=$2 ;; --record) record=$2 ;;
      esac
      shift 2 ;;
    --phase-b) phase_b=1; shift ;;
    *) usage ;;
  esac
done
[[ "$pin" =~ ^[0-9a-f]{40}$ ]] || usage
if [ -n "$assets" ]; then
  source_kind='checksum-verified release assets'
  [ -z "$local_bin$local_runtime" ] || usage
else
  source_kind='local build and checked-out runtime'
  [ -n "$local_bin" ] && [ -n "$local_runtime" ] || usage
fi
for input in "$assets" "$local_bin" "$local_runtime" "$record"; do safe_path "$input"; done
for tool in sandbox-exec git python3 shasum tar; do need "$tool"; done
for input in "$assets" "$local_bin" "$local_runtime" "$record"; do
  [ -z "$input" ] || safe_path "$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$input")"
done
real_home=$(cd "$HOME" && pwd -P)
lab=$(mktemp -d /tmp/zoe-tower-acceptance.XXXXXX)
lab=$(cd "$lab" && pwd -P)
trap 'rm -rf -- "$lab"' EXIT
mkdir -p "$lab/home/.config" "$lab/home/.local/state" "$lab/tmp" "$lab/kit/bin" "$lab/runtime/bin" "$lab/runtime/assets/fleet/crew"
if [ -n "$assets" ]; then
  assets=$(cd "$assets" && pwd -P)
  runtime_archive=("$assets"/zoetrope-fleet-runtime-*.tar.gz)
  host_arch=$(uname -m)
  [ "$host_arch" != arm64 ] || host_arch=aarch64
  # This glob deliberately selects the one archive for the host architecture.
  # shellcheck disable=SC2206
  binary_archive=("$assets"/zoe-fleet-*-${host_arch}-apple-darwin.tar.gz)
  [ "${#runtime_archive[@]}" = 1 ] && [ -f "${runtime_archive[0]}" ] || die 'expected one runtime archive'
  [ "${#binary_archive[@]}" = 1 ] && [ -f "${binary_archive[0]}" ] || die 'expected one host binary archive'
  for archive in "${runtime_archive[0]}" "${binary_archive[0]}" "$assets/SHA256SUMS"; do
    safe_path "$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$archive")"
  done
  python3 - "$assets" <<'PY'
import pathlib, re, sys
assets = pathlib.Path(sys.argv[1]).resolve()
lines = (assets / 'SHA256SUMS').read_text().splitlines()
assert lines, 'empty SHA256SUMS'
for line in lines:
    match = re.fullmatch(r'[0-9a-fA-F]{64} [ *]([^/]+\.tar\.gz)', line)
    assert match, 'unsafe SHA256SUMS entry'
    target = (assets / match.group(1)).resolve()
    assert target.parent == assets, 'asset escapes release directory'
PY
  (cd "$assets" && shasum -a 256 -c SHA256SUMS) || die 'release checksums failed'
  tar -xzf "${runtime_archive[0]}" -C "$lab/runtime"
  tar -xzf "${binary_archive[0]}" -C "$lab/runtime/bin" zoe-fleet
else
  [ -x "$local_bin" ] || die 'local binary is not executable'
  [ -f "$local_runtime/bin/zoe-fleet-tower" ] || die 'local runtime lacks the launcher'
  cp "$local_bin" "$lab/runtime/bin/zoe-fleet"
  cp "$local_runtime/bin/zoe-fleet-tower" "$lab/runtime/bin/"
  mkdir -p "$lab/runtime/scripts" "$lab/runtime/fleet-plugin"
  cp "$local_runtime/scripts/"{fleet-config.sh,firstmate-fleet.py,fleet-demo.py,claude-telemetry.py} "$lab/runtime/scripts/"
  cp "$local_runtime/fleet-plugin/"{open.sh,pane.sh,captain.py,herdr-plugin.toml} "$lab/runtime/fleet-plugin/"
fi
cp -R "$repo/assets/fleet/crew/." "$lab/runtime/assets/fleet/crew/"
cp "$script_dir/acceptance-tower.sh" "$lab/runtime/scripts/acceptance-tower.sh"
# Doctor probes plugin availability without launching Herdr. Phase B participants
# replace this probe with their real lab Herdr binary in the interactive shell.
cat >"$lab/kit/bin/herdr" <<'SH'
#!/bin/sh
[ "${1:-}" = plugin ] && [ "${2:-}" = list ] && exit 0
echo 'The Phase A Herdr probe cannot start Herdr.' >&2
exit 1
SH
chmod 755 "$lab/kit/bin/herdr"
chmod 755 "$lab/runtime/bin/zoe-fleet-tower" "$lab/runtime/bin/zoe-fleet"

# Remove all possible owner overrides before entering the sandbox. env -i then
# passes only this allowlist, making the refusal below independent of caller state.
unset FM_HOME FM_ROOT_OVERRIDE
for name in ${!FM_@} ${!ZOE_@} ${!HERDR_@}; do
  case "$name" in FM_*_OVERRIDE|ZOE_*|HERDR_*) unset "$name" ;; esac
done
export HOME="$lab/home" XDG_CONFIG_HOME="$lab/home/.config" XDG_STATE_HOME="$lab/home/.local/state" TMPDIR="$lab/tmp"
export PATH="$lab/kit/bin:/usr/bin:/bin:/usr/sbin:/sbin"
# The inherited process may carry unrelated owner paths. Only the explicit
# env -i allowlist crosses the boundary; internal mode refuses any leftovers.
if [ -f "$XDG_CONFIG_HOME/zoe-fleet/config" ] && grep -Fq 'Desktop/Agentic' "$XDG_CONFIG_HOME/zoe-fleet/config"; then
  die 'lab config points at Desktop/Agentic'
fi
(cd "$lab" && env -i HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" XDG_STATE_HOME="$XDG_STATE_HOME" \
  TMPDIR="$TMPDIR" PATH="$PATH" ACCEPTANCE_INSIDE=1 \
  /usr/bin/sandbox-exec -D REAL_HOME="$real_home" \
  -D OWNER_FIRSTMATE="$real_home/Desktop/Agentic/firstmate" \
  -D OWNER_ZOETROPE="$real_home/Desktop/Agentic/zoetrope" \
  -D OWNER_TOOLS="$real_home/Desktop/Agentic/.tools" \
  -f "$script_dir/tower-acceptance.sb" /bin/bash "$lab/runtime/scripts/acceptance-tower.sh" \
  --inside-lab "$lab" "$pin" "$phase_b" "$source_kind")
mkdir -p "$(dirname "$record")"
cp "$lab/record.md" "$record"
printf 'Acceptance record: %s\n' "$record"
