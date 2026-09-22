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
`pikenguyen227.zoetrope-fleet` plugin and opens a dedicated tab. The adapter refuses
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
| Tab / arrows / click | Select a node |
| Enter | Open the selected session's original inspector and timeline |
| Esc in a session | Return to the live fleet |
| x in the fleet | Collapse/expand the selected session's native children |
| r | Arrange the graph |
| o / f | Overview / follow camera |
| q or Ctrl+C | Quit |

The fleet overview is live-only. Inside a session, Space, the scrubber, `[` / `]`,
and `g` retain their existing replay/follow behavior. Returning to Fleet brings that
session to its live edge.

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

Automatic Captain lineage/handoff capture and combined historical fleet replay are
future work. The journal records observed changes, not transitions before collection
started. The manifest accepts at most 128 sessions and 4096 task attempts/links;
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
