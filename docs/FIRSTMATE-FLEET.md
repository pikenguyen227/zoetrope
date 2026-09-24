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
no-mistakes CLI ----------------- attributed validation runs (read) ---+
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

Workspace (space) and tab names are weaker still: the operator renames them at
will, so they are display, never identity or a match key. The manifest keeps a
Captain's and a secondmate's location as `herdr` (`pane_id`, `tab_id`,
`workspace_id`, and the last `workspace` and `tab` names read) only to read
their current names again. The crew root takes the standing Captain's workspace
name, and a Captain or secondmate card its tab name. A task's endpoint pane is
crew, never a Captain, whatever pane the collector is handed.

Relationships have an ID, type, endpoints, evidence source and observation time:

- `delegates`: explicit Captain-to-worker assignment.
- `native_parent`: parentage stated by the provider's native records.
- `continues`: an explicitly recorded new session taking over the same work.
- `depends_on`: a task prerequisite, separate from parentage.

Do not infer these from timestamps, working directories or similar titles (the
on-demand pipeline agents below are an inference labelled as one, never an
edge or a member). Until
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
derived from the fact (never from arrival), a `source` (`bridge`, `firstmate`
or `no_mistakes`), `at` and `at_quality`, the `attempt` (`task`, `spawn_gen`),
an optional native `session`, and a `type` with its payload under a key of the
same name: `spawned`, `bound`, `status`, `decision`, `steered`, `steer_acked`,
`reclassified`, `torn_down`, `busy` or `coverage` (`{from, to, max_gap}`, an
interval someone was observing; `max_gap` in seconds is optional), and, from
the no-mistakes reads, `validation` and `validation_coverage` (see "Validation
runs" below).
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

Feed events replace the bridge events that say the same thing
(`Lifecycle::superseded` in `src/fleet/journal.rs`; the adapter's `Reach` keeps
it from writing them in the first place). A bridged spawn, teardown or status
gives way only to the feed's own record of it: the attempt's spawn, its
teardown, or, for a status, the feed event of that line. So a status the feed
lost to a skipped `seq` still places from the bridge, and a session join
(`bound`), which no feed knows, always stays. Bridge events stay in the journal
for removal to count; they no longer place on the timeline.

A status line is identified by the feed key Firstmate publishes for it: the
snapshot's `paths.status_log.last_event` carries `offset`, `stream` and
`lifecycle_key`, and `lifecycle_key` is exactly the key of that line's
`task.status` event. The bridge copies it to its status as `lifecycle_key`, and
that status gives way only to the feed event with that key (the part of its
`id` after `#`), stamped or not, provided that event has an `at`: an undated
one never places, so the bridged copy stays. A status stamped at the same
moment but keyed differently is another line and stays. The key also tells
apart two identical lines at different offsets, which the bridge writes as two
statuses.

Firstmate leaves all three fields `null` when it could not establish the
identity (an empty log, or one replaced while it was read), and an older
Firstmate omits them. Such a line is matched as before the key existed: a
stamped one by its stamp, and an unstamped one always places, even beside the
feed's record of the same line, so it can show twice. Without the key nothing
both sides share identifies an unstamped line, and no rule guessed from verb,
key or time can tell a repeat the feed lost from one it holds, so a duplicate
is accepted where a missing status is not.

### Validation runs

A ship's no-mistakes validation runs as short-lived agents with no Herdr pane
and no Firstmate task record, so the adapter reads it (`Validation` in
`scripts/firstmate-fleet.py`) and the worker's card shows it as a band, never
as graph nodes of its own.

- **Attribution is Firstmate's.** Each snapshot task's additive
  `validation_run` names the run Firstmate attributed to it (its
  `docs/configuration.md`, "Attributed validation run"); the adapter never
  matches runs by branch. A task without a `spawn_gen`, a remote task, or an
  ID that is not a no-mistakes run ID (a ULID) is left out, the last with a
  diagnostic. Once attributed, a run is read until it ends, even after its
  task leaves the snapshot: runs outlive their workers.
- **no-mistakes on the snapshot's `PATH`.** Firstmate attributes a run only
  when `command -v no-mistakes` finds the CLI, and a Herdr plugin pane's `PATH`
  rarely holds it. The adapter takes `ZOE_NO_MISTAKES_BIN`, else the first
  `no-mistakes` on its own `PATH` (`find_no_mistakes`), appends that directory
  to the snapshot's `PATH` after Herdr's, and reads runs with that binary.
  `fleet-plugin/open.sh` names the one in `.tools/bin` when its `PATH` has
  none. Without one, every `validation_run` is null, which is a manifest
  diagnostic rather than a crew that merely never validates.
- **Two allow-listed reads, nothing else.** `no-mistakes axi status --run
  <id>`, which reads the run's record without the daemon's socket, and
  `no-mistakes daemon status`, both from `/`, with
  `NO_MISTAKES_NO_UPDATE_CHECK=1` (no network) and a 5 s timeout
  (`NoMistakes`). The adapter never
  responds to, aborts, reruns, syncs or attaches to a run, never starts, stops
  or restarts the daemon, and never opens no-mistakes' database.
- **Strict parsing.** The CLI prints TOON only (`read_run`): the run's status,
  each step's status, round and findings, a waiting gate and its ask-user
  findings, outcome, PR and error. Anything it does not understand (a status
  word, a table whose rows do not match its header, indentation) is a
  manifest diagnostic and journals nothing; parts it does not read (help,
  `branch_sync`, ...) cannot fail it. The CLI's own error is not drift: a run
  `not found` is `gone`, any other error a diagnostic.
- **Written only on change.** Activity ages and process IDs are not part of a
  read, so an unchanged run writes nothing but coverage. A `validation` event
  carries `{run, phase, ...}`: `started` at the run's creation, decoded from
  its ID (`derived`); `seen` for a change, `parked` when a gate newly waits,
  `ended` when the run newly completed, failed or was cancelled, each with the
  whole state read; `daemon_down` when `daemon status` answers that the daemon
  is down; `gone` when the record can no longer be found.
- **Times are the adapter's.** The CLI's times are relative, so a read is
  stamped when the adapter made it (`observed`) and carries `since`, the read
  before it: the change happened after `since` and at or before `at`, and the
  viewer shows it as that range (`by` when no earlier read is known), never as
  an exact time. An active round's start (`round_since`) is derived from the
  age the read gave, to the second. A settled step's `duration_ms` (time
  parked at a gate excluded) is a duration, never a place on the timeline.
