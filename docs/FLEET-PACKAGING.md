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
  --assets /tmp/downloaded-fleet-assets --record /tmp/fleet-acceptance.md
# Or, before a release exists:
scripts/acceptance-tower.sh --firstmate-ref <40-character-firstmate-sha> \
  --local-bin target/debug/zoe --local-runtime . \
  --record /tmp/fleet-acceptance.md
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

Phase B is a separate, explicit `--phase-b` invocation for one person after
lab-safe Herdr and agent binaries have been provisioned. It opens an
interactive shell inside the same sandbox and prompts for one Herdr pane,
`zoe-fleet-tower open`, and one `fm-spawn.sh --secondmate lab-mate` launch.
After the person exits that shell, the runner inspects the manifest and checks
its fleet ID, spawned task presence, and absence of owner paths in the manifest
and event journal. Record the inspect output in the Markdown record. Phase B
must pass once before declaring full tower parity; it is never run by a kit
install or check, and is not part of this automated Phase A check.

## Linux and WSL2 extension

The target-keyed archive names can later include
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`. The existing
`.github/workflows/cd.yml` uses `cross` for those targets. Extending the script
requires target-specific build and executable smoke checks on Linux, while the
runtime archive and producer contract remain architecture-independent. WSL2 is
a Linux consumer; its kit installer chooses the Linux asset and Python runtime.
