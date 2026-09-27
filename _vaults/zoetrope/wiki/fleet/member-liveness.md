---
title: Member liveness and Herdr runtime
description: How a fleet card reads active or idle, and how Herdr's pane status overrides transcript recency.
date: 2026-09-27
---

# Member liveness and Herdr runtime

Updated 2026-09-27: ownership and activity also cover waiting cards.

A fleet card's spinner is its session's `main` agent status. By default `main`
is `Running` while its transcript recorded something within
`INTERACTIVE_IDLE_SECS` (120s) or holds a pending tool call
(`src/state/session.rs:22`, `src/state/session.rs:763`).

Herdr's view of the member's pane overrides that. On every collection (default
cadence 5s, `scripts/firstmate-fleet.py:1975`) the adapter copies the task's own
pane's `agent_status` into the task's `runtime`, source `herdr.pane.get`,
even without a joined session or while the join is deferred, provided the
pane was read at the
task's endpoint in both snapshots (`scripts/firstmate-fleet.py:253`). The viewer
turns the newest such runtime among the tasks joined to a session into a
`Sighting` (`src/fleet/mod.rs:355`, stored as `App::seen`,
`src/state/mod.rs:240`):

- `working`: `main` is active for 120s after the observation however quiet its
  transcript (`src/state/session.rs:160`). Each collection renews it; a
  collector that stops observing does not pin a card active.
- `done` or `idle`: `main` settles idle at once, until it records anything
  later (`src/state/session.rs:791`).

This holds at every level of the crew, primary worker, secondmate and
secondmate's worker, because it keys on the session, not the task's kind
(regression tests `src/fleet/tests.rs:714`, `src/fleet/tests.rs:841`).

## Relaunched panes

A worker relaunched in place starts a new session that the member is never
joined to, so only the pane's Herdr runtime shows it working, whether Herdr
keeps the pane's old registration or re-registers it. `docs/FIRSTMATE-FLEET.md`
("UI and performance") owns this contract; the adapter cases are tested in
`scripts/test_firstmate_fleet.py:72`, `:86` and `:95`.

## Waiting cards and ownership

A secondmate home's task list proves delegation before either endpoint has a
session. The collector records `delegates` between `(task, spawn_gen)` attempts;
the viewer accepts both these and older session endpoints
(`scripts/firstmate-fleet.py:334`, `src/fleet/mod.rs:140`). It resolves endpoints
to visible waiting or session cards, and gives unbound direct reports a home
root membership edge (`src/fleet/mod.rs:922`, `src/fleet/mod.rs:971`). A member
draws only the delegator latest observed by the playhead, so each secondmate
attempt keeps its own link and time (`src/fleet/mod.rs:939`). A session join changes the displayed endpoint without changing the assignment; missing
provisional generations and their edges are replaced together
(`scripts/firstmate-fleet.py:354`, `scripts/test_firstmate_fleet.py:653`).

A card awaiting a session or transcript uses its own pane's runtime with the
same working window; done/idle settles idle, and replay never borrows today's
runtime (`src/fleet/mod.rs:747`, `src/fleet/mod.rs:839`,
`src/state/session.rs:160`). The regression covers both providers, both ends
unbound, subsequent binding, parent visibility, and liveness
(`src/fleet/tests.rs:919`). Native session identity still requires matching
Herdr registration; lack of a hook can leave that identity unknown without
orphaning the worker (`scripts/firstmate-fleet.py:232`).
