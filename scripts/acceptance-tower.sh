#!/usr/bin/env bash
# shellcheck disable=SC2016
# Isolated Fleet acceptance. Phase B requires a person and is never implicit.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/acceptance-tower.sh --firstmate-ref <40-char SHA> \
  (--assets <release directory> | --local-bin <zoe binary> --local-runtime <checkout>) \
  [--record <Markdown path>] [--evidence-dir <directory>] [--phase-b]

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
phase() { printf '%s\n' "$1" >"$lab/failure-phase.txt"; }

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd "$script_dir/.." && pwd -P)
if [ "${1:-}" = --inside-lab ]; then
  [ "${ACCEPTANCE_INSIDE:-}" = 1 ] || die 'internal mode requires the sandbox runner'
  shift
  lab=$1 pin=$2 phase_b=$3 source_kind=$4 real_home=$5
  shift 5
  [ "$#" -eq 0 ] || usage
  case "$real_home" in /?*) ;; *) die 'real home must be an absolute path' ;; esac
  case "$lab/" in "$real_home"/*) die 'lab is under the real home' ;; esac
  [ "$HOME" = "$lab/home" ] || die 'lab HOME mismatch'
  [ "$XDG_CONFIG_HOME" = "$lab/home/.config" ] || die 'lab config mismatch'
  [ "$XDG_STATE_HOME" = "$lab/home/.local/state" ] || die 'lab state mismatch'
  [ "$TMPDIR" = "$lab/tmp" ] || die 'lab TMPDIR mismatch'
  case "$PATH" in "$lab/kit/bin:/usr/bin:/bin:/usr/sbin:/sbin") ;; *) die 'non-minimal PATH' ;; esac
  if env | grep -Fq -e 'Desktop/Agentic' -e "$real_home/"; then die 'inherited environment points at an owner path'; fi
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
  phase clone
  git clone --no-checkout https://github.com/pikenguyen227/firstmate.git "$fm" >"$lab/clone.log" 2>&1 || {
    cat "$lab/clone.log" >&2; die 'Firstmate clone failed';
  }
  git -C "$fm" checkout --detach "$pin" >>"$lab/clone.log" 2>&1 || {
    cat "$lab/clone.log" >&2; die 'Firstmate pin unavailable';
  }
  [ "$(git -C "$fm" rev-parse HEAD)" = "$pin" ] || die 'Firstmate checkout is not at the pin'
  mkdir -p "$fm/data" "$fm/state" "$fm/config" "$fm/projects"
  phase setup
  "$lab/runtime/bin/zoe-fleet-tower" setup --home "$fm" --bin "$lab/runtime/bin/zoe-fleet" >"$lab/setup.txt" 2>&1
  config="$XDG_CONFIG_HOME/zoe-fleet/config"
  [ "$(stat -f %Lp "$config")" = 600 ] || die 'config is not mode 0600'
  if grep -Fq -e 'Desktop/Agentic' -e "$real_home/" "$config"; then die 'config contains owner path'; fi
  phase doctor
  "$lab/runtime/bin/zoe-fleet-tower" doctor >"$lab/doctor.txt" 2>"$lab/doctor.err"

  phase snapshot-before
  FM_HOME="$fm" "$fm/bin/fm-fleet-snapshot.sh" --json >"$lab/snapshot-before.json" 2>"$lab/snapshot-before.err"
  phase seed
  (cd "$fm" && FM_HOME="$fm" \
    FM_SECONDMATE_CHARTER='Acceptance lab mate; local fleet visibility only.' \
    FM_SECONDMATE_SCOPE='Acceptance lab only.' \
    "$fm/bin/fm-home-seed.sh" lab-mate "$mate" --no-projects) >"$lab/seed.txt" 2>&1
  phase snapshot-after
  FM_HOME="$fm" "$fm/bin/fm-fleet-snapshot.sh" --json >"$lab/snapshot-after.json" 2>"$lab/snapshot-after.err"
  phase phase-a-assertions
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
  phase inspect
  "$lab/runtime/bin/zoe-fleet" fleet "$lab/runtime/assets/fleet/crew/fleet.json" --inspect >"$lab/inspect.txt" 2>"$lab/inspect.err"
  # Immutable pinned source may mention an owner's example path in documentation.
  # Scan all generated lab state and output, excluding the two source checkouts.
  if grep -r -F -l -e 'Desktop/Agentic' -e "$real_home/" "$lab/home" "$lab/setup.txt" "$lab/doctor.txt" \
      "$lab/seed.txt" "$lab/snapshot-before.json" "$lab/snapshot-after.json" \
      "$lab/inspect.txt" 2>/dev/null | grep -q .; then
    die 'generated lab data contains an owner path'
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
    printf 'Assertions passed: exact source pin; private config; fresh-home snapshot; lab-only registered mate; zero tasks; binary inspect; generated state has no `Desktop/Agentic` or real-home path.\n\n'
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
    phase phase-b-shell
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
    phase phase-b-inspect
    "$lab/runtime/bin/zoe-fleet-tower" inspect >"$lab/phase-b-inspect.txt" 2>"$lab/phase-b-inspect.err"
    phase phase-b-snapshot
    FM_HOME="$fm" "$fm/bin/fm-fleet-snapshot.sh" --json >"$lab/phase-b-snapshot.json" 2>"$lab/phase-b-snapshot.err"
    phase phase-b-assertions
    python3 - "$lab" "$real_home" <<'PY'
import json, pathlib, sys
lab = pathlib.Path(sys.argv[1])
real_home = sys.argv[2]
state = next((lab / 'home/.local/state/zoe-fleet').glob('*/fleet.json'))
manifest = json.loads(state.read_text())
snapshot = json.loads((lab / 'phase-b-snapshot.json').read_text())
assert manifest['fleet_id'] == 'firstmate:' + str(lab / 'firstmate')
expected = {task['id'] for task in snapshot['tasks']}
actual = {task['id'] for task in manifest['tasks']}
assert expected == actual == {'lab-mate'}, (expected, actual)
assert len(expected) == len(snapshot['tasks']) == len(manifest['tasks']), 'unexpected task count'
def owner_path(text):
    return 'Desktop/Agentic' in text or text == real_home or real_home + '/' in text
