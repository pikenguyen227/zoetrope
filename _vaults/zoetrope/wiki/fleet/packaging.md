---
title: Fleet release packaging
description: How the fork builds target-keyed Fleet archives and keeps the runtime separate.
date: 2026-10-27
---

# Fleet release packaging

Updated 2026-10-27.

The Fleet release script clones the fork at an exact commit and builds selected
Darwin and Linux targets sequentially with `SOURCE_DATE_EPOCH` from that commit.
Darwin uses host Rust; Linux uses ephemeral Ubuntu 22.04 Docker containers
pinned by digest, with the host's Rust toolchain version. It checks architecture,
version stamp, and inspect fixture before staging assets
(`scripts/release-fleet.sh:23`, `scripts/release-fleet.sh:103`,
`scripts/release-fleet.sh:108`, `scripts/release-fleet.sh:117`,
`scripts/release-fleet.sh:140`, `scripts/release-fleet.sh:163`). A build from a
pristine Git checkout matters: `build.rs` appends `-dirty` when Git reports
changes, while a non-checkout build cannot name a commit (`build.rs:1`,
`build.rs:63`, `build.rs:73`).

Each target archive holds `zoe-fleet`, `LICENSE`, a commit-stamped `NOTICE`, and
`BUILD.txt`; the separate architecture-independent runtime keeps
`fleet-plugin/` beside `scripts/` and adds the launcher when it exists
(`scripts/release-fleet.sh:184`, `scripts/release-fleet.sh:208`,
`scripts/release-fleet.sh:224`). Linux `BUILD.txt` records the glibc 2.35 floor
(`scripts/release-fleet.sh:153`, `scripts/release-fleet.sh:195`). `SHA256SUMS`
covers only emitted archives and is checked locally; a Linux-only target
selection emits no runtime or Darwin archive (`scripts/release-fleet.sh:208`,
`scripts/release-fleet.sh:244`). The fixture tests all four binaries and the
Linux-only subset without building Rust (`scripts/test_release_fleet.sh:10`,
`scripts/test_release_fleet.sh:16`, `scripts/test_release_fleet.sh:44`).

The script never uploads. The approved post-acceptance commit is the release
source, and the separate upload procedure is in
`docs/FLEET-PACKAGING.md:96`. The fork's general CD matrix still builds musl
targets with `cross`; those are separate from the fleet linux-gnu archives
(`.github/workflows/cd.yml:29`, `scripts/release-fleet.sh:23`).
