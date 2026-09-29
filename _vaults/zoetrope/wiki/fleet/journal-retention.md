---
title: Journal retention
description: Why the fleet journal grew to 139 MB, what compaction keeps, and how the viewer survives the rewrite.
date: 2026-09-29
---

# Journal retention

`fleet.events.jsonl` held two kinds of line, and only one of them is history.
A manifest checkpoint repeats the whole manifest each time it changes
(`publish`, `scripts/firstmate-fleet.py:1786`), so on a real fleet checkpoints were
127 of 139 MB after a week, about 64 KB each, while every lifecycle line
together was 9 MB. Yet only the newest checkpoint is ever read: `recover` takes
the newest recoverable one (`scripts/firstmate-fleet.py:1664`), and the viewer
parses checkpoints as `Ok(None)` and skips them (`src/fleet/journal.rs:565`).
Lifecycle lines are the timeline's history and the collector's restart state
(`Bridge.recover` and `Validation.recover`, `scripts/firstmate-fleet.py:592`
and `:1424`).

So retention is compaction, not rotation or an age cut: `compact_journal`
(`scripts/firstmate-fleet.py:1729`) keeps every lifecycle line and every line it
does not know, byte for byte and in order, and of the checkpoints only the one
`recover` would pick (`recoverable`, `:1643`, shared with `recover`). On a copy
of the live journal this took 139 MB to 9 MB in under a second, with `recover`
unchanged.

`retain` (`:1768`) checks when the collector starts (`:2072`) and after every
poll (`:2115`), and runs it once the journal is past `COMPACT_BYTES` (16 MiB,
`:56`) and twice what the last compaction left, so rewrite cost stays
proportional to what was appended. The startup check passes `compacted = 0`
(nothing left yet), so at start it compacts only past 16 MiB. The rewrite goes
through `write_atomic` (`:1716`): temp file beside the journal, fsync, rename,
under the collector's lock.

## The viewer across a rewrite

`JournalTail::poll` reopens the journal every read and keeps a byte offset
(`src/fleet/journal.rs:1177`). Before this change it detected a replaced file
only when the file shrank or the byte before its offset was no longer a
newline. A compacted file is usually far shorter, but one that dropped little
and then gained appends could be as long as the old offset and end in a newline
there, and the tail would silently read on from the wrong place. It now also
compares the file's device and inode (`identity`, `src/fleet/journal.rs:1140`,
compared at `:1195`), and on a reset `Fleet::absorb_lifecycle` drops what it
held before taking the re-read (`src/fleet/mod.rs:596`). The compacted file's
lifecycle lines are the same, so the view does not change.
