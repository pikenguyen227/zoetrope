# Zoetrope Fleet development build

Fleet mode shows independently registered sessions in one graph. Each session
keeps its parser, timeline, tool identities and native subagent tree. The Firstmate
adapter supplies membership and observed launch generations. This is a development
prototype: automated and synthetic terminal checks pass, and the collector runs in
Herdr's Team tab. The ordinary single-session `zoe` command remains available.

## Install and try without model tokens

For a configured Firstmate home, use the launcher beside the Fleet runtime:

```sh
bin/zoe-fleet-tower setup --home /absolute/path/to/firstmate --bin /absolute/path/to/zoe-fleet
bin/zoe-fleet-tower doctor
bin/zoe-fleet-tower demo
```

`setup` requires the home's `bin/fm-fleet-snapshot.sh`, writes a private
`${XDG_CONFIG_HOME:-$HOME/.config}/zoe-fleet/config` (mode 0600), and creates
its state directory. Setup never records `ZOE_FLEET_RUNTIME` and records
`ZOE_FLEET_BIN` only when `--bin` is given; otherwise the runtime is the one the
running launcher resolves to and the binary is `zoe-fleet` in that runtime's
`bin/`, so a newer runtime's launcher uses its own scripts without re-running
setup. A binary kept anywhere else, including beside a link to the launcher,
needs `--bin`. The
config accepts `FM_HOME`, `ZOE_FLEET_BIN`, `ZOE_FLEET_STATE_DIR`,
`ZOE_TELEMETRY_DIR`, `ZOE_NO_MISTAKES_BIN` and `ZOE_FLEET_RUNTIME` as plain
`KEY=VALUE` lines; environment variables take precedence. Without `--state`,
state lives in `${XDG_STATE_HOME:-$HOME/.local/state}/zoe-fleet/<home-hash>/`,
where the hash is the first 12 SHA-256 hex digits of the resolved `FM_HOME`,
and telemetry lives in its `telemetry/` subdirectory. Setup creates both
directories private (0700) and records them in the config only when `--state`
or `--telemetry` is given. Recorded paths apply only to the configured home, so
an `FM_HOME` override always gets its own per-home state. `FM_HOME` has no
implicit default. `doctor` reads the setup and
reports binary, Python, Firstmate, Herdr, optional no-mistakes, plugin, and
state readiness. `inspect` runs a headless check of the current manifest.

For a source development build, `bash scripts/install-fleet.sh` builds
`zoe-fleet` and installs `zoe-fleet-tower`, plus the older `zoe-fleet-open` and
`zoe-fleet-demo` command names as delegates. Set `FM_HOME` first for a new
checkout. On an existing checkout with a sibling Firstmate home and `.tools/bin`,
the wrapper writes the config once with that same home, binary and existing
Fleet and telemetry state directories. Later runs leave the config alone, so an
already-running Team tab keeps its current state and binary until reopened.
The installer does not replace the Homebrew `zoe` or start a coding agent.

The demo is labelled SYNTHETIC. It creates temporary transcripts for a Captain,
two independent Codex workers and one native child, then appends fake tool events
while the viewer runs. Closing the demo removes its temporary files.

For the repeatable headless fixture check:

```sh
cargo run --locked -- fleet assets/fleet/demo/fleet.json --inspect
```

Expected: 3 sessions, 4 native agents, 4 tool calls, 5 graph nodes (including the
fleet membership node) and 4 edges. No Herdr server or model is involved.

## Firstmate in Herdr

Run `zoe-fleet-tower open` (or the older `zoe-fleet-open`) from a shell
**inside Herdr**. It links the separate
`pikenguyen227.zoetrope-fleet` plugin and opens a dedicated tab named **Team**. The adapter refuses
to run outside Herdr. Close an existing Fleet collector before opening another.

