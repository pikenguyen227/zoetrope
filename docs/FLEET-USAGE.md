# Zoetrope Fleet development build

Fleet mode shows independently registered sessions in one graph. Each session
keeps its parser, timeline, tool identities and native subagent tree. The Firstmate
adapter supplies membership and observed launch generations. This is a development
prototype: automated and synthetic terminal checks pass; live Herdr validation is
pending. The ordinary single-session `zoe` command remains available.

## Install and try without model tokens

From this checkout:

```sh
bash scripts/install-fleet.sh
../.tools/bin/zoe-fleet-demo
```

The installer builds a release executable and copies it as `zoe-fleet` into the
parent Agentic folder's `.tools/bin`, alongside `zoe-fleet-demo` and `zoe-fleet-open`.
It does not replace the installed Homebrew `zoe` or start a coding agent.

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

Run `zoe-fleet-open` from a shell **inside Herdr**. It links the separate
`pikenguyen227.zoetrope-fleet` plugin and opens a dedicated tab named **Team**. The adapter refuses
to run outside Herdr. Close an existing Fleet collector before opening another.

Once linked, invoke **Zoetrope: Firstmate Fleet tab** through the plugin manager
with a Captain pane focused to include that pane's exact session as a Captain.
Opening from Shell still discovers Firstmate workers; a Shell pane is not guessed
to be a Captain. The adapter does not launch Claude, Codex, or any workers.

Default arrangement: `Agentic/firstmate` beside `Agentic/zoetrope`. Override with
`FM_HOME`, `ZOE_FLEET_BIN`, or `ZOE_FLEET_STATE_DIR` if needed. By default, the
manifest and checkpoint journal live in `Agentic/.tools/state/zoe-fleet/`.

The adapter reads `fm-fleet-snapshot.sh --json` before and after exact Herdr pane
registrations. Changed generations/endpoints defer a join. The canonical snapshot
may refresh its own observational remote-summary caches; the adapter displays local
Herdr tasks only and reports unsupported endpoints. It never drains wakes, starts
workers, sends prompts, or changes approvals. Snapshot collection can take several
seconds; transcript tailing and rendering run independently.

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
| x in the fleet | Collapse/expand the selected session's native children |
| v | Show or hide finished workers (hidden when the fleet opens) |
| A | Archive the selected finished worker (under a collector) |
| D | Delete the selected worker, after a confirmation (under a collector) |
| C | Clear everything: `a` archives it all, `D` deletes it all after a confirmation (under a collector) |
| r | Arrange the graph |
| o / f | Overview / follow camera |
| q or Ctrl+C | Quit |

The reading panel uses 40% of the canvas width, capped at 80 terminal columns,
so the graph remains visible while inspecting an agent.

The fleet overview has one timeline for the whole crew: the upstream scrubber
over every session's activity plus Firstmate lifecycle marks (`+` spawned, `▲`
needs-decision or blocked, `✓` done, `⊘` torn down). Scrubbing shows the crew as
it was then: sessions and attempts that did not exist yet are absent, torn-down
ones are dimmed, and cards carry the task's status badge at that moment. Hatched
stretches are times the adapter was not observing Firstmate; there badges read
`?` and the header says the lifecycle there is unverified. The footer narrates
the newest lifecycle event at the playhead. Inside a session, its own timeline
works as before.

Try it without Herdr on the synthetic crew (a finished run with a coverage gap):

```sh
cargo run --locked -- fleet assets/fleet/crew/fleet.json
```

## Finished workers, archive and delete

