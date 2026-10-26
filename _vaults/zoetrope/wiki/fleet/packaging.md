---
title: Fleet release packaging
description: How the fork builds target-keyed Fleet archives and keeps the runtime separate.
date: 2026-10-26
---

# Fleet release packaging

The Fleet release script clones the fork at an exact commit and builds both
Darwin targets sequentially with `SOURCE_DATE_EPOCH` from that commit. It checks
the built slices, version stamp, and inspect fixture before staging assets
(`scripts/release-fleet.sh:63`, `scripts/release-fleet.sh:69`,
`scripts/release-fleet.sh:71`, `scripts/release-fleet.sh:79`,
`scripts/release-fleet.sh:81`, `scripts/release-fleet.sh:82`). A build from a
pristine Git checkout matters: `build.rs` appends `-dirty` when Git reports
changes, while a non-checkout build cannot name a commit (`build.rs:1`,
`build.rs:63`, `build.rs:73`).

Each target archive holds `zoe-fleet`, `LICENSE`, a commit-stamped `NOTICE`, and
`BUILD.txt`; the separate architecture-independent runtime keeps
`fleet-plugin/` beside `scripts/` and adds the launcher when it exists
(`scripts/release-fleet.sh:92`, `scripts/release-fleet.sh:108`,
`scripts/release-fleet.sh:123`). `SHA256SUMS` covers every emitted archive and
is checked locally (`scripts/release-fleet.sh:142`). The fixture shell test
exercises layout without starting a viewer or building Rust
(`scripts/test_release_fleet.sh:4`, `scripts/test_release_fleet.sh:13`).

The script never uploads. The approved post-acceptance commit is the release
source, and the separate upload procedure is in
`docs/FLEET-PACKAGING.md:72`. For future Linux/WSL2 builds, the existing CD
matrix shows the musl target names and use of `cross`
(`.github/workflows/cd.yml:29`).