Once linked, invoke **Zoetrope: Firstmate Fleet tab** through the plugin manager
from any pane. The Fleet tab binds the Captain by what a pane runs, not by focus
or tab name (`fleet-plugin/captain.py`): the one pane whose registered Claude or
Codex session was launched in the Firstmate home and which no Firstmate task
names as its endpoint. A plain shell, such as your own Shell tab, a crew or
secondmate pane, and an agent outside the home are never taken for it; the
focused pane only settles a choice between two coordinators. When none can be
identified the collector gets no `--captain` and shows the last Captain
registered. The adapter does not launch Claude, Codex, or any workers.
Space and tab names are yours to rename, and the view follows them without
reading them as identity. The crew's root card takes the standing Captain's
Herdr workspace name (else the Captain pane's, else the Fleet tab's own
workspace), and reads `Firstmate · Control Tower` when Herdr cannot name one.
The Captain's card, and each secondmate's, takes its tab's name, such as
`(General) Captain` or `(Zoe) Captain`. When Herdr cannot name the tab, the card
keeps the last name known, else `Captain · <provider>` or the task ID. Each one
is found again by Herdr's pane, tab and workspace IDs, which a rename leaves
alone, so a rename shows at the next collection. Workers in their own disposable
workspaces, and other workspaces in the same Herdr session, neither rename the
crew nor leave it.

One Captain stands at a time: the session the Captain pane registers, or, while
that pane has none (a Shell pane, or between Captain sessions), the last Captain
registered. An earlier Captain keeps its membership and history but is no
longer registered, so it counts as finished below. A crew pane is never a
Captain, whatever its tab is called: opened (or autostarted) from a secondmate's
or a worker's pane, the Fleet keeps the last Captain registered, keeps the
crew's name, and a diagnostic names the task whose pane it was.

The manifest and checkpoint journal live in the configured per-home state
directory. Set `ZOE_NO_MISTAKES_BIN` in the config or environment when Herdr's
PATH does not contain no-mistakes; without it no card shows a validation run.

Every viewer's header names the build it runs (`build <commit>`, `-dirty` when
built from an uncommitted tree); `zoe-fleet --version` names the one on disk, and
`--inspect` reports it under `build`. Installing a rebuild replaces the binary,
not a viewer already running it: that viewer's header then says `this viewer
runs build <commit> (built …); zoe-fleet on disk is newer, rebuilt …`, and only
reopening the Team tab runs the new one. A change to the adapter needs the same
reopen.

The adapter reads `fm-fleet-snapshot.sh --json` before and after exact Herdr pane
registrations. Changed generations/endpoints defer a join. The canonical snapshot
may refresh its own observational remote-summary caches; the adapter displays local
Herdr tasks only and reports unsupported endpoints. It never drains wakes, starts
workers, sends prompts, or changes approvals. Snapshot collection can take several
seconds; transcript tailing and rendering run independently.

## Windows (WSL2 Ubuntu)

Status: **Windows via WSL2 Ubuntu; arm64 hands-on acceptance pending on the
owner's VM; x64 unconfirmed until a teammate x64 PC runs it.** The tower runs
inside WSL2 Ubuntu under Herdr for Linux, not in native Windows. Standalone
Linux desktops and servers are not claimed and not tested. WSL-specific facts
below are unverified until the VM run records them.

Prerequisites:

- Windows 11 with WSL2 and Ubuntu 22.04 or 24.04 (`wsl -l -v` shows VERSION 2).
  For a Parallels VM, enable nested virtualization in the stopped VM's
  configuration first.
- Herdr for Linux (0.9.1 or newer), Python 3.9 or newer, `bash`, `git`, `curl`,
  `jq` and `sha256sum`. `shasum` is not required; on Ubuntu it ships with the
  `perl` package.
- A Firstmate home checked out at the kit's pin.
- Keep everything on the Linux filesystem under `/home/<user>`: the runtime,
  `zoe-fleet`, the Firstmate home, the config and the state directory. Never
  use `/mnt/c`; the adapter's `fcntl.flock` lock and the mode 0600 config are
  unreliable on Windows drives.

Install, in an Ubuntu shell:

```sh
uname -m                      # aarch64 or x86_64
ldd --version | head -1       # glibc 2.35 or newer
sudo apt-get install -y jq git curl
curl -fsSL https://herdr.dev/install.sh | sh
herdr --version               # run `herdr` once, then detach
```

Download into an empty directory under the Linux home. The runtime archive
comes from release `fleet-v0.2.0-fc45c4d9ea1d`; the Linux binary and its
checksums come from the sibling release `fleet-v0.2.0-fc45c4d9ea1d-linux`
(source commit `fc45c4d9ea1de0af27092f26eaa8f5008881f01e`). Pick the binary by
`uname -m`: `aarch64` selects `aarch64-unknown-linux-gnu`, `x86_64` selects
`x86_64-unknown-linux-gnu`. The `-linux` release carries its own two-entry
`SHA256SUMS`; the runtime archive's checksum is in the original release's
`SHA256SUMS`.

```sh
case "$(uname -m)" in
  aarch64) target=aarch64-unknown-linux-gnu ;;
  x86_64)  target=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported: $(uname -m)" >&2; exit 1 ;;