A worker is **finished** once every launch attempt joined to its session was
torn down (the journal's `torn_down`: it left the Firstmate snapshot), or, at
the live edge, once the adapter no longer registers its session at all. An
idle or quiet worker has not finished, and neither has a Captain, which has
no attempt to finish. Attempt cards without a session finish when torn down.

Finished workers are hidden when the fleet opens: they are left out of the
graph, which re-arranges around the crew still at work, and the header counts
them (`14 finished hidden`). `v` shows them again, dimmed, and re-arranges.
Hiding follows the playhead: scrub back to when a worker was still at work and
it is there, since it had not finished yet; its lifecycle marks stay on the
scrubber either way. At the live edge, a worker finishing re-arranges the
crew once; scrubbing never does.

Archive and delete remove records rather than hiding them, from the adapter's
state directory only: the manifest `fleet.json` and its journal
`fleet.events.jsonl`, which holds the lifecycle records and every manifest
checkpoint. Agent transcripts under `~/.claude` or `~/.codex`, the Firstmate
home, other backups and anything else in the directory are never touched.

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
state=../.tools/state/zoe-fleet
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
- **delegates:** explicit relationship supplied in the manifest.
- **continues:** a new native session taking over the same work. The adapter records
  different observed launch generations/IDs of the same Firstmate task/project.
  Resuming the same native ID keeps one node.
- **depends on:** explicit session dependency in the manifest. Firstmate blocker
  IDs are retained as task metadata; automatic dependency arrows are future work.
- Unlabelled edges inside a session are the provider's native subagent relationships.

Firstmate task state and Herdr runtime state stay separate in root details, with
provenance and observation times. Quiet output does not prove task completion.
Missing transcripts are unavailable; closed panes and cleanup do not erase observed
membership. Transcript contents are neither copied into the journal nor retained if
an agent deletes them.

Automatic Captain lineage/handoff capture and opening a finished crew as a replay
are future work. The journal records lifecycle the adapter observed (see
`FIRSTMATE-FLEET.md`, "Lifecycle journal"): a status line keeps its own stamp and a
spawn its `spawn_gen` time, but lines written between polls or while no adapter
ran are not recovered. The manifest accepts at most 128 sessions and 4096 task attempts/links;
start a separately named archive for larger histories. Invalid refreshes retain the
last usable view and show its age.

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
bash -n fleet-plugin/open.sh fleet-plugin/pane.sh scripts/install-fleet.sh
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
2026-09-22 against [OpenAI pricing](https://developers.openai.com/api/docs/pricing)
and [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing).
The table covers the GPT-6 Astra / GPT-5.6 family and listed recent Claude
Opus/Sonnet/Haiku model IDs. It uses standard short-context rates, cache-read,
5-minute cache-write and recorded 1-hour cache-write rates. Fast-mode, long-context,
regional and tool surcharges, discounts and subscription billing are not included.
**This is an API-equivalent estimate, not your actual subscription bill.** New or
unrecognized model IDs deliberately show no dollar total rather than guessing.

The footer reports five-hour and weekly windows by their actual duration. Quotas
are shared account limits: they are never added across agents. It uses the newest
reported snapshot per provider/limit bucket among current manifest members. If you
use multiple accounts with the same provider/bucket, run separate Fleet viewers:
transcripts do not expose a reliable account identity for partitioning them.
Percentages are **remaining**; reset countdowns and observation age are shown.
Snapshots older than five minutes are labeled stale. A passed reset time means
“refresh pending”, not an assumed full allowance. Missing windows show `—`.
These current account snapshots stay separate from session replay.

Codex supplies rate-limit snapshots in its `token_count` records. Claude supplies
them through its documented [status-line JSON](https://code.claude.com/docs/en/statusline),
which normally appears after the first API response on a supported subscription.
The optional `scripts/claude-telemetry.py` launcher adds a status-line bridge while
preserving existing status-line output and explicit `--settings` overlays:

```sh
python3 scripts/claude-telemetry.py --launch /absolute/path/to/real/claude -- [normal Claude arguments]
```

It writes only exact session ID, reported quota windows and observation time to
`Agentic/.tools/state/zoe-telemetry/`. No prompts, credentials or transcript bodies
are written. Override with `ZOE_TELEMETRY_DIR`; use that same directory in the viewer.
The Fleet Herdr launcher sets this directory automatically. Already-running Claude
sessions need to be reopened through the bridge. The helper does not start an agent
until you explicitly run the launcher; installation and tests use no model calls.
Neither the viewer nor bridge makes network requests or refreshes account credentials.
