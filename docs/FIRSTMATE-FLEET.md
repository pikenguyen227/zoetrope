# Firstmate Fleet mode: implementation proposal

Status: development prototype implemented and validated with synthetic fixtures,
including a fleet timeline over the adapter's lifecycle journal.
Live Firstmate/Herdr validation is pending; see `FLEET-USAGE.md` for launch steps.
Baseline: upstream commit `b1f31dd`, Zoetrope 0.2.0.
Fork: https://github.com/pikenguyen227/zoetrope

## Outcome

One permanent Zoetrope tab shows a Firstmate crew: the Captain, independently
launched workers, their native subagents, and recorded session continuations.
Preserve the existing cards, tool activity, camera controls, and session details.
Start with local Codex sessions; mixed providers and combined historical replay
follow once identity and lifecycle behavior are proven.

The application remains a read-only observer of agents. It does not launch agents,
send prompts, complete Firstmate tasks, or change approval settings.

## Confirmed constraints

- `src/tailer/mod.rs` accepts `Watch(Target)` for one watched session.
- Tail events carry a session ID; `App::is_current` in `src/state/mod.rs` rejects
  events for other sessions. Reset currently replaces the selected session model.
- `src/state/graph.rs` projects one `SessionModel`; agents have one native parent.
- Native agent IDs such as `main` are only unique inside a session.
- The existing Herdr plugin resolves the focused pane's exact provider/session ID.
- Firstmate's local `fm-fleet-snapshot.v1` exposes tasks, launch generations,
  endpoints, status provenance and freshness. An endpoint is not a native session ID.

Preserve the provider boundary and the time/identity invariants documented in
`ARCHITECTURE.md`, `DISCOVERY.md`, and `HERDR-PLUGIN.md`.

## Architecture

```text
Firstmate canonical snapshot ---- task ownership and observed state ---+
Firstmate lifecycle feed(s) ----- timestamped transitions (tailed) ----+
                                                                    |
Herdr exact session registration -- endpoint/session join ------------+-> adapter
                                                                         |
                                                       atomic fleet manifest
                                                       lifecycle journal
                                                                         |
Native transcripts -> existing provider parsers -> per-session models -> Fleet UI
```

The adapter runs inside a genuine Herdr environment. Zoetrope's portable core
consumes data structures and performs no Herdr commands. Native filesystem and
process operations stay behind the native feature boundary.

Use an additive `src/fleet/` module for validated membership, relationships and
graph projection. Keep provider parsers and per-session facts unchanged wherever
possible. Store independent models, timelines, metadata and reset generations per
session; a reset or parse error affects only its source. Scope any combined graph,
call index or token-dedup index by session rather than concatenating raw facts.

## Identity and relationships

| Entity | Stable key |
| --- | --- |
| Native session | `(provider, native_session_id)` for the local-only first version |
| Visible native agent | `(session_key, provider_agent_id)` |
| Logical task | `(firstmate_home_id, task_id)` |
| Launch attempt | `(task_key, spawn_gen)` |

Pane, tab, workspace and process IDs are locations, not durable identities. A launch
attempt references its verified native session. Resuming the same native session
does not create another session node, even if a new launch generation is recorded.

Relationships have an ID, type, endpoints, evidence source and observation time:

- `delegates`: explicit Captain-to-worker assignment.
- `native_parent`: parentage stated by the provider's native records.
- `continues`: an explicitly recorded new session taking over the same work.
- `depends_on`: a task prerequisite, separate from parentage.

Do not infer these from timestamps, working directories or similar titles. Until
the initiating Captain is verified, associate a worker with its Firstmate home;
do not draw an invented Captain edge. On a future multi-host extension, add source
identity before accepting remote native IDs.

## Adapter contract and durability

