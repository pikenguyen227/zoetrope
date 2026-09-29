---
title: Slow refreshes and lifecycle coverage
description: Why snapshot latency starves coverage and which visual symptoms have other sources.
date: 2026-09-29
---

# Slow refreshes and lifecycle coverage

Updated 2026-09-29.

The snapshot join brackets pane reads with two observations; secondmate homes
are already read concurrently within each phase. The initial primary snapshot
must finish before it can discover those homes (`scripts/firstmate-fleet.py:513`,
`scripts/firstmate-fleet.py:522`, `scripts/firstmate-fleet.py:545`). Increasing
parallelism alone does not remove the full-refresh dependency.

`while_refreshing` permits one join in flight while its caller reads previously
discovered durable feeds. It leaves journal writes on the calling thread. Live
feed reads use read time, while bridge observations retain the snapshot's own
time. A failed refresh keeps the previous manifest timestamp and diagnostics;
its next attempt can still read the known feeds
(`scripts/firstmate-fleet.py:1664`, `scripts/firstmate-fleet.py:1677`,
`scripts/firstmate-fleet.py:2095`). Missing previously observed feed files cannot
extend a saved cursor's coverage (`scripts/firstmate-fleet.py:954`).

The behavioral regressions run a real subprocess with short and sufficient
deadlines, hold one refresh while heartbeats continue, exercise timeout and
recovery through `main`, and verify that missing/unreadable feeds and cached
snapshot facts gain no coverage (`scripts/test_firstmate_fleet.py:1593`).

## Distinguishing the screenshot symptoms

The coverage warning is about lifecycle history (`src/fleet/native.rs:725`),
not the node's activity glyph: `◌` means idle, and even a running node pulses
with a hollow `○` (`src/state/session.rs:119`, `src/ui/nodes.rs:214`). Herdr
`done`/`idle` readings settle activity; coverage does not set those readings
(`src/fleet/mod.rs:449`).

Green animated edges mean the target is running. A current delegation comes
from the latest observed ownership link, while existing node positions remain
stable until an explicit arrangement. A child visually below a different
root can still have the correct edge (`src/fleet/mod.rs:1055`,
`src/fleet/mod.rs:1137`, `src/fleet/mod.rs:1175`). Check endpoints before calling
that a parentage bug.

Codex quota is independent transcript telemetry, not snapshot data. The footer
uses the newest quota per bucket across current members, keeping its recorded
observation age (`src/fleet/native.rs:921`). See
[quota sources](quota-sources.md) for the separate empty-bucket regression.
