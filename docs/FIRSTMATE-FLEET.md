# Firstmate Fleet mode: implementation proposal

Status: development prototype implemented and validated with synthetic fixtures.
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
outside the source checkout. A worker disappearing from the current snapshot must
not erase historical membership or prove task completion. Do not copy private
transcripts into the repository. Missing transcripts produce an unavailable node.

Preserve Firstmate task state, Herdr runtime state and transcript activity separately,
including unknown/stale values and their provenance. Snapshot observation time is
not an exact historical transition time. Initial integration is local-only;
remote snapshot behavior must be explicitly handled before enabling remote crews.

## UI and performance

- Fleet overview uses readable task names and retains completed nodes, dimmed.
- Selecting a node opens that session's existing tool and transcript details.
- Native children can be collapsed. Continuation and dependency edges have explicit
  labels and distinct styles; they must not override native parentage.
- Incrementally patch graph content. Preserve node positions and camera state;
  relayout deliberately rather than on every poll.
- Watch registered files incrementally. Refresh Firstmate membership at a bounded,
  slower cadence; do not run the snapshot command on every transcript poll.
- The first overview is live-only, with per-session inspection/replay. Do not
  advertise a shared historical playhead until all sources can be reconstructed
  at that playhead without leaking present-day runtime state into the past.

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
4. **Future: combined history.** Ordered lifecycle/transcript events, deterministic tie
   handling, source-local order, late-event snapshot invalidation, and fleet replay.

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
