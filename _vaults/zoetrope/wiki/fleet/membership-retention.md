---
title: Membership retention and the viewer's limits
description: Why a long-running collector outgrew the 128-session limit, how finished members now age out, and how the viewer degrades past the limits.
date: 2026-09-29
---

# Membership retention and the viewer's limits

## What filled the fleet

`build_manifest` seeds its sessions, attempts and links from the previous
manifest and marks every carried attempt `not observed` until a snapshot lists
it again (`scripts/firstmate-fleet.py:187`, `scripts/firstmate-fleet.py:191`).
A collector recovers that previous manifest on every start, from the manifest
file or the journal's latest checkpoint (`scripts/firstmate-fleet.py:1695`,
`scripts/firstmate-fleet.py:2058`), so nothing ever left. On 2026-09-29 the
Team tab's manifest held 146 sessions (138 of 151 attempts `not observed`,
only 13 sessions with an observed task) while a fresh collection yielded 13,
and the viewer refused it: the limits are 128 sessions, 4096 attempts and
4096 links (`src/fleet/mod.rs:30`).

## Retention at the source

Every build calls `retire` (`scripts/firstmate-fleet.py:363`,
`scripts/firstmate-fleet.py:376`). A finished member is an attempt no snapshot
lists, or a Captain a later one replaced (`scripts/firstmate-fleet.py:297`).
Each keeps the moment it left as its runtime's `observed_at` rather than the
latest poll's, and stays at most `RETAIN_SECONDS` (24h) from then, and only
among the `RETAIN_FINISHED` (32) most recent (`scripts/firstmate-fleet.py:55`).
A manifest from before this rule has every departure at one moment; the last
Firstmate state time breaks that tie. Workers of a secondmate whose crew is
unread this poll are held, with the secondmate attempt that owns them
(`scripts/firstmate-fleet.py:362`). Sessions go with
the last attempt that named them, and links with either end. The journal is
untouched, so history stays scrubbable.

Tests: `scripts/test_firstmate_fleet.py:143`, `:163`, `:845`.

## Degrading in the viewer

`Manifest::fit` (`src/fleet/mod.rs:215`) runs before validation on load and on
every refresh (`src/fleet/mod.rs:203`, `src/fleet/mod.rs:585`,
`src/fleet/mod.rs:621`). Over a limit it keeps the most recent members,
ranking tasks still observed first, then the newest to leave, then the newest
state, with the standing Captain first. It then drops dangling tasks, sessions
and links, and adds a first diagnostic that the header shows. `validate` still
refuses an over-limit manifest by itself (`src/fleet/tests.rs:1217`).

The journal (`fleet.events.jsonl`) grew to 139 MB by 2026-09-29; its retention
is in [Journal retention](journal-retention.md).
