---
title: Member liveness and Herdr runtime
description: How a fleet card reads active or idle, and how Herdr's pane status overrides transcript recency.
date: 2026-09-26
---

# Member liveness and Herdr runtime

A fleet card's spinner is its session's `main` agent status. By default `main`
is `Running` while its transcript recorded something within
`INTERACTIVE_IDLE_SECS` (120s) or holds a pending tool call
(`src/state/session.rs:22`, `src/state/session.rs:753`).

Herdr's view of the member's pane overrides that. The adapter copies the pane's
`agent_status` into the task's `runtime`, source `herdr.pane.get`, on every
collection (`scripts/firstmate-fleet.py:251`; default cadence 5s,
`scripts/firstmate-fleet.py:1957`). The viewer turns the newest such runtime
among the tasks joined to a session into a `Sighting`
(`src/fleet/mod.rs:319`, stored as `App::seen`, `src/state/mod.rs:240`):

- `working`: `main` is active for 120s after the observation however quiet its
  transcript (`src/state/session.rs:786`). Each collection renews it; a
  collector that stops observing does not pin a card active.
- `done` or `idle`: `main` settles idle at once, until it records anything
  later (`src/state/session.rs:784`).

This holds at every level of the crew, primary worker, secondmate and
secondmate's worker, because it keys on the session, not the task's kind
(regression test `src/fleet/tests.rs:714`).

## Stale session registration

The adapter joins a task to the session Herdr registers for its pane
(`agent_session`, `scripts/firstmate-fleet.py:64`). When Firstmate relaunches a
worker in place (a control relaunch in the same pane), Claude Code starts a new
session, but Herdr was seen on 2026-09-26 still registering the pane's first
session. The card is then joined to a transcript that has stopped growing, so
only the Herdr runtime shows it working. The adapter does not detect this: the
session did not change from its point of view, so the
`session changed without a new spawn generation` diagnostic
(`scripts/firstmate-fleet.py:240`) never fires. Whether Herdr ever re-registers
on relaunch is `UNVERIFIED`.
