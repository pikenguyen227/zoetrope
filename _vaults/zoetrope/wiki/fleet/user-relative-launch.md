---
title: User-relative Fleet launch
description: How the launcher chooses a Firstmate home, runtime and private state.
date: 2026-10-26
---

# User-relative Fleet launch

`bin/zoe-fleet-tower` resolves its own symlinks to find the Fleet runtime and
offers setup, doctor, open, inspect and a synthetic demo
(`bin/zoe-fleet-tower:4`, `bin/zoe-fleet-tower:25`). Setup requires a Firstmate
snapshot executable, creates its directories under `umask 077`, and writes a
mode-0600 config under the user's XDG config home (`bin/zoe-fleet-tower:43`,
`bin/zoe-fleet-tower:60`, `bin/zoe-fleet-tower:64`). See `docs/FLEET-USAGE.md`
for commands.

The shared config reader treats `KEY=VALUE` as data, lets environment values
win, and refuses an unset `FM_HOME` (`scripts/fleet-config.sh:7`,
`scripts/fleet-config.sh:13`, `scripts/fleet-config.sh:30`). Recorded state and
telemetry paths apply only when the effective `FM_HOME` resolves to the
configured home, so an `FM_HOME` override never reuses another home's manifest
or journal (`scripts/fleet-config.sh:39`). Otherwise the state path is under the
user's XDG state home, keyed by the first 12 SHA-256 hex digits of the resolved
Firstmate home; telemetry lives in that home's state subtree
(`scripts/fleet-config.sh:38`, `scripts/fleet-config.sh:47`,
`scripts/fleet-config.sh:50`). The Claude bridge follows the same home-scoped
store and creates its parents privately (`scripts/claude-telemetry.py:20`).

The separate state is necessary because a Fleet manifest records the home in
its ID and rejects a previous manifest from a different home
(`scripts/firstmate-fleet.py:190`, `scripts/firstmate-fleet.py:193`). Collection
needs that home's snapshot command and a real Herdr pane
(`scripts/firstmate-fleet.py:544`, `scripts/firstmate-fleet.py:2185`).
The adapter uses Python 3.9 syntax, including dictionary union and an
assignment expression (`scripts/firstmate-fleet.py:555`,
`scripts/firstmate-fleet.py:959`).

The developer installer delegates setup to the launcher. It recognizes the
older sibling-home layout once and keeps its existing binary and state paths
when it first writes config (`scripts/install-fleet.sh:7`,
`scripts/install-fleet.sh:40`). This migration is specific to the developer
wrapper; the runtime launcher and plugin have no sibling-home default.