esac
# From ...-linux: zoe-fleet-0.2.0-fc45c4d9ea1d-$target.tar.gz and SHA256SUMS
# From the original release: zoetrope-fleet-runtime-0.2.0-fc45c4d9ea1d.tar.gz,
# with its SHA256SUMS saved here as SHA256SUMS.runtime
sha256sum -c SHA256SUMS --ignore-missing
sha256sum -c SHA256SUMS.runtime --ignore-missing
mkdir -p ~/zoe-fleet-runtime/bin
tar -xzf zoetrope-fleet-runtime-0.2.0-fc45c4d9ea1d.tar.gz -C ~/zoe-fleet-runtime
tar -xzf "zoe-fleet-0.2.0-fc45c4d9ea1d-$target.tar.gz" -C ~/zoe-fleet-runtime/bin zoe-fleet
chmod 755 ~/zoe-fleet-runtime/bin/zoe-fleet
~/zoe-fleet-runtime/bin/zoe-fleet --version   # release commit, no -dirty
```

`--ignore-missing` skips lines for files you did not download, but a mismatch in
a downloaded file still fails; each command must report its archive `OK`.

Set up, check and open:

```sh
~/zoe-fleet-runtime/bin/zoe-fleet-tower setup --home ~/firstmate --bin ~/zoe-fleet-runtime/bin/zoe-fleet
~/zoe-fleet-runtime/bin/zoe-fleet-tower doctor
# inside a Herdr shell pane:
~/zoe-fleet-runtime/bin/zoe-fleet-tower open
# from any shell:
~/zoe-fleet-runtime/bin/zoe-fleet-tower inspect
```

Expect `doctor` to exit 0 reporting Herdr found and, before the first `open`,
Herdr plugin not linked. `open` links the plugin and opens the **Team** tab.
`inspect` prints JSON whose `fleet_id` is `firstmate:/home/<user>/firstmate`.
Keep the outputs of the version, checksum, `doctor` and `open` steps, and a
screenshot of the Team tab, as the hands-on evidence. When done, close the Team
tab and run `herdr server stop`.

Known caveats:

- The WSL VM, and the Herdr server with it, stops shortly after the last Ubuntu
  terminal closes; keep one open (Windows Terminal is the expected host).
- Herdr's own native-Windows plugin support is preview and its bash plugin
  commands need Windows alternatives, which is why this path uses WSL2.
- The Phase A/B acceptance runner is macOS-only; WSL2 is checked by this
  checklist instead.

## Controls

| Key | Action |
| --- | --- |
| Tab / arrows / click and release | Select a node and open its reading panel |
| Drag a card | Move it without opening/changing the reading panel |
| Wheel over reading panel | Scroll tool history (wheel on canvas zooms) |
| Space | Pause or play the whole crew on the fleet timeline |
| Click or drag the scrubber, `[` / `]` | Seek the fleet playhead; step between prompts and lifecycle events |
| g / End | Back to the live edge |
| s | Skip idle gaps (only while every session is quiet) or play in real time |
| Enter | Open the selected session's inspector and timeline, at the fleet's moment |
| Esc in a session | Return to the fleet at its moment |
| p | List the selected card's validation run's pipeline agents; ↑↓ (or `j`/`k`) and Enter opens one read-only, `p` or Esc closes the list (see "Pipeline agents") |
| x in the fleet | Collapse/expand the selected session's native children |
| e | Show or hide the graph's edge labels (shown by default) |
| r in the fleet | Rearrange the graph: re-run the layout so cards return to the arrangement a fresh open gives, and fit them into view |
| v | Show or hide finished workers (hidden when the fleet opens) |
| A | Archive the selected finished worker (under a collector) |
| D | Delete the selected worker, after a confirmation (under a collector) |
| C | Clear everything: `a` archives it all, `D` deletes it all after a confirmation (under a collector) |
| o / f | Overview / follow camera |
| q or Ctrl+C | Quit |

The reading panel uses 40% of the canvas width, capped at 80 terminal columns,
so the graph remains visible while inspecting an agent.

The fleet overview has one timeline for the whole crew: the upstream scrubber
over every session's activity plus Firstmate lifecycle marks (`+` spawned, `▲`
needs-decision, blocked or a decision, `✓` done, `✗` failed, `•` any other
status line, `⊘` torn down) and validation marks (see
"Validation runs" below). Scrubbing shows the crew as
it was then: sessions and attempts that did not exist yet are absent, torn-down
ones are dimmed, and cards carry the task's status badge at that moment. Hatched
stretches are times nothing observed Firstmate (before its lifecycle feed began,
and outside the adapter's polls where it bridges, though the stretch since
its newest poll counts as observed while it is still running); there badges read
`?` and the header says the lifecycle there is unverified. The footer narrates
the newest lifecycle event at the playhead. Inside a session, its own timeline
works as before.

Try it without Herdr on the synthetic crew (a finished run with a coverage gap):

```sh
cargo run --locked -- fleet assets/fleet/crew/fleet.json
```

## Validation runs

A worker validating through no-mistakes runs short-lived pipeline agents with
no pane of their own. The adapter reads the run Firstmate attributes to each
task (the snapshot's `validation_run`) with two read-only CLI calls, `axi
status --run <id>` and `daemon status`, and never drives the run or its
daemon; `FIRSTMATE-FLEET.md`, "Validation runs", has the rules. No new cards:
the worker's card shows its run as of the playhead in the row that otherwise
reads `provider · session`, headline first and then one glyph per step:

```text
▸ test r1 · 2m ✓✓✓▸·····     running: step, round, how long the round has run
▲ review gate · 1 ask-user    parked on findings only the operator decides
▲ review gate · 3 findings    parked on findings the worker decides (not amber)
✓ passed · PR #12             ended (✗ failed at ci, ⊘ cancelled)
· queued                      read, but no step has started
? started · not read yet      Firstmate attributed it; no read yet
? review r1 · daemon down     unverified: the last read, kept but not trusted
```

Step glyphs: `✓` completed, `-` skipped, `·` pending, `▸` running, `⚒`
fixing, `▲` waiting at a gate, `✗` failed. A narrow card drops the glyphs
before the headline, and ends a strip it cannot fit whole in `…`. The band
reads `?` wherever nobody was reading the run (before the adapter first read
it, while it was down, when a read failed or was not understood), once the
no-mistakes daemon is down, since its record may then be stale (a read that
says the run ended is still taken), and once the run's record is gone (`·
record gone`). While in doubt the step glyphs dim too. A run that
had ended cannot change and never reads `?`; a gate with ask-user findings
stays amber, since it stays open until someone answers it. Runs outlive their
workers: press `v` to see a finished worker's card while its CI still runs.

The reading panel adds the run under its header: which run and its status,
when it was read and created, why it is unverified if it is, the gate with its
findings and ask-user count, the PR and any error the run reports. Then comes
the step table, a row per step: its glyph, name and status with its round
(`fixing r2`), its findings, how long it ran and when it got there. Those times are the adapter's, since the CLI
only gives ages: `08:10:00–08:10:30` means it changed between two reads, `by
08:10:30` that no earlier read is known, and `since 08:09:20 (derived)` is an
active round's start worked out from its age. A panel too short for the table
keeps the step the run is at and says how many rows it left out.

On the scrubber a run leaves three kinds of mark: `▷` its start, `▲` each
time a gate parks on ask-user findings (`△` on others), and its end, `✓`
passed, `✗` failed or a dim `✗` cancelled. Each is a `[` / `]` chapter; the
steps between are not, and the footer names the event at the playhead. A
stretch where a run was alive but unread is hatched.
Read failures, format drift and a down daemon are manifest diagnostics in the
header. Try it on the synthetic crew with the runs the adapter read beside it
(`assets/fleet/validation`):

```sh
demo=$(mktemp -d) && cp -R assets/fleet/crew/. "$demo" &&
  cat assets/fleet/validation/fleet.events.jsonl >> "$demo/fleet.events.jsonl" &&
  cargo run --locked -- fleet "$demo/fleet.json"
