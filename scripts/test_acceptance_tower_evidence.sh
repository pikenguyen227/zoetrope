#!/usr/bin/env bash
# Exercise the runner's outer EXIT cleanup with synthetic lab outputs. No
# sandbox, Firstmate clone, Herdr, agent, or Phase B shell is started.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
scratch=$(mktemp -d /tmp/zoe-acceptance-evidence-test.XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
# Load the exact cleanup functions used by the runner, without its command body.
sed -n '/^export_evidence() {/,/^trap finish EXIT/p' "$root/scripts/acceptance-tower.sh" | sed '$d' >"$scratch/cleanup.sh"

run_case() (
  lab=$1 evidence_dir=$2 status=$3
  source "$scratch/cleanup.sh"
  trap finish EXIT
  exit "$status"
)
check_status() {
  local expected=$1 actual
  shift
  actual=0
  "$@" || actual=$?
  [ "$actual" -eq "$expected" ] || { echo "expected status $expected, got $actual" >&2; exit 1; }
}

# Failure before record creation: capture clone output and a report, not auth.
lab="$scratch/a-lab"
mkdir -p "$lab/home/.codex" "$scratch/a-evidence"
printf 'clone failed\n' >"$lab/clone.log"
printf 'clone\n' >"$lab/failure-phase.txt"
printf 'secret\n' >"$lab/home/.codex/auth.json"
ln -s "$lab/home/.codex/auth.json" "$lab/doctor.txt"
check_status 17 run_case "$lab" "$scratch/a-evidence" 17
test ! -e "$lab"
test ! -e "$scratch/a-evidence/doctor.txt"
test ! -e "$scratch/a-evidence/home/.codex/auth.json"
grep -q '^phase: clone$' "$scratch/a-evidence/failure-report.txt"
grep -q '^status: 17$' "$scratch/a-evidence/failure-report.txt"
grep -q 'clone failed' "$scratch/a-evidence/clone.log"

# Synthetic Phase B assertion failure after a record and Fleet state exist.
lab="$scratch/b-lab"
state="$lab/home/.local/state/zoe-fleet/fixture"
mkdir -p "$state" "$lab/home/.codex" "$scratch/b-evidence"
printf 'phase-b-assertions\n' >"$lab/failure-phase.txt"
printf 'Phase A passed\n' >"$lab/record.md"
printf '{"tasks":[]}\n' >"$lab/phase-b-snapshot.json"
printf '{"tasks":[]}\n' >"$state/fleet.json"
printf '{"event":"test"}\n' >"$state/fleet.events.jsonl"
printf 'secret\n' >"$lab/home/.codex/auth.json"
check_status 23 run_case "$lab" "$scratch/b-evidence" 23
test ! -e "$lab"
grep -q '^phase: phase-b-assertions$' "$scratch/b-evidence/failure-report.txt"
grep -q '^status: 23$' "$scratch/b-evidence/failure-report.txt"
cmp "$scratch/b-evidence/fleet/fixture/fleet.json" <(printf '{"tasks":[]}\n')
test -f "$scratch/b-evidence/fleet/fixture/fleet.events.jsonl"
test ! -e "$scratch/b-evidence/home/.codex/auth.json"

# A failing step's captured stderr reaches the terminal without --evidence-dir.
lab="$scratch/doctor-lab"
mkdir -p "$lab"
printf 'doctor\n' >"$lab/failure-phase.txt"
printf 'herdr plugin missing\n' >"$lab/doctor.err"
check_status 1 run_case "$lab" '' 1 2>"$scratch/doctor-stderr"
test ! -e "$lab"
grep -q 'herdr plugin missing' "$scratch/doctor-stderr"

# A non-empty evidence directory is refused before staging and left untouched.
mkdir -p "$scratch/stale-evidence"
printf 'old\n' >"$scratch/stale-evidence/failure-report.txt"
check_status 1 "$root/scripts/acceptance-tower.sh" --firstmate-ref "$(printf '0%.0s' {1..40})" \
  --local-bin "$scratch/missing-zoe" --local-runtime "$root" --record "$scratch/record.md" \
  --evidence-dir "$scratch/stale-evidence" 2>"$scratch/stale-stderr"
grep -q 'evidence directory must be new or empty' "$scratch/stale-stderr"
[ "$(ls -A "$scratch/stale-evidence")" = failure-report.txt ]
grep -qx old "$scratch/stale-evidence/failure-report.txt"

# Success retains the record and creates no failure report.
lab="$scratch/success-lab"
mkdir -p "$lab" "$scratch/success-evidence"
printf 'passed\n' >"$lab/record.md"
check_status 0 run_case "$lab" "$scratch/success-evidence" 0
test ! -e "$lab"
grep -q '^passed$' "$scratch/success-evidence/record.md"
test ! -e "$scratch/success-evidence/failure-report.txt"
echo 'acceptance evidence cleanup: passed'
