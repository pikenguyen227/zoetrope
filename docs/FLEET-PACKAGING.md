# Firstmate fleet release packaging

`zoetrope` owns the Fleet manifest producer and the release assets. The agent-kit
pins an approved commit and consumes the archives; its installer does not run the
producer or open the viewer. The release script only builds local assets. Creating
a GitHub tag or release is a separate, explicit act after approval of the
post-acceptance commit.

## Producer contract

The runtime archive keeps `fleet-plugin/` beside `scripts/`. In a Herdr pane,
`fleet-plugin/open.sh` invokes `scripts/firstmate-fleet.py` with `--home <FM_HOME>`,
`--output <state>/fleet.json`, `--watch`, and `--view <zoe-fleet>`. The adapter
requires a Firstmate home with `bin/fm-fleet-snapshot.sh`, `HERDR_ENV=1`, Python
3.9 or newer, and Herdr on `PATH` or at `HERDR_BIN_PATH`. The Firstmate snapshot
uses `jq`; `no-mistakes` is optional for validation bands. The Herdr plugin is
registered by linking the shipped path: `herdr plugin link <runtime>/fleet-plugin`.
The kit installer does not invoke this collector.

The producer writes `fleet.json` and companion checkpoint, feed, request, and lock
files in its state directory. A separate state directory per Firstmate home is
required because the manifest identifies its source by the absolute home path.
The shipped `bin/zoe-fleet-tower` launcher derives that directory per home;
its config, state paths, and commands are documented in `docs/FLEET-USAGE.md`.
The binary archive ships `zoe-fleet` apart from the runtime, and the launcher
looks for it in the runtime's `bin/` beside the real `zoe-fleet-tower`, not
beside a link to it. The kit either places `zoe-fleet` in `<runtime>/bin/` or
runs `zoe-fleet-tower setup --bin <path to zoe-fleet>`.

## Build and archive layout

On an arm64 Mac with Rust, Xcode command-line tools, Rosetta for the x64 smoke
test, and both Rust targets installed:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
scripts/release-fleet.sh <approved-full-40-character-sha> --output /tmp/zoe-fleet-release
```

The script clones `https://github.com/pikenguyen227/zoetrope.git` into a fresh
temporary checkout at the exact commit. `SOURCE_DATE_EPOCH` comes from that
commit's Unix timestamp. It builds the `zoe` bin once per target with
`cargo build --locked --release`, checks each architecture with `lipo`, runs
`--version` and `fleet assets/fleet/crew/fleet.json --inspect` for both slices
(the x64 binary through Rosetta), and rejects a dirty checkout or wrong build
stamp. Build hosts need the dependencies locally; the script does not install
Rust targets. It removes its temporary checkout on exit.

The output directory must not exist. It contains:

| Asset | Root contents |
| --- | --- |
| `zoe-fleet-0.2.0-<shortsha>-aarch64-apple-darwin.tar.gz` | one executable regular file `zoe-fleet`, `LICENSE`, `NOTICE`, `BUILD.txt` |
| `zoe-fleet-0.2.0-<shortsha>-x86_64-apple-darwin.tar.gz` | same layout for x64 |
| `zoetrope-fleet-runtime-0.2.0-<shortsha>.tar.gz` | `scripts/firstmate-fleet.py`, `scripts/claude-telemetry.py`, `scripts/fleet-demo.py`, `scripts/fleet-config.sh`, `fleet-plugin/*`, `docs/FLEET-USAGE.md`, `docs/FLEET-PACKAGING.md`, `LICENSE`, `NOTICE`, `README.md`, `bin/zoe-fleet-tower` |
| `SHA256SUMS` | SHA-256 entries for all emitted archives |

`BUILD.txt` records version, full commit, target, rustc, and
`SOURCE_DATE_EPOCH`. Each archive's NOTICE includes the build commit. The
packager still tolerates a source tree without the launcher, but a release
must include it. `--universal` optionally adds a fourth binary
archive assembled by `lipo` with the same root layout. Per-target archives are
the default because the kit manifest selects a target.

The shell fixture check exercises staging without building or launching the
viewer:

```sh
bash scripts/test_release_fleet.sh
```

## Verify and upload after approval

Review the exact commit and local archive listing. Recheck the checksums from
inside the output directory:

```sh
cd /tmp/zoe-fleet-release
shasum -a 256 -c SHA256SUMS
tar -tzf zoe-fleet-0.2.0-<shortsha>-aarch64-apple-darwin.tar.gz
```

After the post-acceptance commit is approved, create a **pre-release** on
`pikenguyen227/zoetrope`, tagged `fleet-v0.2.0-<shortsha>` and targeted at the
full commit. `gh-axi release create` is manual and is never called by
`release-fleet.sh`:

```sh
gh-axi release create fleet-v0.2.0-<shortsha> \
  --target <approved-full-sha> --prerelease \
  --title 'Fleet tower 0.2.0 (<shortsha>)' --notes-file notes.md \
  zoe-fleet-0.2.0-<shortsha>-aarch64-apple-darwin.tar.gz \
  zoe-fleet-0.2.0-<shortsha>-x86_64-apple-darwin.tar.gz \
  zoetrope-fleet-runtime-0.2.0-<shortsha>.tar.gz SHA256SUMS
```

Download the assets into an empty temporary directory and run
`shasum -a 256 -c SHA256SUMS` again. Record direct asset URLs and SHA-256 values
in the release notes for the kit manifest. No code signing, notarization, or
checksum signature is part of this release format.

## Acceptance

Run this script from a zoetrope source checkout at the candidate commit. Run
Phase A for every candidate release on macOS before approving its asset
URLs for the kit. Supply either the downloaded release directory (including
`SHA256SUMS`) or a locally built `zoe` and this runtime checkout, plus the
approved full Firstmate source commit:

```sh
scripts/acceptance-tower.sh --firstmate-ref <40-character-firstmate-sha> \
  --assets /tmp/downloaded-fleet-assets --record /tmp/fleet-acceptance.md \
  --evidence-dir /tmp/fleet-acceptance-evidence
# Or, before a release exists:
scripts/acceptance-tower.sh --firstmate-ref <40-character-firstmate-sha> \
  --local-bin target/debug/zoe --local-runtime . \
  --record /tmp/fleet-acceptance.md \
  --evidence-dir /tmp/fleet-acceptance-evidence
```

The runner stages inputs in a new `/private/tmp` lab, then executes the test
under `sandbox-exec` with a lab `HOME`, XDG directories, `TMPDIR`, and a
minimal `PATH`. Its profile denies reads and writes to the real home and
explicitly to the three live Agentic checkout/tool paths. It clones Firstmate
from GitHub at the supplied pin; it does not read a local Firstmate checkout.
Phase A configures the tower, checks `doctor`, seeds a local secondmate without
launching an agent, verifies both snapshots and the bundled Fleet inspect
fixture, checks that generated lab data contains no owner path, and writes a
Markdown record. The lab is removed on exit. The Phase A Herdr command is an
inert availability probe used only by `doctor`; no Herdr process or viewer runs.
`--record` remains success-only. With `--evidence-dir`, the single outer exit
cleanup exports only the record if present, a failure phase/status report,
setup/doctor/clone/seed logs, snapshots, inspect output, and the Fleet manifest
and journal before removing the lab. Each exported file is capped at 1 MiB;
the directory must be new or empty, outside the lab, and pass the runner's
live-path checks; a rejected directory is never written. On any failure the
runner also prints the failing step's captured output to stderr.
This is useful when Phase A fails before the record exists or a later Phase B
assertion fails. It never copies the lab HOME or `.codex` authentication files.

### Phase B: one dedicated macOS account

Use a fresh, separate macOS user account, or a teammate's machine on which
that user's own **default** Herdr session is the lab. Run from Terminal outside
Herdr. `HOME` must be that macOS login's actual home, not an alternate HOME in
the owner's account: Herdr's API socket is per macOS user. Do not use an
account with a Fleet config/state, a Firstmate home, or any occupied or second
Herdr session. The runner reads the session, workspace and pane lists before
staging and refuses an occupied default session. `--phase-b-dedicated-account`
is the person's explicit acknowledgement of this account boundary.

Install a checksum-verified, self-contained native Herdr binary satisfying
`fleet-plugin/herdr-plugin.toml`'s `min_herdr_version`, and a native Codex
binary, in this account. Check `herdr --version`, `codex --version`, `jq --version`,
and `python3 --version`. Bring up this account's default Herdr session with no
workspaces or panes using Herdr's normal account setup. Do not sign in to Codex
in the account home or copy another person's credentials. Run Phase A first,
with its own new evidence directory. The Phase B command is:

```sh
scripts/acceptance-tower.sh --firstmate-ref <40-character-firstmate-sha> \
  --assets /tmp/downloaded-fleet-assets \
  --phase-b --phase-b-dedicated-account \
  --phase-b-codex-bin /path/to/native/codex \
  --record /tmp/fleet-phase-b.md \
  --evidence-dir /tmp/fleet-phase-b-evidence
```

`--local-bin` and `--local-runtime` can replace `--assets` before a release
exists. Herdr is taken from this account's `PATH`; the runner copies it and
Codex into the disposable kit before the sandbox. The preflight checks the
account home, absence of known Fleet/Firstmate state and ambient overrides,
Herdr version, and the empty default session. It does not start or stop Herdr.
The staged `herdr` wrapper adds the lab HOME, XDG directories, TMPDIR,
CODEX_HOME, PATH, FM_HOME, and wrapper path to every `workspace create` and
`tab create`, including Firstmate's mate pane. The real binary remains staged
as `herdr-real`. The sandbox still denies the account's real home.

In the printed lab shell, set `lab=<printed-absolute-lab-path>`. Confirm
`command -v herdr` resolves under `$lab/kit/bin`, `command -v no-mistakes`
fails, and `ZOE_NO_MISTAKES_BIN` is unset. Create the one lab workspace with
`herdr workspace create --cwd "$lab/firstmate" --label phase-b-lab --no-focus`.
Record its returned root pane ID; run a shell in that pane with
`herdr pane run <root-pane-id> 'export lab="$ACCEPTANCE_LAB" HERDR_BIN_PATH="$ACCEPTANCE_LAB/kit/bin/herdr"; exec /bin/bash --noprofile --norc'`. In the pane,
verify `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME`, `TMPDIR`,
`CODEX_HOME`, `PATH`, `FM_HOME`, and `HERDR_BIN_PATH` all point into the printed
lab, and that `no-mistakes` is absent. If any value differs, stop the run.
The pane is a genuine Herdr pane; do not fabricate `HERDR_ENV`.

The person then signs in **inside that pane** with the lab `CODEX_HOME`:

```sh
mkdir -p "$CODEX_HOME"
codex login --device-auth
codex login status
cd "$FM_HOME"
FM_HOME="$FM_HOME" "$FM_HOME/bin/fm-spawn.sh" \
  lab-mate "$lab/mate" --secondmate --backend herdr --harness codex
"$lab/runtime/bin/zoe-fleet-tower" open
```

The spawn is the single agent launch; do not also use `herdr agent start`.
Verify the new mate pane's environment before treating a collection as valid.
After one `lab-mate` collection, read `firstmate/state/lab-mate.meta` as data,
verify its recorded Herdr session is `default` and its pane is the lab mate,
end that agent, and close **only that recorded pane**. Confirm the agent exited.
Keep `lab-mate.meta`, the mate home and the collected Fleet state. Stop the
default Herdr session using this dedicated account's Herdr control, then exit
the lab shell. Do not run `fm-teardown.sh` or remove the mate before the runner
completes its assertions. The runner then inspects Fleet, takes a fresh
Firstmate snapshot and requires exactly `{lab-mate}` in both task lists, with
lab-only paths.

On success the Markdown result is `--record`; success or failure evidence is
exported to `--evidence-dir` before the lab is removed. Inspect that bounded
directory for `failure-report.txt`, snapshots, inspect output, and Fleet
manifest/journal. It excludes Codex authentication. After saving evidence,
confirm the dedicated account has no running Herdr session or agent and
remove the temporary account only according to the machine owner's normal
account cleanup procedure. Phase B must pass once before full tower parity is
claimed. It is never part of kit install or the automated Phase A check; a
new tower release still requires the owner's word.

## Linux and WSL2 extension

The target-keyed archive names can later include
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`. The existing
`.github/workflows/cd.yml` uses `cross` for those targets. Extending the script
requires target-specific build and executable smoke checks on Linux, while the
runtime archive and producer contract remain architecture-independent. WSL2 is
a Linux consumer; its kit installer chooses the Linux asset and Python runtime.