```

## Pipeline agents

A validation run's agents are not fleet members, but their transcripts can be
read. Select a card whose band shows a run and press `p`: a list opens over
the canvas with the run the band shows at the playhead and one row per
transcript found for it. Each row gives the transcript's first and last
record, how many tools it called, its subagents and model, and its session.
`↑`/`↓` (or `j`/`k`) choose, Enter opens one in the session inspector (at the
fleet's moment, like Enter on a card), Esc returns to the list, and Esc or `p`
closes it. A card with no run at the playhead, or a run whose ID carries no
time, says so instead of opening the list.

The join is an inference, and the list says so. no-mistakes does not say
which transcript is which agent, so the viewer uses what is on disk. Each
pipeline agent runs in the run's worktree, and Claude files its transcript under
`~/.claude/projects/-…-no-mistakes-worktrees-<repo>-<run>/<session>.jsonl`.
A transcript is taken as the run's only when it is filed there and its own
dated records fall inside the run's life: none before the run was created
(its ID encodes that) and none after a read found it ended. No agent is tied
to a step, since nothing on disk says which step ran it. The rest are listed
dimmed with the reason, and Enter does not open them:

```text
ambiguous: also filed under run 01M…       the same session under two runs
not this run's: it began …, before …       records outside the run's life
cannot be placed in the run: no dated record
unreadable: Permission denied …            the file could not be read
```

`p` with nothing to list says why in the header: no card is selected, or no
validation run is attributed to the selected card at the playhead, followed
by the collector's diagnostic about no-mistakes when it gave one.
A run with nothing filed under its worktree says so: its agents may not have
started yet, their transcripts may have been removed, or they may not be
Claude's. The heading repeats the run, when it was created and whether a read
has found it ended. An opened transcript is read-only and followed live while it is
open. It is not a fleet member: it is not in the manifest or the journal, it
does not count toward the crew or the 128-session cap, and closing it stops
its watcher. The list reads files only. It never talks to no-mistakes or opens its
database.

## Finished workers, archive and delete

A worker is **finished** once every launch attempt joined to its session was
torn down (the journal's `torn_down`: it left the Firstmate snapshot), or, at
the live edge, once the adapter no longer registers it: its session is gone
from the manifest, or every task joined to it has runtime `not observed`, as
for history the journal never recorded. An idle or quiet worker has not
finished, and neither has the standing Captain, which has no attempt to finish;
an earlier Captain finishes once a later one takes its place (the manifest
reads that session's own runtime as `not observed`). Attempt
cards without a session finish when torn down or, at the live edge, when their
task is `not observed`. Scrubbed into the past, only the journal counts.

Finished workers are hidden when the fleet opens: they are left out of the
graph, which re-arranges around the crew still at work, and the header counts
them (`14 finished hidden`). `v` shows them again, dimmed, and re-arranges.
Hiding follows the playhead: scrub back to when a worker was still at work and
it is there, since it had not finished yet; its lifecycle marks stay on the
scrubber either way. At the live edge, a worker finishing re-arranges the
crew once; scrubbing never does.

The journal needs no trimming by hand: the collector drops superseded manifest
checkpoints from it on its own, keeping every lifecycle record (when and how:
`FIRSTMATE-FLEET.md`, "Retention").

Archive and delete remove records rather than hiding them, from the adapter's
state directory only: the manifest `fleet.json` and its journal
`fleet.events.jsonl`, which holds the lifecycle records and the newest manifest
checkpoint. Agent transcripts under `~/.claude` or `~/.codex`, the Firstmate
home, other backups and anything else in the directory are never touched. The
adapter's feed cursor `fleet.feeds.json` stays too, so the removed history is
not read back from Firstmate's lifecycle feed.

| | Everything | One worker |
| --- | --- | --- |
| Archive | Moves both files into `backup-<time>/` beside them. The next collection starts empty. | Moves its manifest entry, its lifecycle records and its part of every checkpoint into `backup-<time>-<worker>/`, a fleet of its own. Refused while it is still registered. |
| Delete | Removes both files, after a confirmation. | Removes the same records permanently, after a confirmation. |

A worker is its session with every attempt joined to it, or an attempt card
without a session. Removal is scoped by those attempts: a relaunch under a new
generation and session keeps its own records. Deleting a worker the adapter
still registers is refused unless confirmed, and it reappears, with fresh
records, on the next collection; a finished one does not. Removed records
leave the timeline too, past included, which is what sets them apart from
hiding. Every delete names what it removes and that it cannot be undone;
archive asks nothing, since it can be restored.

**In the viewer**, while it runs under the collector (the Team tab), select a
card and press `A` or `D` (press `v` first to reach a hidden finished worker),
or press `C` for everything. The viewer never
edits the records: the collector owns them and holds them in memory, and
would write back anything removed under it. So the viewer writes the request
to the file the collector named, `fleet.request.json`, and exits; the
collector ends its coverage in the journal, carries the request out, forgets
what it held, collects afresh and reopens the viewer with a note saying what
happened. A viewer opened directly (`zoe-fleet fleet …`) has no collector and
says so.

**From a shell**, while no collector runs (close the Team tab first; these
refuse while one holds the lock). They only change files, so Herdr is not
needed:

```sh
state=/absolute/state/path/printed/by/zoe-fleet-tower/setup
python3 scripts/firstmate-fleet.py --output "$state/fleet.json" --archive
python3 scripts/firstmate-fleet.py --output "$state/fleet.json" --delete
python3 scripts/firstmate-fleet.py --output "$state/fleet.json" --archive --worker <session ID, task or task/spawn_gen>
python3 scripts/firstmate-fleet.py --output "$state/fleet.json" --delete --worker <…>
```

To restore an archive of everything, with no collector running, move the
current `fleet.json` and `fleet.events.jsonl` aside and move the backup's two
files back. A worker's backup opens on its own with
`zoe-fleet fleet <backup>/fleet.json`.

## Connections and history

- **member:** observed fleet membership, without implying Captain ancestry.
- **delegates:** explicit assignment. The adapter records one from a local
  secondmate to each worker it dispatched in its own Firstmate home, so that
  crew hangs under the secondmate as Herdr's sidebar nests it, with its own
  status and timeline. It links launch attempts, so a worker hangs under its
  secondmate as soon as it appears, before either has a session, and follows
  the secondmate's current attempt across a relaunch; while the secondmate is not shown (hidden as finished), its crew
  hangs from the root. A remote secondmate's crew is not read.
- **continues:** a new native session taking over the same work. The adapter records
  different observed launch generations/IDs of the same Firstmate task/project.
  Resuming the same native ID keeps one node. The new session keeps its own
  member edge to the crew root: succession is history, not parentage.
- **depends on:** explicit session dependency in the manifest. Firstmate blocker
  IDs are retained as task metadata; automatic dependency arrows are future work.
- Unlabelled edges inside a session are the provider's native subagent relationships.

Firstmate task state and Herdr runtime state stay separate in root details, with
provenance and observation times. Quiet output does not prove task completion.
Missing transcripts are unavailable; closed panes and cleanup do not erase observed
membership. Transcript contents are neither copied into the journal nor retained if
an agent deletes them.

Automatic Captain lineage/handoff capture and opening a finished crew as a replay
are future work. The journal records lifecycle from Firstmate's own feed where
the snapshot points at one, with every transition at its real time, including
those while no adapter ran (see `FIRSTMATE-FLEET.md`, "Lifecycle journal").
Without a feed, the adapter bridges from snapshots: a status line keeps its own
stamp and a spawn its `spawn_gen` time, but lines written between polls or while
no adapter ran are not recovered. The collector retires finished members on its
own: each stays in the manifest for 24 hours after it left every home's snapshot,
and only the 32 most recent stay at all; the journal keeps their history. The
viewer shows at most 128 sessions and 4096 task attempts/links: past that it shows
the most recent and says in the header how many older ones it hid (see
`FIRSTMATE-FLEET.md`, "Adapter contract and durability"). Invalid refreshes retain
the last usable view and show its age.

`assets/fleet/demo/fleet.json` demonstrates `zoetrope.fleet.v1`. File targets are
relative to the manifest and must match the declared complete native session ID.
Omit `file` to resolve that exact registered ID through existing provider discovery.
Session prefixes and newest-in-directory selection are not fleet identities.

## Verification

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --no-default-features
python3 -m unittest discover -s scripts -p 'test_*.py' -v
bash -n bin/zoe-fleet-tower scripts/fleet-config.sh fleet-plugin/open.sh fleet-plugin/pane.sh scripts/install-fleet.sh
```

