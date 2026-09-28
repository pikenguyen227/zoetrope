---
title: Quota sources and empty Codex buckets
description: Quota extraction, replay persistence, and the empty-bucket regression.
date: 2026-09-28
---

# Quota sources and empty Codex buckets

Codex quota observations come from transcript `token_count.rate_limits` records,
decoded at the provider boundary, not the Firstmate manifest collector. The
provider requires at least one window with both a duration and used percentage
before emitting a quota fact (`src/provider/codex/mod.rs:242`). Claude has a
separate status-line bridge that writes `zoe.usage/v1` snapshots with the `claude`
bucket (`scripts/claude-telemetry.py:23`); the native fleet telemetry reader only
loads them for current Claude members (`src/fleet/native.rs:864`).

Replay folds quota facts from the whole transcript into session metadata
(`src/tailer/replay.rs:124`). Each bucket keeps its latest observation
(`src/state/info.rs:30`). The footer takes the newest observation per
provider/bucket across current manifest members, excluding retained history;
different bucket names are not deduplicated, and age labels do not expire rows
(`src/fleet/native.rs:919`, `src/usage.rs:255`). Thus an old record can continue
to appear when its session remains in the manifest.

The September 28 regression reduced a September 22 `premium` record with null
primary/secondary windows to a fixture. Previously the provider emitted an empty
quota for it, leaving a separate `codex/premium` row beside the real `codex`
quota. The replay test reproduces the exact `stale 8458m ago` age before the fix;
with the extraction guard, only the actual quota row remains
(`src/fleet/native.rs:1100`, `src/provider/codex/mod.rs:257`).

The disproof control supplies a real window under the same `premium` name and
requires two rows. This rules out treating that name as an alias or filtering it
from the UI (`src/fleet/native.rs:1144`). Run
`cargo test empty_codex_bucket_does_not_survive_replay_as_a_stale_quota_row`.
Token records with incomplete windows still contribute usage; an empty later
record cannot replace an earlier real observation
(`src/provider/codex/mod.rs:590`).
