---
title: Isolated fleet tower acceptance
description: How the fork checks a fresh Firstmate home without touching live fleet state.
date: 2026-10-26
---

# Isolated fleet tower acceptance

The acceptance runner stages a chosen binary and runtime, or checksum-verified
release assets, in a throwaway lab before entering `sandbox-exec`
(`scripts/acceptance-tower.sh:183`, `scripts/acceptance-tower.sh:202`,
`scripts/acceptance-tower.sh:263`). Its profile denies file reads and writes
under the real home, with explicit rules for the live Agentic locations
(`scripts/tower-acceptance.sb:4`). The sandbox process receives only lab HOME,
XDG directories, TMPDIR and a minimal PATH (`scripts/acceptance-tower.sh:250`,
`scripts/acceptance-tower.sh:263`).

Phase A clones Firstmate from GitHub at the requested pin, creates the fresh
home layout, configures the launcher, seeds one registered secondmate without
an agent, compares snapshots and inspects the bundled Fleet fixture
(`scripts/acceptance-tower.sh:54`, `scripts/acceptance-tower.sh:60`,
`scripts/acceptance-tower.sh:68`, `scripts/acceptance-tower.sh:74`,
`scripts/acceptance-tower.sh:89`). Its Herdr executable is only an inert
`doctor` probe (`scripts/acceptance-tower.sh:241`). The generated Markdown
record is [the local Phase A example](../../../../docs/FLEET-ACCEPTANCE-RECORD.md);
it records a local build, not uploaded release verification.

Phase B is a separate human-invoked path that prompts for a lab Herdr pane and
one agent, then checks the collected manifest against the Firstmate snapshot,
checks home/file paths stay in the lab, and scans the manifest and event
journal for owner paths (`scripts/acceptance-tower.sh:118`,
`scripts/acceptance-tower.sh:131`, `scripts/acceptance-tower.sh:139`,
`scripts/acceptance-tower.sh:144`, `scripts/acceptance-tower.sh:155`).
It was not run in the Phase A example; release and parity requirements live
in `docs/FLEET-PACKAGING.md:102`.