The existing browser lock requires Rust 1.90 or later. Check that unchanged
single-session frontend with its own manifest and the wasm target.

## Tokens, cost and account limits

Agent cards and reading panels show recorded input + output tokens and a standard
API-equivalent cost estimate. Cache reads/writes are included in input exactly once;
reasoning is part of output, not added again. Repeated request usage is merged by
request ID, including later higher output counts. Codex cumulative snapshots emit
only positive advances. Tokens follow the session replay cursor.

`tok+` means only partial usage was recorded. `API est. —` means usage or a model
rate is unavailable, not zero spend. Prices are an offline snapshot verified on
2026-09-22 (Claude Opus 5.5 added 2026-09-23) against
[OpenAI pricing](https://developers.openai.com/api/docs/pricing)
and [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing).
The table covers the GPT-6 / GPT-6.1 (sol, terra, luna, astra) and GPT-5.6 families and
listed recent Claude Fable/Opus/Sonnet/Haiku model IDs, rechecked 2026-10-28.
Cache-read multiples vary by model (Fable 5.1 0.025x; Opus 5.5, Sonnet 5.5 and
GPT-6.1 sol/terra 0.05x; others 0.1x). The estimate uses standard short-context,
5-minute cache-write and recorded 1-hour cache-write rates. Fast-mode,
long-context, regional and tool surcharges,
discounts and subscription billing are not included.
**This is an API-equivalent estimate, not your actual subscription bill.**

A model ID missing from the table never silently takes another model's rate.
When its ID has the shape of a listed tier (`claude-opus-*`, `claude-sonnet-*`,
`claude-haiku-*`, or `gpt-<version>-astra|sol|terra|luna`), it is priced at the
nearest listed model of that tier (the newest version not above it) and shown as
`API est. ~$1.234 (unlisted model)` on both the card and the reading panel; the
`~` leads so a narrow card that clips the suffix still reads as approximate. One such request makes the whole total
approximate. That figure is a rough guide only: a new model can be priced
differently from its predecessor, and long-context, fast-mode or other premium
variants may bill differently again. An ID with no listed tier (another Claude
family, another GPT tier, or any other provider), or older than every listed
model of its tier (older models can cost more), still shows `API est. —`.

The footer reports five-hour and weekly windows by their actual duration. Quotas
are shared account limits: they are never added across agents. It uses the newest
reported snapshot per provider/limit bucket among current manifest members. If you
use multiple accounts with the same provider/bucket, run separate Fleet viewers:
transcripts do not expose a reliable account identity for partitioning them.
Percentages are **remaining**; reset countdowns and observation age are shown.
Snapshots older than five minutes are labeled stale. A passed reset time means
“refresh pending”, not an assumed full allowance. Missing windows show `—`.
These current account snapshots stay separate from session replay.

Codex supplies rate-limit snapshots in its `token_count` records. Bucket/credits-only
records without a complete quota window do not create or replace a snapshot;
distinct buckets with reported windows remain separate, including stale ones.
Claude supplies them through its documented [status-line JSON](https://code.claude.com/docs/en/statusline),
which normally appears after the first API response on a supported subscription.
The optional `scripts/claude-telemetry.py` launcher adds a status-line bridge while
preserving existing status-line output and explicit `--settings` overlays:

```sh
python3 scripts/claude-telemetry.py --launch /absolute/path/to/real/claude -- [normal Claude arguments]
```

It writes only exact session ID, reported quota windows and observation time to
the configured home's Fleet `telemetry/` directory. No prompts, credentials or
transcript bodies are written. Override with `ZOE_TELEMETRY_DIR`; use that same
directory in the viewer.
The Fleet Herdr launcher sets this directory automatically. Already-running Claude
sessions need to be reopened through the bridge. The helper does not start an agent
until you explicitly run the launcher; installation and tests use no model calls.
Neither the viewer nor bridge makes network requests or refreshes account credentials.
