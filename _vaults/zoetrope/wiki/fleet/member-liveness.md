---
title: Member liveness and Herdr runtime
description: How a fleet card reads active or idle, and how Herdr's pane status overrides transcript recency.
date: 2026-09-26
---

# Member liveness and Herdr runtime

A fleet card's spinner is its session's `main` agent status. By default `main`
is `Running` while its transcript recorded something within
`INTERACTIVE_IDLE_SECS` (120s) or holds a pending tool call
(`src/state/session.rs:22`, `src/state/session.rs:754`).

Herdr's view of the member's pane overrides that. On every collection (default
cadence 5s, `scripts/firstmate-fleet.py:1962`) the adapter copies the task's own
pane's `agent_status` into the task's `runtime`, source `herdr.pane.get`, for
the session the task is already joined to, whatever session the pane registers
now and even while the join is deferred, provided the pane was read at the
task's endpoint in both snapshots (`scripts/firstmate-fleet.py:253`). The viewer
turns the newest such runtime among the tasks joined to a session into a
`Sighting` (`src/fleet/mod.rs:319`, stored as `App::seen`,
`src/state/mod.rs:240`):

- `working`: `main` is active for 120s after the observation however quiet its
  transcript (`src/state/session.rs:787`). Each collection renews it; a
  collector that stops observing does not pin a card active.
- `done` or `idle`: `main` settles idle at once, until it records anything
  later (`src/state/session.rs:785`).

This holds at every level of the crew, primary worker, secondmate and
secondmate's worker, because it keys on the session, not the task's kind
(regression tests `src/fleet/tests.rs:714`, `src/fleet/tests.rs:841`).

## Relaunched panes

A worker relaunched in place starts a new session that the member is never
joined to, so only the pane's Herdr runtime shows it working, whether Herdr
keeps the pane's old registration or re-registers it. `docs/FIRSTMATE-FLEET.md`
("UI and performance") owns this contract; the adapter cases are tested in
`scripts/test_firstmate_fleet.py:72`, `:86` and `:95`.