Define a versioned `zoetrope.fleet.v1` manifest with `fleet_id`, `observed_at`,
sessions, tasks, attempts and evidenced links. Exact transcript file targets can
be used in fixtures; production joins use Herdr's verified provider/session pair.
The native reader verifies that a resolved transcript has the requested identity.

Resolve membership from the canonical Firstmate snapshot rather than implementing
a second parser of Firstmate task state. Recheck launch generation when resolving
an endpoint, rejecting a join if the worker was replaced during observation.
Mark sessions awaiting registration as unresolved, and retry without guessing.
Run the snapshot with the directory of `HERDR_BIN_PATH` on its `PATH`, because
Firstmate reads crew state through `herdr`; when every local task still reads
backend unreachable, say so in a manifest diagnostic rather than showing a quiet unknown.

Publish manifests atomically. Keep the last valid version if a refresh is malformed;
show its age and diagnostic. Retain lifecycle events in an append-only journal
outside the source checkout; only an explicit archive or delete removes records
from it. A worker disappearing from the current snapshot must
not erase historical membership or prove task completion. Do not copy private
transcripts into the repository. Missing transcripts produce an unavailable node.

Preserve Firstmate task state, Herdr runtime state and transcript activity separately,
including unknown/stale values and their provenance. Snapshot observation time is
not an exact historical transition time. Initial integration is local-only;
remote snapshot behavior must be explicitly handled before enabling remote crews.

## UI and performance

- Fleet overview uses readable task names and retains completed nodes, dimmed.
  Finished workers (every attempt torn down, or no longer registered) are
  hidden until shown, as of the playhead; archive and delete remove records,
  through the collector while it runs (see `FLEET-USAGE.md`).
- Selecting a node opens that session's existing tool and transcript details.
- Native children can be collapsed. Continuation and dependency edges have explicit
  labels and distinct styles; they must not override native parentage.
- Incrementally patch graph content. Preserve node positions and camera state;
  relayout deliberately rather than on every poll.
- Watch registered files incrementally. Refresh Firstmate membership at a bounded,
  slower cadence; do not run the snapshot command on every transcript poll.
