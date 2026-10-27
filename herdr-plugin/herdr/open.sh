#!/usr/bin/env bash
# Pane command: resolve the session of the pane this was opened from, and run
# `zoe` on it. Every failure is printed here and held until you press enter,
# since this terminal is what the user is looking at.
set -euo pipefail

root="${HERDR_PLUGIN_ROOT:-.}"

die() {
  printf '\n  %s\n\n' "$*" >&2
  read -r -p '  press enter to close ' _ || true
  exit 1
}

# Herdr's persistent server may have started with a minimal PATH; plugin panes
# do not source the interactive shell's startup files. Preserve a PATH-selected
# executable, then try the standard Homebrew/Cargo installation locations.
zoe_bin=$(command -v zoe 2>/dev/null || true)
if [ -z "$zoe_bin" ]; then
  for candidate in /opt/homebrew/bin/zoe /usr/local/bin/zoe "${HOME:-}/.cargo/bin/zoe"; do
    if [ -f "$candidate" ] && [ -x "$candidate" ]; then
      zoe_bin=$candidate
      break
    fi
  done
fi
[ -n "$zoe_bin" ] || die "zoe is not on PATH or in a standard install location (brew install furkankly/tap/zoetrope, or cargo install zoetrope)"

if ! target=$(bash "$root/herdr/resolve.sh" 2>&1); then
  die "$target"
fi

# The pane's agent is running, so ride the live edge; space and the scrubber
# go back.
"$zoe_bin" --provider "${target%% *}" --follow "${target#* }" \
  || die "zoe exited with status $? (zoe --provider ${target%% *} --follow ${target#* })"
