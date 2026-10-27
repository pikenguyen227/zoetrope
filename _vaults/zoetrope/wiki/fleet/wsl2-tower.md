---
title: Fleet tower on Windows via WSL2 Ubuntu
description: Why the tower runs inside WSL2 Ubuntu, the Linux-filesystem rule, prerequisites and the acceptance label.
date: 2026-10-27
---

# Fleet tower on Windows via WSL2 Ubuntu

The agent-kit wants the tower on Windows. It runs inside WSL2 Ubuntu under
Herdr for Linux, using the linux-gnu `zoe-fleet` archives from the sibling
release `fleet-v0.2.0-fc45c4d9ea1d-linux`; the runtime archive comes from the
original release. Standalone Linux desktops and servers are out of scope: not
claimed, not tested. User steps are in `docs/FLEET-USAGE.md`, section "Windows
(WSL2 Ubuntu)".

Status label: Windows via WSL2 Ubuntu; arm64 hands-on acceptance pending on the
owner's Parallels VM; x64 unconfirmed until a teammate x64 PC runs it.

## Why WSL2, not native Windows

The Herdr plugin commands are `bash` (`fleet-plugin/herdr-plugin.toml:12`,
`fleet-plugin/herdr-plugin.toml:18`) and Herdr's Windows page says bash plugin
commands need Windows alternatives. Both plugin manifests already declare
`platforms = ["macos", "linux"]` (`fleet-plugin/herdr-plugin.toml:5`).

## Rules that follow from the code

- Keep every tower path on the Linux filesystem. The adapter takes an
  `fcntl.flock` lock (`scripts/firstmate-fleet.py:1922`) and the launcher writes
  a mode 0600 config (`bin/zoe-fleet-tower:71`); `/mnt/c` DrvFS does not honour
  either reliably. UNVERIFIED on a real WSL2 host.
- Prerequisites: Python 3.9 or newer (`bin/zoe-fleet-tower:88`), `jq` for the
  Firstmate snapshot, and Herdr 0.9.1 or newer (`fleet-plugin/herdr-plugin.toml:6`).
  `sha256sum` is always present; `shasum` needs the `perl` package.
- The home hash uses `python3` (`bin/zoe-fleet-tower:49`).
- Build floor: binaries are built on Ubuntu 22.04, glibc 2.35
  (`docs/FLEET-PACKAGING.md`, section "Windows via WSL2 Ubuntu binaries");
  see [packaging](packaging.md).

## Unverified WSL facts

Herdr socket at `~/.config/herdr/herdr.sock`, the WSL VM and Herdr server
stopping when the last Ubuntu terminal closes, and nested virtualization in
Parallels are all UNVERIFIED until the owner's VM run records them.

The acceptance runner stays macOS-only (see [acceptance](acceptance.md)); WSL2
is checked by the manual checklist instead.