- **Its own coverage.** `validation_coverage` (`{run, from, to, max_gap}`)
  is each run's reading windows, like the bridge's segments, broken whenever
  a read fails, the daemon does not answer, or it answers down: lifecycle
  coverage says nothing about whether anyone read a run. Being its own type,
  a viewer that predates it skips it rather than counting it as lifecycle
  coverage.
- **A dead instrument proves nothing.** While the daemon is down its records
  may still say running, so a read is taken only when it says the run ended
  (an ended run cannot be stale); otherwise `daemon_down` is written once, and
  the run stays unverified until a read after the daemon is back (which writes
  a `seen` even when nothing changed). A timeout is neither up nor down.
- **Restarts** recover what was written from the journal: nothing is
  re-emitted, runs that had not ended are read again, and the first read after
  the restart is bounded by the last read before it.

**Pipeline agents on demand** (`src/fleet/pipeline.rs`). `p` on a card lists
the transcripts of the run its band shows and opens one read-only in the
session inspector. The join between a run and a transcript is local and
inferred. A transcript is the run's only when it is filed under the run's
worktree project key (`…-no-mistakes-worktrees-<repo>-<run>`) and its dated
records fall between the run's creation (decoded from its ULID) and the first
read that found it ended. A session filed under two runs is ambiguous. A file
that cannot be read, one with no dated record, or one with records outside
the run is listed with its reason and never opened. No agent is attributed to
a step. An opened transcript is an `Inspection` with its own watcher while
open: never a member, never in the manifest or journal, and not counted
toward the session cap. Only files are read, through the Claude provider.

The viewer folds these per attempt (`Lifecycle::validation_at`), keeping each
attempt's newest run, and checks each run against its own windows
(`run_verified`). Outside them, or after a `daemon_down` or `gone`, the band
keeps the last read and marks it `?` (unverified), and an alive run nobody was
reading hatches the scrubber too. A run that had ended cannot change, so it is
never in doubt; an open gate with ask-user findings keeps its amber, since it
stays open until someone answers it.

### The snapshot bridge

Where no feed speaks (Firstmate without the feed, or history before it began),
the adapter bridges lifecycle from successive `fm-fleet-snapshot.v1` polls
(`Bridge` in `scripts/firstmate-fleet.py`):

- `status` from `paths.status_log.last_event`, at `generated - age_seconds`
  (`stamp`); observed time when the age is unknown. It carries the line's
  `lifecycle_key` when Firstmate publishes one. A repeated last line is not
  re-emitted: the same key, or without one the same text and stamp.
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
joins sessions for attempts a feed reports on.

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
- A card whose attempt has a validation run shows it as of the playhead in its
  description's row, and its reading panel lists the steps with how each time
  is known (see "Validation runs"). The scrubber marks a run's start, a gate
  parking for a decision, and its end, each a `[` / `]` chapter; the steps
  between are not.
- Outside every coverage window, the scrubber is hatched, the header says so,
  and badges read `?`: the last record, unverified. So is a stretch where a
  validation run was alive and nobody read it. At the live edge the same
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
   the fleet timeline over the live fleet, with coverage gaps, and each
   worker's attributed validation run as a band with its own coverage, and
   a run's pipeline agents on demand, joined by inference from the run's
   worktree and each transcript's time span (an exact join needs no-mistakes
   to name each agent's native session). Future: mirrored remote secondmate
   feeds and opening a finished crew as a replay.

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
