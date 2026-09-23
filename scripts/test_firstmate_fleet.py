import copy
import importlib.util
import json
import os
import sys
from pathlib import Path
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("adapter", Path(__file__).with_name("firstmate-fleet.py"))
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)

WHEN = "2026-09-22T00:00:00Z"


def snapshot(gen="one", target="main:w1:p2"):
    return {"schema": "fm-fleet-snapshot.v1", "fm_home": "/synthetic/firstmate", "generated": WHEN,
        "tasks": [{"id": "worker", "harness": "codex", "spawn_gen": gen, "backend": "herdr", "remote": None,
            "project": "/synthetic/project", "current_state": {"state": "unknown", "source": "codex-unverified"},
            "endpoint": {"target": target}}], "backlog": {"records": []}}


def pane(session="native-one"):
    return {"pane_id": "w1:p2", "agent_status": "done",
            "agent_session": {"agent": "codex", "kind": "id", "value": session}}


class AdapterTests(unittest.TestCase):
    def build(self, snap=None, previous=None, member=None):
        snap = snap or snapshot()
        return adapter.build_manifest(snap, snap, {"main:w1:p2": member or pane()}, previous, observed=WHEN)

    def test_exact_identity_and_status_provenance(self):
        manifest = self.build()
        task = manifest["tasks"][0]
        self.assertEqual(task["session"]["session_id"], "native-one")
        self.assertEqual(task["state"]["value"], "unknown")
        self.assertEqual(task["runtime"]["value"], "done")
        self.assertEqual(manifest["links"], [])  # Current Captain isn't proven ancestry.

    def test_generation_or_endpoint_race_defers_join(self):
        for after in (snapshot("two"), snapshot(target="main:w2:p2")):
            manifest = adapter.build_manifest(snapshot(), after, {"main:w1:p2": pane()})
            self.assertIsNone(manifest["tasks"][0]["session"])
            self.assertTrue(manifest["diagnostics"])

    def test_no_guess_when_unregistered_or_wrong_provider(self):
        for member in ({"agent": "codex"}, {"agent_session": {"agent": "claude", "kind": "id", "value": "wrong"}}):
            self.assertIsNone(self.build(member=member)["tasks"][0]["session"])

    def test_new_attempt_chains_and_same_session_resume_deduplicates(self):
        first = self.build()
        resumed = self.build(snapshot("two"), first)
        self.assertEqual(len(resumed["sessions"]), 1)
        self.assertEqual(resumed["links"], [])
        continued = self.build(snapshot("two"), first, pane("native-two"))
        self.assertEqual(len(continued["sessions"]), 2)
        self.assertEqual(continued["links"][0]["kind"], "continues")
        again = self.build(snapshot("two"), continued, pane("native-two"))
        self.assertEqual(len(again["links"]), 1)

    def test_changed_session_without_new_generation_preserves_history(self):
        first = self.build()
        changed = self.build(previous=first, member=pane("unrelated"))
        self.assertEqual(changed["tasks"][0]["session"]["session_id"], "native-one")
        self.assertTrue(changed["diagnostics"])

    def test_reused_task_in_another_project_is_not_a_continuation(self):
        second = snapshot("two")
        second["tasks"][0]["project"] = "/synthetic/other-project"
        manifest = self.build(second, self.build(), pane("native-two"))
        self.assertEqual(manifest["links"], [])

    def test_cleanup_retains_nodes_and_task_evidence(self):
        first = self.build()
        empty = snapshot(); empty["tasks"] = []
        cleaned = adapter.build_manifest(empty, empty, {}, first)
        self.assertEqual(len(cleaned["sessions"]), 1)
        self.assertEqual(cleaned["tasks"][0]["runtime"]["value"], "not observed")
        self.assertEqual(cleaned["tasks"][0]["state"]["source"], "codex-unverified")

    def test_every_task_backend_unreachable_is_diagnosed(self):
        unreachable = {"state": "unknown", "source": "none",
                       "detail": "backend unreachable (herdr endpoint state: unreadable)",
                       "raw": "state: unknown · source: none · backend unreachable (herdr endpoint state: unreadable)"}
        timed_out = {"state": "unknown", "source": "none", "detail": "", "raw": ""}
        working = {"state": "working", "source": "pane", "detail": "harness busy",
                   "raw": "state: working · source: pane · harness busy"}
        cases = (([unreachable], True), ([timed_out], False), ([unreachable, timed_out], False),
                 ([unreachable, working], False), ([working], False), ([], False))
        for states, expected in cases:
            snap = snapshot()
            snap["tasks"] = [dict(snap["tasks"][0], id=f"t{n}", current_state=state)
                             for n, state in enumerate(states)]
            diagnostics = adapter.build_manifest(snap, snap, {}, observed=WHEN)["diagnostics"]
            self.assertEqual(any("backend unreachable" in d for d in diagnostics), expected, states)

    def test_remote_tasks_do_not_count_toward_unreachable(self):
        snap = snapshot()
        snap["tasks"][0].update(remote={"host": "far"}, current_state={
            "state": "unknown", "source": "none", "raw": "",
            "detail": "remote endpoint liveness not collected by fleet snapshot"})
        diagnostics = adapter.build_manifest(snap, snap, {}, observed=WHEN)["diagnostics"]
        self.assertFalse(any("backend unreachable" in d for d in diagnostics))

    def test_snapshot_path_includes_herdr_directory(self):
        with mock.patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}):
            env = adapter.snapshot_env(Path("/synthetic/firstmate"), "/opt/herdr/bin/herdr")
            self.assertEqual(env["PATH"], "/opt/herdr/bin:/usr/bin:/bin")
            self.assertEqual(env["FM_HOME"], "/synthetic/firstmate")
            self.assertEqual(adapter.snapshot_env(Path("/h"), "herdr")["PATH"], "/usr/bin:/bin")
        with mock.patch.dict(os.environ, clear=True):
            self.assertEqual(adapter.snapshot_env(Path("/h"), "/opt/herdr/bin/herdr")["PATH"],
                             "/opt/herdr/bin")

    def test_home_mismatch_fails_before_merge(self):
        other = snapshot(); other["fm_home"] = "/another/home"
        with self.assertRaises(ValueError):
            adapter.build_manifest(snapshot(), other, {})

    def test_atomic_checkpoint_recovery_and_partial_tail(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "fleet.json"
            first = self.build()
            adapter.publish(output, first, None)
            journal = output.with_suffix(".events.jsonl")
            newer = copy.deepcopy(first); newer["observed_at"] = "2026-09-22T00:01:00Z"
            adapter.publish(output, newer, first)
            self.assertEqual(len(journal.read_text().splitlines()), 1)  # No observation-only spam.
            self.assertEqual(adapter.recover(output)["observed_at"], newer["observed_at"])
            output.write_text("{partial")
            with journal.open("a") as stream:
                stream.write('{"interrupted":')
            self.assertEqual(adapter.recover(output)["fleet_id"], first["fleet_id"])
            second = self.build(snapshot("two"), first, pane("native-two"))
            adapter.publish(output, second, first)
            self.assertEqual(len(journal.read_text().splitlines()), 2)
            for line in journal.read_text().splitlines():
                json.loads(line)
            self.assertEqual(len(adapter.recover(output)["sessions"]), 2)

    def test_malformed_checkpoints_do_not_hide_valid_history(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "fleet.json"
            first = self.build()
            adapter.publish(output, first, None)
            output.write_text("null")
            with output.with_suffix(".events.jsonl").open("a") as stream:
                for value in (None, [], {}, {"schema": adapter.SCHEMA},
                              dict(first, observed_at="invalid"),
                              dict(first, observed_at="2026-09-22T00:00:00")):
                    stream.write(json.dumps({"schema": "zoetrope.fleet.journal.v1", "manifest": value}) + "\n")
                stream.write("[]\n")
            self.assertEqual(adapter.recover(output), first)


B = 1790064000  # 2026-09-22T08:00:00Z
HOME = "/synthetic/firstmate"
FLEET = "firstmate:" + HOME


def raw(epoch, verb, note, key=None):
    return f"{verb} [at={epoch}]" + (f" [key={key}]" if key else "") + f": {note}"


def fm_task(task_id, gen, now, statuses=(), target=None, stamped=True):
    """A snapshot task row whose last status line is the newest of `statuses`
    at or before `now`: (epoch, verb, note[, key])."""
    row = {"id": task_id, "kind": "ship", "harness": "codex", "project": "/synthetic/project",
           "spawn_gen": gen, "backend": "herdr", "remote": None,
           "endpoint": {"target": target or f"crew:w1:{task_id}"},
           "current_state": {"state": "working", "source": "pane"},
           "paths": {"status_log": {"present": True, "last_event": {}}}}
    seen = [entry for entry in statuses if entry[0] <= now]
    if seen:
        epoch, verb, note, *key = seen[-1]
        row["paths"]["status_log"]["last_event"] = {
            "state": verb, "note": note, "raw": raw(epoch, verb, note, *key),
            "age_seconds": now - epoch if stamped else None}
    return row


def fm_snapshot(now, tasks):
    return {"schema": "fm-fleet-snapshot.v1", "fm_home": HOME, "generated": adapter.iso(now),
            "generated_epoch": now, "tasks": tasks, "backlog": {"records": []}}


def events(lines, kind=None):
    found = [line["event"] for line in lines if line["kind"] == "lifecycle"]
    return [e for e in found if kind is None or e["type"] == kind]


def observe(bridge, now, tasks, panes=None, previous=None):
    snap = fm_snapshot(now, tasks)
    manifest = adapter.build_manifest(snap, snap, panes or {}, previous, observed=adapter.iso(now))
    return bridge.observe(snap, manifest), manifest


# The crew fixture (assets/fleet/crew): two adapter runs with a gap between
# them, three attempts. Each task: spawn_gen, pane registration time, status
# lines, and the window its metadata exists in (present while start <= t < end).
CREW = {
    "impl": {"gen": f"s{B + 100}.4101.1", "session": "c0c0c0c0-0000-4000-8000-000000000002",
             "bound": B + 150, "present": (B + 100, B + 540),
             "statuses": [(B + 110, "working", "implementing the parser"),
                          (B + 300, "needs-decision", "pick grammar A or B", "design"),
                          (B + 360, "working", "resolved: going with A"),
                          (B + 460, "done", "PR ready for review")]},
    "tests": {"gen": f"s{B + 400}.4102.2", "session": "c0c0c0c0-0000-4000-8000-000000000003",
              "bound": B + 450, "present": (B + 400, None),
              "statuses": [(B + 410, "working", "writing parser tests"),
                           (B + 570, "needs-decision", "skip or fix the flaky test?", "flaky"),
                           (B + 720, "blocked", "waiting on the impl merge"),
                           (B + 870, "working", "unblocked, finishing")]},
    "docs": {"gen": f"s{B + 660}.4103.3", "session": None, "bound": None, "present": (B + 660, B + 780),
             "statuses": [(B + 665, "working", "drafting the parser docs")]},
}
CREW_RUNS = ((f"{adapter.iso(B + 60)}/4100", range(B + 60, B + 481, 30)),
             (f"{adapter.iso(B + 600)}/4200", range(B + 600, B + 961, 30)))
CREW_JOURNAL = Path(__file__).resolve().parent.parent / "assets/fleet/crew/fleet.events.jsonl"


def crew_journal():
    """What the bridge writes for the crew scenario, line by line."""
    records, previous = [], None
    for run, polls in CREW_RUNS:
        bridge = adapter.Bridge(FLEET, run, max_gap=60, checkpoint=120)
        bridge.recover(records)
        for now in polls:
            tasks, panes = [], {}
            for task_id, spec in CREW.items():
                start, end = spec["present"]
                if start <= now and (end is None or now < end):
                    tasks.append(fm_task(task_id, spec["gen"], now, spec["statuses"]))
                    if spec["session"] and now >= spec["bound"]:
                        panes[f"crew:w1:{task_id}"] = pane(spec["session"])
            lines, previous = observe(bridge, now, tasks, panes, previous)
            records += lines
        records += bridge.close()
    return [json.dumps(record, sort_keys=True) for record in records]


class BridgeTests(unittest.TestCase):
    def bridge(self, **kwargs):
        return adapter.Bridge(FLEET, "run-1", **kwargs)

    def test_status_time_is_the_recovered_stamp(self):
        lines, _ = observe(self.bridge(), B + 61, [fm_task("t", "s1790063669.1.2", B + 61,
                                                          [(B, "working", "on it", "k")])])
        status = events(lines, "status")[0]
        self.assertEqual((status["at"], status["at_quality"]), (adapter.iso(B), "stamp"))
        self.assertEqual(status["status"]["value"], "working")
        self.assertEqual(status["status"]["key"], "k")
        self.assertEqual(status["attempt"], {"task": "t", "spawn_gen": "s1790063669.1.2"})

    def test_unstamped_status_is_observed_and_not_repeated(self):
        bridge = self.bridge()
        rows = lambda now: [fm_task("t", "opaque", now, [(B, "working", "x")], stamped=False)]
        first, _ = observe(bridge, B + 10, rows(B + 10))
        self.assertEqual([(e["at"], e["at_quality"]) for e in events(first, "status")],
                         [(adapter.iso(B + 10), "observed")])
        again, _ = observe(bridge, B + 20, rows(B + 20))
        self.assertEqual(events(again, "status"), [])

    def test_repeated_last_event_is_deduplicated_by_stamp(self):
        bridge = self.bridge()
        history = [(B, "working", "same text")]
        observe(bridge, B + 5, [fm_task("t", "g", B + 5, history)])
        repeat, _ = observe(bridge, B + 10, [fm_task("t", "g", B + 10, history)])
        self.assertEqual(events(repeat, "status"), [])
        # The same words written again later are a new line.
        history.append((B + 12, "working", "same text"))
        later, _ = observe(bridge, B + 15, [fm_task("t", "g", B + 15, history)])
        self.assertEqual([e["at"] for e in events(later, "status")], [adapter.iso(B + 12)])

    def test_spawn_time_from_spawn_gen_else_observed(self):
        cases = ((f"s{B - 30}.84654.22793", B - 30, "derived"), ("attempt-1", B, "observed"),
                 (f"s{B + 999}.1.1", B, "observed"))  # A spawn after the snapshot is not believed.
        for gen, at, quality in cases:
            lines, _ = observe(self.bridge(), B, [fm_task("t", gen, B)])
            spawned = events(lines, "spawned")[0]
            self.assertEqual((spawned["at"], spawned["at_quality"]), (adapter.iso(at), quality), gen)
            self.assertEqual(spawned["spawned"], {"kind": "ship", "harness": "codex",
                                                  "project": "/synthetic/project"})

    def test_teardown_and_relaunch_are_observed(self):
        bridge = self.bridge()
        observe(bridge, B, [fm_task("t", "one", B)])
        relaunched, _ = observe(bridge, B + 5, [fm_task("t", "two", B + 5)])
        self.assertEqual([(e["type"], e["attempt"]["spawn_gen"]) for e in events(relaunched)
                          if e["type"] != "coverage"], [("spawned", "two"), ("torn_down", "one")])
        gone, _ = observe(bridge, B + 10, [])
        down = events(gone, "torn_down")
        self.assertEqual([(e["attempt"]["spawn_gen"], e["at"], e["at_quality"]) for e in down],
                         [("two", adapter.iso(B + 10), "observed")])
        quiet, _ = observe(bridge, B + 15, [])
        self.assertEqual(events(quiet, "torn_down"), [])

    def test_bound_once_when_the_join_is_observed(self):
        bridge = self.bridge()
        rows = lambda now: [fm_task("t", "g", now, target="main:w1:p2")]
        first, manifest = observe(bridge, B, rows(B))
        self.assertEqual(events(first, "bound"), [])
        joined, manifest = observe(bridge, B + 5, rows(B + 5), {"main:w1:p2": pane()}, manifest)
        bound = events(joined, "bound")
        self.assertEqual([(e["session"]["session_id"], e["at_quality"]) for e in bound],
                         [("native-one", "observed")])
        again, _ = observe(bridge, B + 10, rows(B + 10), {"main:w1:p2": pane()}, manifest)
        self.assertEqual(events(again, "bound"), [])

    def test_remote_and_generationless_tasks_are_left_out(self):
        remote = fm_task("far", "g", B)
        remote["remote"] = {"host": "far"}
        lines, _ = observe(self.bridge(), B, [remote, fm_task("anon", None, B)])
        self.assertEqual([e for e in events(lines) if e["type"] != "coverage"], [])

    def test_coverage_chains_checkpoints_and_splits_on_gaps(self):
        bridge = self.bridge(max_gap=20, checkpoint=30)
        windows = []
        for now in (0, 5, 10, 15, 20, 25, 30, 35, 40, 100, 105):
            lines, _ = observe(bridge, B + now, [])
            windows += [(e["coverage"]["from"], e["coverage"]["to"]) for e in events(lines, "coverage")]
        windows += [(e["event"]["coverage"]["from"], e["event"]["coverage"]["to"]) for e in bridge.close()]
        t = lambda n: adapter.iso(B + n)
        # Point at the first poll, a checkpoint every 30 s, the tail when the
        # 40 -> 100 pause breaks the window, and the tail again on close.
        self.assertEqual(windows, [(t(0), t(0)), (t(0), t(30)), (t(30), t(40)),
                                   (t(100), t(100)), (t(100), t(105))])
        self.assertEqual(bridge.close(), [])

    def test_coverage_carries_the_cadence_the_viewer_waits_for(self):
        # `--interval 150` polls every 150 s, so its max_gap is 600 s; each
        # segment says so, and the viewer tolerates that much lag.
        bridge = self.bridge(max_gap=max(60, 4 * 150))
        segments = []
        for now in (0, 150, 300):
            lines, _ = observe(bridge, B + now, [])
            segments += events(lines, "coverage")
        self.assertTrue(segments)
        self.assertEqual({e["coverage"]["max_gap"] for e in segments}, {600})
        self.assertEqual(segments[-1]["coverage"]["to"], adapter.iso(B + 300))

    def test_every_line_has_the_viewer_schema(self):
        lines = crew_journal()
        self.assertTrue(lines)
        for text in lines:
            record = json.loads(text)
            self.assertEqual((record["schema"], record["kind"]), (adapter.JOURNAL, "lifecycle"))
            event = record["event"]
            self.assertTrue(event["id"].startswith("bridge:" + FLEET + "#"))
            self.assertEqual(event["source"]["kind"], "bridge")
            self.assertIn(event["at_quality"], ("stamp", "derived", "observed"))
            self.assertRegex(event["at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
            self.assertEqual("attempt" in event, event["type"] != "coverage")
            needs = {"status": "status", "coverage": "coverage", "bound": "session"}.get(event["type"])
            if needs:
                self.assertIn(needs, event)
        self.assertEqual(len({json.loads(t)["event"]["id"] for t in lines}), len(lines))

    def test_crew_fixture_is_bridge_output(self):
        # Regenerate with ZOE_REGENERATE_CREW=1 after changing the scenario.
        expected = "".join(line + "\n" for line in crew_journal())
        if os.environ.get("ZOE_REGENERATE_CREW") == "1":
            CREW_JOURNAL.write_text(expected)
        self.assertEqual(CREW_JOURNAL.read_text(), expected)

    def test_restart_resumes_without_re_emitting(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "fleet.json"
            rows = lambda now: [fm_task("t", f"s{B}.1.1", now, [(B + 1, "working", "x")],
                                        target="main:w1:p2")]
            first = self.bridge()
            lines, manifest = observe(first, B + 5, rows(B + 5), {"main:w1:p2": pane()})
            adapter.publish(output, manifest, None, lines)
            adapter.append_journal(output, first.close())
            resumed = adapter.Bridge(FLEET, "run-2")
            resumed.recover(adapter.journal_records(output))
            again, _ = observe(resumed, B + 300, rows(B + 300), {"main:w1:p2": pane()}, manifest)
            self.assertEqual([e["type"] for e in events(again)], ["coverage"])
            # A task that left while no bridge ran is torn down when noticed.
            gone, _ = observe(resumed, B + 305, [])
            self.assertEqual([(e["type"], e["at_quality"]) for e in events(gone, "torn_down")],
                             [("torn_down", "observed")])

    def test_publish_writes_lifecycle_before_its_checkpoint_and_recover_reads_both(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "fleet.json"
            first = AdapterTests.build(self)
            journal = output.with_suffix(".events.jsonl")
            journal.write_text(json.dumps({"schema": adapter.LEGACY_JOURNAL, "manifest": first}) + "\n")
            self.assertEqual(adapter.recover(output), first)
            lines, _ = observe(self.bridge(), B, [fm_task("t", "g", B)])
            second = copy.deepcopy(first); second["observed_at"] = "2026-09-22T00:05:00Z"
            second["tasks"][0]["project"] = "/elsewhere"
            adapter.publish(output, second, first, lines)
            kinds = [(r["schema"], r.get("kind")) for r in adapter.journal_records(output)]
            self.assertEqual(kinds, [(adapter.LEGACY_JOURNAL, None)]
                             + [(adapter.JOURNAL, "lifecycle")] * len(lines)
                             + [(adapter.JOURNAL, "manifest")])
            self.assertEqual(adapter.recover(output)["observed_at"], second["observed_at"])


class RemovalTests(unittest.TestCase):
    """Archive and delete: the whole fleet or one worker, on a temporary state dir."""

    GONE = {"provider": "codex", "session_id": "native-gone"}
    LIVE = {"provider": "codex", "session_id": "native-live"}

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.state = Path(self.directory.name)
        self.output = self.state / "fleet.json"
        self.journal = self.output.with_suffix(".events.jsonl")
        # Neighbours an archive or delete must leave alone.
        (self.state / "autostart.log").write_text("kept\n")
        (self.state / "backup-20260922-093107").mkdir()
        self.manifest = self.collect([("gone", "s1790064000.1.1", "native-gone"),
                                      ("live", "s1790064000.2.2", "native-live")], until=B + 20)
        # `gone` leaves the snapshot: torn down, no longer registered.
        self.manifest = self.collect([("live", "s1790064000.2.2", "native-live")],
                                     start=B + 30, until=B + 30, previous=self.manifest)

    def tearDown(self):
        self.directory.cleanup()

    def collect(self, rows, start=B + 10, until=B + 10, previous=None):
        """Polls every ten seconds through a bridge resumed from the journal,
        as a collector does."""
        bridge = adapter.Bridge(FLEET, f"run-{start}")
        bridge.recover(adapter.journal_records(self.output))
        for now in range(start, until + 1, 10):
            tasks = [fm_task(task, gen, now, [(B + 1, "working", task)]) for task, gen, _ in rows]
            panes = {f"crew:w1:{task}": pane(session) for task, _, session in rows}
            lines, manifest = observe(bridge, now, tasks, panes, previous)
            adapter.publish(self.output, manifest, previous, lines)
            previous = manifest
        adapter.append_journal(self.output, bridge.close())
        return previous

    def lifecycle(self, output=None):
        return [r["event"] for r in adapter.journal_records(output or self.output)
                if r.get("kind") == "lifecycle"]

    def tasks(self, events):
        return {(e.get("attempt") or {}).get("task") for e in events} - {None}

    def checkpoints(self):
        return [r["manifest"] for r in adapter.journal_records(self.output) if r.get("kind") == "manifest"]

    def backups(self):
        return sorted(p.name for p in self.state.glob("backup-*"))

    def test_archive_everything_then_start_fresh(self):
        before = {p.name: p.read_bytes() for p in (self.output, self.journal)}
        note = adapter.clear(self.output, "archive")
        [backup] = [p for p in self.state.glob("backup-*") if p.name != "backup-20260922-093107"]
        self.assertRegex(backup.name, r"^backup-\d{8}-\d{6}$")
        self.assertIn(str(backup), note)
        # Everything moved, byte for byte: restorable by moving it back.
        self.assertEqual({p.name: p.read_bytes() for p in backup.iterdir()}, before)
        self.assertFalse(self.output.exists() or self.journal.exists())
        self.assertIsNone(adapter.recover(self.output))
        self.assertEqual((self.state / "autostart.log").read_text(), "kept\n")
        # The next collection starts empty: only who is registered now.
        fresh = self.collect([("live", "s1790064000.2.2", "native-live")], start=B + 40, until=B + 40)
        self.assertEqual([s["key"] for s in fresh["sessions"]], [self.LIVE])
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})

    def test_delete_everything_leaves_backups_and_neighbours(self):
        self.assertEqual(adapter.clear(self.output, "delete"),
                         "deleted fleet.json and fleet.events.jsonl permanently")
        self.assertFalse(self.output.exists() or self.journal.exists())
        self.assertEqual(self.backups(), ["backup-20260922-093107"])
        self.assertEqual((self.state / "autostart.log").read_text(), "kept\n")
        self.assertEqual(adapter.clear(self.output, "delete"), "nothing to clear")

    def test_archive_one_worker(self):
        removed = [e for e in self.lifecycle() if self.tasks([e]) == {"gone"}]
        live = [e for e in self.lifecycle() if self.tasks([e]) != {"gone"}]
        worker = adapter.Worker(self.output, session=self.GONE)
        self.assertFalse(worker.registered())
        note = worker.remove("archive")
        self.assertIn(f"({len(removed)} lifecycle records)", note)
        # Gone from the live manifest, the journal and every checkpoint...
        manifest = adapter.recover(self.output)
        self.assertEqual([s["key"] for s in manifest["sessions"]], [self.LIVE])
        self.assertEqual([t["id"] for t in manifest["tasks"]], ["live"])
        self.assertEqual(self.lifecycle(), live)
        for checkpoint in self.checkpoints():
            self.assertNotIn(self.GONE, [s["key"] for s in checkpoint["sessions"]])
        # ...and kept, as a fleet of its own, in a backup named for it.
        [backup] = [p for p in self.state.glob("backup-*-gone")]
        self.assertEqual(self.lifecycle(backup / "fleet.json"), removed)
        excerpt = json.loads((backup / "fleet.json").read_text())
        self.assertEqual((excerpt["schema"], [s["key"] for s in excerpt["sessions"]]),
                         (adapter.SCHEMA, [self.GONE]))
        self.assertEqual([t["id"] for t in excerpt["tasks"]], ["gone"])

    def test_a_registered_worker_is_not_archived(self):
        before = self.journal.read_bytes(), self.output.read_bytes()
        worker = adapter.Worker(self.output, session=self.LIVE)
        self.assertTrue(worker.registered())
        with self.assertRaisesRegex(ValueError, "still registered"):
            worker.remove("archive")
        self.assertEqual((self.journal.read_bytes(), self.output.read_bytes()), before)

    def test_delete_one_worker_and_refuse_a_registered_one_unconfirmed(self):
        gone = adapter.Worker(self.output, session=self.GONE)
        self.assertIn("DELETE gone", gone.describe("delete"))
        self.assertIn("cannot be undone", gone.describe("delete"))
        gone.remove("delete")
        self.assertEqual(self.backups(), ["backup-20260922-093107"])
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})
        live = adapter.Worker(self.output, session=self.LIVE)
        self.assertIn("STILL REGISTERED", live.describe("delete"))
        with self.assertRaisesRegex(ValueError, "needs confirmation"):
            live.remove("delete")
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})
        live.remove("delete", confirmed_registered=True)
        self.assertEqual(self.tasks(self.lifecycle()), set())

    def test_a_deleted_member_stays_gone_and_a_registered_one_returns(self):
        adapter.Worker(self.output, session=self.GONE).remove("delete")
        adapter.Worker(self.output, session=self.LIVE).remove("delete", confirmed_registered=True)
        self.assertEqual(adapter.recover(self.output)["sessions"], [])
        after = self.collect([("live", "s1790064000.2.2", "native-live")], start=B + 40,
                             until=B + 40, previous=adapter.recover(self.output))
        self.assertEqual([s["key"] for s in after["sessions"]], [self.LIVE])
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})
        self.assertIn("spawned", [e["type"] for e in self.lifecycle()])

    def test_removal_is_scoped_by_attempt(self):
        # `live` relaunches under a new generation and session: the first
        # attempt's session is finished, the second is at work.
        rows = [("live", "s1790064100.3.3", "native-again")]
        self.manifest = self.collect(rows, start=B + 40, until=B + 40, previous=self.manifest)
        first = {"provider": "codex", "session_id": "native-live"}
        self.assertTrue(any(l["kind"] == "continues" for l in self.manifest["links"]))
        worker = adapter.Worker(self.output, session=first)
        self.assertEqual(worker.attempts, {("live", "s1790064000.2.2")})
        worker.remove("delete")
        manifest = adapter.recover(self.output)
        self.assertEqual([(t["id"], t["spawn_gen"]) for t in manifest["tasks"]],
                         [("gone", "s1790064000.1.1"), ("live", "s1790064100.3.3")])
        self.assertEqual(manifest["links"], [])
        kept = {tuple(e["attempt"].values()) for e in self.lifecycle() if e.get("attempt")}
        self.assertIn(("live", "s1790064100.3.3"), kept)
        self.assertNotIn(("live", "s1790064000.2.2"), kept)

    def test_a_card_without_a_session_is_its_attempt(self):
        worker = adapter.Worker(self.output, attempt=("gone", "s1790064000.1.1"))
        self.assertEqual(worker.label, "gone")
        self.assertIn("(", worker.remove("archive"))
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})

    def run_cli(self, *argv, answer=None):
        arguments = ["firstmate-fleet.py", "--output", str(self.output), *argv]
        with mock.patch("sys.argv", arguments), \
                mock.patch("builtins.input", return_value=answer) as asked, \
                mock.patch("sys.stdout"):
            code = adapter.main()
        return code, asked.called

    def test_command_line_confirms_deletes_but_not_archives(self):
        code, asked = self.run_cli("--delete", answer="n")
        self.assertEqual((code, asked), (1, True))
        self.assertTrue(self.output.exists() and self.journal.exists())
        code, asked = self.run_cli("--archive", "--worker", "gone")
        self.assertEqual((code, asked), (0, False))
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})
        code, asked = self.run_cli("--delete", "--worker", "native-live", answer="y")
        self.assertEqual((code, asked), (0, True))
        self.assertEqual(self.tasks(self.lifecycle()), set())
        code, _ = self.run_cli("--delete", answer="y")
        self.assertEqual(code, 0)
        self.assertFalse(self.output.exists() or self.journal.exists())

    def test_command_line_refuses_while_a_collector_runs(self):
        held = adapter.take_lock(self.output)
        try:
            with mock.patch("sys.stderr"), self.assertRaises(SystemExit):
                self.run_cli("--archive")
        finally:
            held.close()
        self.assertTrue(self.output.exists() and self.journal.exists())

    def test_collector_carries_out_the_viewers_request_and_reopens_it(self):
        home = self.state / "home"
        home.mkdir()
        seen = self.state / "viewer.log"
        viewer = self.state / "viewer"
        # Asks to archive everything, then reports the note it reopened with.
        viewer.write_text(f"""#!{sys.executable}
import json, os, sys
log = {str(seen)!r}
runs = open(log).read().splitlines() if os.path.exists(log) else []
with open(log, "a") as out:
    out.write(json.dumps(os.environ.get("ZOE_FLEET_NOTICE")) + "\\n")
if not runs:
    open(os.environ["ZOE_FLEET_REQUEST"], "w").write('{{"action": "archive", "scope": "all"}}')
    sys.exit({adapter.REQUEST_EXIT})
""")
        viewer.chmod(0o755)
        polls = iter(range(B + 40, B + 400, 10))

        def collect(home, herdr, previous, captain):
            now = next(polls)
            snap = fm_snapshot(now, [fm_task("live", "s1790064000.2.2", now)])
            panes = {"crew:w1:live": pane("native-live")}
            return adapter.build_manifest(snap, snap, panes, previous, observed=adapter.iso(now)), snap

        arguments = ["firstmate-fleet.py", "--home", str(home), "--output", str(self.output),
                     "--watch", "--interval", "1", "--view", str(viewer)]
        with mock.patch("sys.argv", arguments), mock.patch.dict(os.environ, {"HERDR_ENV": "1"}), \
                mock.patch.object(adapter, "collect", collect), mock.patch("sys.stdout"):
            self.assertEqual(adapter.main(), 0)
        notes = [json.loads(line) for line in seen.read_text().splitlines()]
        self.assertEqual(notes[0], None)
        self.assertRegex(notes[1], r"^archived fleet.json and fleet.events.jsonl to .*backup-")
        [backup] = [p for p in self.state.glob("backup-*") if p.name != "backup-20260922-093107"]
        self.assertIn("gone", self.tasks(self.lifecycle(backup / "fleet.json")))
        # The reopened view started empty: only the worker still registered.
        self.assertEqual([s["key"] for s in adapter.recover(self.output)["sessions"]], [self.LIVE])
        self.assertEqual(self.tasks(self.lifecycle()), {"live"})
        self.assertFalse(self.output.with_suffix(".request.json").exists())


if __name__ == "__main__":
    unittest.main()
