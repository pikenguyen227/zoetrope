---
title: Isolated fleet tower acceptance
description: How the fork checks a fresh Firstmate home without touching live fleet state.
date: 2026-10-26
---

# Isolated fleet tower acceptance

Updated 2026-10-27.

The acceptance runner stages a chosen binary and runtime, or checksum-verified
release assets, in a throwaway lab before entering `sandbox-exec`
(`scripts/acceptance-tower.sh:299`, `scripts/acceptance-tower.sh:375`). Its
profile denies file reads and writes under the real home, with explicit rules
for the live Agentic locations (`scripts/tower-acceptance.sb:4`). The sandbox
process receives only lab HOME, XDG directories, TMPDIR and a minimal PATH
(`scripts/acceptance-tower.sh:368`, `scripts/acceptance-tower.sh:377`).

Phase A clones Firstmate from GitHub at the requested pin, creates the fresh
home layout, configures the launcher, seeds one registered secondmate without
an agent, compares snapshots and inspects the bundled Fleet fixture
(`scripts/acceptance-tower.sh:59`, `scripts/acceptance-tower.sh:67`,
`scripts/acceptance-tower.sh:76`, `scripts/acceptance-tower.sh:78`,
`scripts/acceptance-tower.sh:100`). Its Herdr executable is only an inert
`doctor` probe (`scripts/acceptance-tower.sh:345`). The generated Markdown
record is [the local Phase A example](../../../../docs/FLEET-ACCEPTANCE-RECORD.md);
it records a local build, not uploaded release verification.

Phase B is a separate human-invoked path for a dedicated macOS account whose
own default Herdr session is empty. The outer preflight requires an explicit
acknowledgement, verifies the login HOME, rejects known Fleet/Firstmate state,
checks Herdr against the plugin's minimum version, and reads session,
workspace, and pane lists before lab staging
(`scripts/acceptance-tower.sh:228`, `scripts/tower-phase-b-preflight.sh:8`,
`scripts/tower-phase-b-preflight.sh:19`, `scripts/tower-phase-b-preflight.sh:26`,
`scripts/tower-phase-b-preflight.sh:36`). Phase B stages native Herdr and Codex
binaries into the lab; its Herdr command injects the lab workload environment
into workspace, tab and plugin pane creation, including Firstmate's spawned
mate pane, and runs the lab `pane.sh` itself for the Fleet open action so the
collector tab gets the lab environment (`scripts/acceptance-tower.sh:354`,
`scripts/tower-phase-b-herdr.sh:7`, `scripts/tower-phase-b-herdr.sh:10`).
Before the lab shell opens, an in-sandbox `herdr session list` must reach the
account's Herdr API socket (`scripts/acceptance-tower.sh:137`).
The owner guide and stop order are in `docs/FLEET-PACKAGING.md:141`. The runner
then checks the collected manifest against a fresh Firstmate snapshot and
requires only `lab-mate`, with lab-only paths
(`scripts/acceptance-tower.sh:151`, `scripts/acceptance-tower.sh:155`).
The real Phase B has not been run; the preflight and wrapper are covered only
by stub tests (`scripts/test_tower_phase_b_preflight.sh:1`).

The outer exit cleanup accepts `--evidence-dir` and copies only named,
size-bounded diagnostics before deleting the lab. It records the failure phase
and status even when `record.md` was never created; `--record` is success-only
(`scripts/acceptance-tower.sh:242`, `scripts/acceptance-tower.sh:269`,
`scripts/acceptance-tower.sh:386`). The evidence directory must pass the
runner's live-path checks, be new or empty, and sit outside the lab; it is
armed only after those checks pass
(`scripts/acceptance-tower.sh:224`, `scripts/acceptance-tower.sh:290`).
