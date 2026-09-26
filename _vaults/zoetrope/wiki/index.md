---
title: zoetrope knowledge vault
description: Entry point and rules for zoetrope's compiled, file-cited project knowledge.
date: 2026-09-26
---

# zoetrope knowledge vault

zoetrope is a terminal UI (and wasm browser frontend) that draws Claude Code and
Codex sessions as a live flow graph; this fork adds a Firstmate fleet mode. The
source is this repository: the library and `zoe` binary under `src/`, the
browser frontend under `web/wasm/`, the Firstmate fleet adapter under `scripts/`.

## Operating mode

Notes are compiled just in time: a task that needs a piece of knowledge reads
the live code and writes one focused, file-cited note answering it, or updates
the existing note (marked `Updated YYYY-MM-DD`). Every note is listed in its
folder's `index.md`. Wiki notes never link into `raw/`, which is untracked.

## Authority order

When sources disagree: live code (`src/`, `scripts/`, `web/wasm/src/`) wins,
then the design docs in `docs/` (`ARCHITECTURE.md`, `DESIGN.md`,
`FIRSTMATE-FLEET.md`, `FLEET-USAGE.md`), then these wiki notes, then session
memory. Volatile detail (types, values) lives by pointer to its source file.

## Taxonomy

| Folder | Holds |
| --- | --- |
| [fleet/](fleet/index.md) | Firstmate fleet mode: adapter joins, member liveness, Herdr runtime |

## Note conventions

- Frontmatter with `title`, `description`, `date`.
- Relative Markdown links between notes; a link to an unwritten note marks a gap.
- Every claim cites `path:line`. A claim not verified in code is marked
  `UNVERIFIED`.