- The overview has one wall-clock playhead across every session (see "Fleet
  timeline" below). Never claim a reconstruction outside covered windows: where
  lifecycle was not observed, show the gap, and never let present-day runtime
  state leak into the past.

## Lifecycle journal

The adapter appends `zoetrope.fleet.journal.v2` lines to `<manifest>.events.jsonl`.
Manifest checkpoints are `{"schema":…,"kind":"manifest","manifest":{…}}` (v1
checkpoints still recover). Lifecycle lines are
`{"schema":…,"kind":"lifecycle","event":{…}}`, where the event carries an `id`
derived from the fact (never from arrival), a `source` (`bridge` or `firstmate`),
`at` and `at_quality`, the `attempt` (`task`, `spawn_gen`), an optional native
`session`, and a `type` with its payload under a key of the same name:
`spawned`, `bound`, `status`, `decision`, `steered`, `steer_acked`,
`reclassified`, `torn_down`, `busy` or `coverage` (`{from, to, max_gap}`, an
interval someone was observing; `max_gap` in seconds is optional).
Types and validation are in `src/fleet/journal.rs`. The viewer tails complete
lines from a byte offset, skips lines that do not validate, deduplicates by `id`,
and orders by time, then same-second rank (spawned and bound before transcript
items, torn_down after), then source order.

`at_quality` says how the time is known: `stamp` (the author's `[at=]`),
`firstmate` (Firstmate's own transition), `derived` (a `spawn_gen` epoch),
`observed` (when an observer noticed), `backfill`, or `unknown` (no `at`; never
placed on the timeline).

### Firstmate's lifecycle feed

Firstmate appends an `fm-lifecycle.v1` feed per home, whether or not the adapter
runs; its `docs/configuration.md` owns that contract. The adapter translates it
(`Feeds` and `translate` in `scripts/firstmate-fleet.py`):

- Discovery is the snapshot's additive `lifecycle` pointer: this home's feed and
  each local secondmate home's. A remote secondmate's feed (not mirrored locally)
  and a feed that cannot be read are skipped with a manifest diagnostic; neither
  fails a collection. No pointer means the feed is off: the bridge speaks alone.
- Each feed is tailed from a cursor in `<manifest>.feeds.json`: the file identity
  and byte offset read up to, and the last `seq`. Reading follows rotation to
  `events.v1.<first-seq>.jsonl` by file identity, falls back to `seq` when that
  file is gone, and leaves a final line without its newline for the next read.
  A skipped `seq` is an event Firstmate could not write: it is counted and stays
  in the manifest diagnostics, and this home's coverage breaks from the last
  record before it (or the coverage already written) to the first after it.
  Archive and delete leave the cursor, so removed history does not come back.
- `task.spawned`, `task.status`, `task.decision`, `task.steered`,
  `task.steer_acked`, `task.reclassified`, `task.torn_down` and `task.busy`
  become the journal type of the same name. `feed.*` bookkeeping and a status
  line without a verb (continuation prose) are not events. A task without a `spawn_gen` is the manifest's `unresolved` attempt.
- The event `id` is `firstmate:<home id>#<feed key>` and the `source` is
  `{kind: firstmate, home, seq}`. `at_source` maps to `at_quality`: `stamp`
  stays, `firstmate` and `inbox` (Firstmate's own clock) are `firstmate`, a
  backfilled `firstmate` time (a `spawn_gen` epoch) is `backfill`, `observed`
  stays, and a null `at` is `unknown`.
- This home's feed covers from its `feed.started` to the adapter's latest read,
  across adapter downtime: one `coverage` segment per read that brought
  anything, at least once a minute, and on stop. Secondmate feeds add none,
  since coverage is fleet-wide.

For an attempt the feed reports on, feed events replace bridge events
(`Lifecycle::superseded` in `src/fleet/journal.rs`; the adapter's `Reach` keeps
it from writing them in the first place). A feed that recorded the attempt's
spawn, live or by backfill, holds its whole life; otherwise it holds the attempt
from the earlier of its coverage start and its first fact about the attempt.
Neither holds in a hole between the home's coverage windows, where the feed lost
events: there the bridge's status and other facts place. A bridged spawn or
teardown gives way only to the feed's own record of it, and a session join
(`bound`), which no feed knows, always stays. Bridge events stay in the journal
for removal to count; they no longer place on the timeline.

### The snapshot bridge

Where no feed speaks (Firstmate without the feed, or history before it began),
the adapter bridges lifecycle from successive `fm-fleet-snapshot.v1` polls
(`Bridge` in `scripts/firstmate-fleet.py`):

- `status` from `paths.status_log.last_event`, at `generated - age_seconds`
  (`stamp`); observed time when the age is unknown. A repeated last line is not
  re-emitted.
- `spawned` at the epoch in `spawn_gen` (`derived`); observed time otherwise.
- `bound` when the Herdr join is first made, `torn_down` when an attempt leaves
  the snapshot (a relaunch tears down the previous generation), both observed.
- `coverage` windows while it polls: a segment at least once a minute and
  whenever it emits anything else, split when polls stop for longer than its
  `max_gap` (60 s, or four poll intervals if longer), which each segment carries.
- On restart it recovers what it wrote from the journal and re-emits nothing.

Only the last status line is visible per poll, so lines that land between polls,
and anything while no bridge runs, are lost. Coverage is how the viewer knows.
Remote homes and tasks without a `spawn_gen` are left out. The bridge still
joins sessions for attempts a feed speaks for.

## Fleet timeline

One wall-clock playhead across all sessions (`src/fleet/timeline.rs`). The
overview's upstream `Timeline` holds a merged index — one mark per dated member
transcript item and per dated lifecycle event — that it never folds; it provides
the upstream scrubber, pacing, markers and transport. Its cursor drives every
member through its own seek path (`App::park_at`), so each member's model,
liveness and chips are as of that moment; following the edge, members ride their
own live edges. Dead air is compressed only while every session is quiet.

- Membership is as of the playhead: a session that has recorded nothing yet is
  absent, an attempt not yet spawned is absent, a torn-down attempt is dimmed,
  or left out while finished workers are hidden (`src/fleet/actions.rs`).
  In the past, cards read the journal, never the manifest, which describes today.
- Cards carry the attempt's status badge; `needs-decision` and `blocked` (or an
  open decision) are highlighted in amber on the card and the scrubber strip.
- Outside every coverage window, the scrubber is hatched, the header says so,
  and badges read `?`: the last record, unverified. At the live edge the same
  holds once the newest window is older than twice the `max_gap` its
  segments carry (two minutes when absent): the adapter has stopped, so
  today's state is not observed.
- Space, `[` / `]` (member prompts and lifecycle transitions), `g`/End and the
  scrubber move the one playhead. Enter opens a session at that moment with its
  own DVR; Esc rejoins the fleet's moment.

## Delivery sequence

1. **Implemented: offline multi-session prototype.** Validate a static manifest and show three
   independent Codex sessions together through the existing parsers. Test identical
   local agent/call IDs, native children, partial appends and source-local resets.
2. **Implemented, live validation pending: Firstmate integration.** Adapter with exact Herdr joins, durable membership,
   task labels, stale/unavailable state and one dedicated plugin tab. Verify its
   commands against the installed Herdr schema inside Herdr before live integration.
3. **Partial: continuity and mixed providers.** Observed worker launch generations
   link new native sessions for the same task/project. Explicit manifest links
   work across either supported provider. Automatic Captain handoff capture is
   future work. Capture explicit handoffs and new-session
   continuations, preserve completed attempts, and exercise Claude fixtures without
   launching Claude. A provider switch must not reassign old workers retroactively.
4. **Partial: combined history.** Implemented: the lifecycle journal, Firstmate's
   lifecycle feed as its source with the snapshot bridge where no feed speaks,
   and the fleet timeline over the live fleet, with coverage gaps. Future:
   mirrored remote secondmate feeds, and opening a finished crew as a replay.

Build a separate `zoe-fleet` development executable before changing any installed
plugin or shortcut. Keep the existing `zoe` and its single-session behavior intact.
Do not publish releases, crate packages or captured transcripts as part of a prototype.

## Acceptance gates

- Captain and two independent Codex workers appear simultaneously, without root,
  tool or token collisions; native children attach to the correct session.
- Re-reading the same input is idempotent. Out-of-order arrival converges to the
  same model. One source reset leaves the rest of the fleet intact.
- A resumed ID remains one session; an explicitly linked new ID is a continuation.
- Missing registration, closed panes and unavailable transcripts remain explicit.
- Worker completion is retained without conflating agent idle with task done.
- Existing native tests pass; portable-core checks continue to pass. Changes to
  shared APIs also require the browser's own wasm-target checks.
- Use sanitized synthetic fixtures first. Any later live pilot uses Codex only;
  neither the viewer nor its tests launch Claude.

## Preparation record

Local checkout: `Desktop/Agentic/zoetrope`.
Development branch: `codex/firstmate-fleet`.
`origin` points to the user's fork; `upstream` points to `furkankly/zoetrope`.
At preparation, both main branches were at the baseline commit above.
Baseline validation: `cargo test --locked` passed 216 tests (207 library, 9 CLI).
This validates the unchanged upstream starting point, not the proposed Fleet mode.

Implementation checks cover source-scoped identity/reset behavior, same-ID resume,
explicit continuation links, graph rendering, provider fixtures, buffered partial
appends, replaced-file rejection, and adapter checkpoint recovery. Runtime
membership and launch scripts still require validation inside Herdr.