def check_paths(value, key=''):
    if isinstance(value, dict):
        for child_key, child in value.items():
            assert not owner_path(child_key), child_key
            check_paths(child, child_key)
    elif isinstance(value, list):
        for child in value:
            check_paths(child, key)
    elif isinstance(value, str):
        assert not owner_path(value), (key, value)
        if key in {'file', 'home', 'project', 'root', 'path'} and value.startswith('/'):
            assert value.startswith(str(lab) + '/'), (key, value)
check_paths(manifest)
for line in state.with_name('fleet.events.jsonl').read_text().splitlines():
    if line.strip():
        check_paths(json.loads(line))
assert not owner_path((lab / 'phase-b-inspect.txt').read_text())
PY
    {
      printf '\n## Phase B inspect\n\n```json\n'
      cat "$lab/phase-b-inspect.txt"
      printf '```\n\nPhase B assertions passed: fleet ID, exact spawned task set `{lab-mate}`, lab-only paths, no owner path in manifest or journal.\n'
    } >>"$report"
  fi
  exit 0
fi

pin='' assets='' local_bin='' local_runtime='' phase_b=0 record="$PWD/acceptance-tower-record.md" evidence_dir=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --firstmate-ref|--assets|--local-bin|--local-runtime|--record|--evidence-dir)
      [ "$#" -ge 2 ] || usage
      case "$1" in
        --firstmate-ref) pin=$2 ;; --assets) assets=$2 ;; --local-bin) local_bin=$2 ;;
        --local-runtime) local_runtime=$2 ;; --record) record=$2 ;; --evidence-dir) evidence_dir=$2 ;;
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
for input in "$assets" "$local_bin" "$local_runtime" "$record" "$evidence_dir"; do safe_path "$input"; done
for tool in sandbox-exec git python3 shasum tar; do need "$tool"; done
for input in "$assets" "$local_bin" "$local_runtime" "$record" "$evidence_dir"; do
  [ -z "$input" ] || safe_path "$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$input")"
