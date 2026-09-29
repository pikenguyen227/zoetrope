---
title: Fleet viewer CPU
description: What keeps the fleet viewer busy per poll and per frame, and which costs scale with history rather than activity.
date: 2026-09-30
---

# Fleet viewer CPU

The fleet viewer runs two clocks whose cost must not grow with history.

**Per poll (every member).** Each member's tailer polls every 200 ms
(`src/tailer/live.rs:27`) and rescans the session's files on every tick
(`src/tailer/live.rs:407`, `src/provider/mod.rs:210`). For a Codex root the
rescan names only the day directories between the root's day and the day of
its last write (`src/provider/codex/discovery.rs:259`,
`src/provider/codex/discovery.rs:285`). It walks the whole
`~/.codex/sessions` tree (`src/provider/codex/discovery.rs:66`) only for a
child file or a span over `MAX_NAMED_DAYS`
(`src/provider/codex/discovery.rs:306`). The full walk per tick cost ~55% of
a core with ten Codex members over a 104-directory tree.

**Per frame.** The main loop draws every 16 ms (`src/fleet/native.rs:548`),
and the footer asks for the newest lifecycle event at the playhead
(`src/fleet/native.rs:776`). `latest_at` binary-searches the moment and walks
back (`src/fleet/journal.rs:1114`). A forward walk over a ~20k-event journal
held the main thread busy, because each step through `ordered` is a
string-keyed map lookup (`src/fleet/journal.rs:917`).

**Still linear, on sync only.** `state_at` and `validation_at` fold every event
up to the moment (`src/fleet/journal.rs:1040`, `src/fleet/journal.rs:986`).
They run on `Fleet::sync`, when something changed or at least once a second,
not per frame; they are the next cost to index as the journal grows.

Measured on a copy of a 49-member, 20.8k-event fleet: ~92% of a core before,
~12% after, most of the remainder being the fixed per-frame draw.
