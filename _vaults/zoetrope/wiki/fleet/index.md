---
title: Fleet notes
description: Index of notes on zoetrope's Firstmate fleet mode.
date: 2026-09-26
---

# Fleet notes

- [Member liveness and Herdr runtime](member-liveness.md): how a fleet card
  decides active or idle, and why Herdr's pane status overrides a quiet or stale
  transcript.
- [Quota sources and empty Codex buckets](quota-sources.md): why a historical
  bucket-only record produced an extra stale footer row and where it is rejected.
- [Membership retention and the viewer's limits](membership-retention.md): why
  the Team tab hit the 128-session limit, how finished members age out, and how
  the viewer shows the most recent past the limits.
- [Slow refreshes and lifecycle coverage](slow-refresh.md): keeping durable-feed reads independent of slow snapshots, and distinguishing activity glyphs, ownership layout and quota age.
- [Journal retention](journal-retention.md): why the journal is compacted
  rather than rotated, what stays, and how the viewer follows the rewrite.
