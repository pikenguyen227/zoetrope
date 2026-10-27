---
title: Isolated fleet tower acceptance
description: How the fork checks a fresh Firstmate home without touching live fleet state.
date: 2026-10-26
---

# Isolated fleet tower acceptance

Updated 2026-10-27.

The acceptance runner stages a chosen binary and runtime, or checksum-verified
release assets, in a throwaway lab before entering `sandbox-exec`
(`scripts/acceptance-tower.sh:189`, `scripts/acceptance-tower.sh:281`,
`scripts/acceptance-tower.sh:349`). Its profile denies file reads and writes
under the real home, with explicit rules for the live Agentic locations
(`scripts/tower-acceptance.sb:4`). The sandbox process receives only lab HOME,
XDG directories, TMPDIR and a minimal PATH (`scripts/acceptance-tower.sh:340`,
`scripts/acceptance-tower.sh:349`).

Phase A clones Firstmate from GitHub at the requested pin, creates the fresh
home layout, configures the launcher, seeds one registered secondmate without
an agent, compares snapshots and inspects the bundled Fleet fixture
(`scripts/acceptance-tower.sh:57`, `scripts/acceptance-tower.sh:66`,
`scripts/acceptance-tower.sh:74`, `scripts/acceptance-tower.sh:76`,
`scripts/acceptance-tower.sh:99`). Its Herdr executable is only an inert
`doctor` probe (`scripts/acceptance-tower.sh:325`). The generated Markdown
record is [the local Phase A example](../../../../docs/FLEET-ACCEPTANCE-RECORD.md);
it records a local build, not uploaded release verification.

Phase B is a separate human-invoked path that prompts for a lab Herdr pane and
one agent, then checks the collected manifest against the Firstmate snapshot,
checks home/file paths stay in the lab, and scans the manifest and event
journal for `Desktop/Agentic` or real-home paths, requiring exactly the
`lab-mate` task (`scripts/acceptance-tower.sh:129`,
`scripts/acceptance-tower.sh:143`, `scripts/acceptance-tower.sh:147`,
`scripts/acceptance-tower.sh:155`, `scripts/acceptance-tower.sh:175`).
It was not run in the Phase A example; release and parity requirements live
in `docs/FLEET-PACKAGING.md:102`.

The outer exit cleanup accepts `--evidence-dir` and copies only named,
size-bounded diagnostics before deleting the lab. It records the failure phase
and status even when `record.md` was never created; `--record` is success-only
(`scripts/acceptance-tower.sh:221`, `scripts/acceptance-tower.sh:249`,
`scripts/acceptance-tower.sh:358`). The evidence directory must pass the
runner's live-path checks, be new or empty, and sit outside the lab; it is
armed only after those checks pass
(`scripts/acceptance-tower.sh:211`, `scripts/acceptance-tower.sh:270`).