done
real_home=$(cd "$HOME" && pwd -P)
lab=$(mktemp -d /tmp/zoe-tower-acceptance.XXXXXX)
lab=$(cd "$lab" && pwd -P)
# Only these named outputs leave the lab. Skip symlinks, including a symlinked
# Fleet state directory, so cleanup never follows a lab path into real HOME.
export_evidence() {
  local src rel dest failed=0 count=0
  [ -n "$evidence_dir" ] || return 0
  mkdir -p -- "$evidence_dir" || return 1
  for rel in record.md failure-phase.txt clone.log setup.txt doctor.txt seed.txt \
      snapshot-before.json snapshot-before.err snapshot-after.json snapshot-after.err \
      doctor.err inspect.txt inspect.err phase-b-snapshot.json phase-b-snapshot.err \
      phase-b-inspect.txt phase-b-inspect.err; do
    src="$lab/$rel"
    if [ -f "$src" ] && [ ! -L "$src" ]; then
      tail -c 1048576 -- "$src" >"$evidence_dir/$rel" || failed=1
    fi
  done
  if [ -d "$lab/home/.local/state/zoe-fleet" ] && [ ! -L "$lab/home/.local/state/zoe-fleet" ]; then
    while IFS= read -r -d '' src; do
      [ ! -L "$src" ] || continue
      rel=${src#"$lab/home/.local/state/zoe-fleet/"}
      case "$rel" in */fleet.json|*/fleet.events.jsonl)
        count=$((count + 1))
        [ "$count" -le 20 ] || { failed=1; break; }
        dest="$evidence_dir/fleet/$rel"
        mkdir -p -- "$(dirname "$dest")" || { failed=1; continue; }
        tail -c 1048576 -- "$src" >"$dest" || failed=1 ;;
      esac
    done < <(find "$lab/home/.local/state/zoe-fleet" -type f \( -name fleet.json -o -name fleet.events.jsonl \) -print0)
  fi
  return "$failed"
}
finish() {
  local status=$? export_status=0 failed_phase log
  trap - EXIT
  set +e
  if [ "$status" -ne 0 ]; then
    failed_phase=$(cat "$lab/failure-phase.txt" 2>/dev/null || printf 'staging')
    printf 'phase: %s\nstatus: %s\n' "$failed_phase" "$status" >"$lab/failure-report.txt"
    for log in "$lab/$failed_phase.txt" "$lab/$failed_phase.err"; do
      [ ! -f "$log" ] || [ -L "$log" ] || { printf 'Acceptance: %s failed; last output of %s:\n' "$failed_phase" "${log##*/}" >&2; tail -n 40 -- "$log" >&2; }
    done
  fi
  export_evidence
  export_status=$?
  if [ -n "$evidence_dir" ] && [ -f "$lab/failure-report.txt" ]; then
    cp "$lab/failure-report.txt" "$evidence_dir/failure-report.txt" 2>/dev/null || export_status=1
  fi
  rm -rf -- "$lab"
  [ "$export_status" -eq 0 ] || printf 'Acceptance: evidence export failed: %s\n' "$evidence_dir" >&2
  exit "$status"
}
trap finish EXIT
if [ -n "$evidence_dir" ]; then
  resolved_evidence=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$evidence_dir")
  evidence_dir=''
  case "$resolved_evidence/" in "$lab/"|"$lab/"*) die 'evidence directory is inside the lab' ;; esac
  case "$lab/" in "$resolved_evidence/"*) die 'evidence directory contains the lab' ;; esac
  [ ! -e "$resolved_evidence" ] || [ -z "$(ls -A -- "$resolved_evidence" 2>/dev/null)" ] \
    || die 'evidence directory must be new or empty'
  evidence_dir=$resolved_evidence
fi
phase staging
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
  python3 - "$assets" "${runtime_archive[0]}" "${binary_archive[0]}" <<'PY'
import pathlib, re, sys
assets = pathlib.Path(sys.argv[1]).resolve()
lines = (assets / 'SHA256SUMS').read_text().splitlines()
assert lines, 'empty SHA256SUMS'
listed = set()
for line in lines:
    match = re.fullmatch(r'[0-9a-fA-F]{64} [ *]([^/]+\.tar\.gz)', line)
    assert match, 'unsafe SHA256SUMS entry'
    target = (assets / match.group(1)).resolve()
    assert target.parent == assets, 'asset escapes release directory'
    listed.add(match.group(1))
missing = {pathlib.Path(name).name for name in sys.argv[2:]} - listed
assert not missing, f'SHA256SUMS does not cover {sorted(missing)}'
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
phase sandbox
inner_status=0
(cd "$lab" && env -i HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" XDG_STATE_HOME="$XDG_STATE_HOME" \
  TMPDIR="$TMPDIR" PATH="$PATH" ACCEPTANCE_INSIDE=1 \
  /usr/bin/sandbox-exec -D REAL_HOME="$real_home" \
  -D OWNER_FIRSTMATE="$real_home/Desktop/Agentic/firstmate" \
  -D OWNER_ZOETROPE="$real_home/Desktop/Agentic/zoetrope" \
  -D OWNER_TOOLS="$real_home/Desktop/Agentic/.tools" \
  -f "$script_dir/tower-acceptance.sb" /bin/bash "$lab/runtime/scripts/acceptance-tower.sh" \
  --inside-lab "$lab" "$pin" "$phase_b" "$source_kind" "$real_home") || inner_status=$?
[ "$inner_status" -eq 0 ] || exit "$inner_status"
phase record
mkdir -p "$(dirname "$record")"
cp "$lab/record.md" "$record"
printf 'Acceptance record: %s\n' "$record"
