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



# The feed fixture (assets/fleet/feed): Firstmate writes its fm-lifecycle.v1
# feed while two adapter runs, with a gap between them, tail it and bridge the
# same home. `impl` spawned before the feed began and was backfilled; `tests`
# was relaunched while no adapter ran; `survey` was promoted. Seq 19 was never
# written, and the feed rotated before seq 17.
FEED_HOME_ID = "fmh_fixture0001"
GENS = {"impl": f"s{B + 100}.4101.1", "tests": f"s{B + 400}.4102.2",
        "tests-2": f"s{B + 600}.4104.4", "survey": f"s{B + 420}.4103.3"}


def fm_record(seq, kind, task, at, source, recorded, data, key, gen=None, backfill=False):
    return {"schema": adapter.FEED, "home": {"id": FEED_HOME_ID, "path": HOME, "host": None},
            "seq": seq, "key": key, "type": kind, "at": at, "at_source": source,
            "recorded_at": recorded,
            "task": {"id": task, "spawn_gen": GENS[gen or task]} if task else None,
            "backfill": backfill, "data": data}


def spawn_data(kind, relaunch=False, previous=None):
    return {"kind": kind, "harness": "codex", "model": "default", "effort": "high",
            "mode": "no-mistakes", "yolo": "off", "project": "/synthetic/project",
            "backend": "herdr", "endpoint": {"target": "crew:w1:x"}, "relaunch": relaunch,
            "previous_spawn_gen": previous, "secondmate": None, "remote": None}


def line_data(verb, note, offset, key=None):
    return {"verb": verb, "key": key, "until": None, "note": note, "offset": offset}


def status_key(task, offset, gen=None):
    return f"status/{task}/{GENS[gen or task]}/@{offset}"


FEED_RECORDS = [
    fm_record(1, "feed.started", None, B + 200, "firstmate", B + 200, {"firstmate_rev": "abc"},
              "started"),
    fm_record(2, "task.spawned", "impl", B + 100, "firstmate", B + 200, spawn_data("ship"),
              f"spawned/impl/{GENS['impl']}", backfill=True),
    fm_record(3, "task.status", "impl", B + 110, "stamp", B + 200,
              line_data("working", "implementing the parser", 0), status_key("impl", 0), backfill=True),
    fm_record(4, "task.steered", "impl", B + 150, "inbox", B + 200,
              {"msg": "001", "delivery": "ringing", "bytes": 12, "sha256": "0" * 64},
              "steered/impl/001/2026-09-22T08:02:30Z", backfill=True),
    fm_record(5, "task.steer_acked", "impl", None, "unknown", B + 200, {"msg": "001"},
              "steer_acked/impl/001/2026-09-22T08:02:30Z", backfill=True),
    fm_record(6, "feed.backfilled", None, B + 200, "firstmate", B + 200,
              {"tasks": ["impl"], "events": 4}, f"backfilled/{B + 200}", backfill=True),
    fm_record(7, "task.status", "impl", B + 300, "stamp", B + 305,
              line_data("needs-decision", "pick grammar A or B", 60, "design"), status_key("impl", 60)),
    fm_record(8, "task.decision", "impl", B + 300, "stamp", B + 305,
              {"key": "design", "change": "opened", "verb": "needs-decision", "closed_by": None,
               "note": "pick grammar A or B"}, f"decision/impl/{GENS['impl']}/@60/design"),
    fm_record(9, "task.status", "impl", B + 360, "stamp", B + 362,
              line_data("resolved", "going with A", 140, "design"), status_key("impl", 140)),
    fm_record(10, "task.decision", "impl", B + 360, "stamp", B + 362,
              {"key": "design", "change": "closed", "verb": "needs-decision", "closed_by": "resolved",
               "note": None}, f"decision/impl/{GENS['impl']}/@140/design"),
    fm_record(11, "task.spawned", "tests", B + 400, "firstmate", B + 400, spawn_data("ship"),
              f"spawned/tests/{GENS['tests']}"),
    fm_record(12, "task.status", "tests", B + 410, "stamp", B + 415,
              line_data("working", "writing parser tests", 0), status_key("tests", 0)),
    fm_record(13, "task.spawned", "survey", B + 420, "firstmate", B + 420, spawn_data("scout"),
              f"spawned/survey/{GENS['survey']}"),
    fm_record(14, "task.reclassified", "survey", B + 440, "firstmate", B + 440,
              {"from": {"kind": "scout"}, "to": {"kind": "ship", "mode": "no-mistakes", "yolo": "off"}},
              f"reclassified/survey/{GENS['survey']}/scout-ship"),
    fm_record(15, "task.status", "impl", B + 460, "stamp", B + 470,
              line_data("done", "PR ready for review", 200), status_key("impl", 200)),
    fm_record(16, "task.torn_down", "impl", B + 500, "firstmate", B + 500,
              {"transition": "close", "outcome": "merged", "forced": False, "kind": "ship",
               "report": None, "pr": "https://example.invalid/pr/1"}, f"torn_down/impl/{GENS['impl']}"),
    fm_record(17, "task.spawned", "tests", B + 600, "firstmate", B + 600,
              spawn_data("ship", True, GENS["tests"]), f"spawned/tests/{GENS['tests-2']}", "tests-2"),
    fm_record(18, "task.status", "tests", B + 610, "stamp", B + 612,
              line_data("working", "relaunched, rerunning", 60), status_key("tests", 60), "tests-2"),
    # Seq 19 was never written.
    fm_record(20, "task.status", "tests", B + 700, "stamp", B + 703,
              line_data("needs-decision", "skip or fix the flaky test?", 160, "flaky"),
              status_key("tests", 160), "tests-2"),
    fm_record(21, "task.decision", "tests", B + 700, "stamp", B + 703,
              {"key": "flaky", "change": "opened", "verb": "needs-decision", "closed_by": None,
               "note": None}, f"decision/tests/{GENS['tests']}/@160/flaky", "tests-2"),
    fm_record(22, "task.busy", "tests", B + 705, "firstmate", B + 705, {"state": "busy", "source": "hook"},
              f"busy/tests/{GENS['tests-2']}/{B + 705}", "tests-2"),
    fm_record(23, "task.status", "tests", None, "unknown", B + 706,
              line_data(None, "continuation prose", 240), status_key("tests", 240), "tests-2"),
    fm_record(24, "task.steered", "tests", B + 750, "inbox", B + 750,
              {"msg": "001", "delivery": "ringing", "bytes": 40, "sha256": "1" * 64},
              "steered/tests/001/2026-09-22T08:12:30Z", "tests-2"),
    fm_record(25, "task.steer_acked", "tests", B + 780, "observed", B + 780, {"msg": "001"},
              "steer_acked/tests/001/2026-09-22T08:12:30Z", "tests-2"),
    fm_record(26, "task.status", "tests", B + 870, "stamp", B + 874,
              line_data("resolved", "fixing it", 300, "flaky"), status_key("tests", 300), "tests-2"),
    fm_record(27, "task.decision", "tests", B + 870, "stamp", B + 874,
              {"key": "flaky", "change": "closed", "verb": "needs-decision", "closed_by": "resolved",
               "note": None}, f"decision/tests/{GENS['tests']}/@300/flaky", "tests-2"),
]
FEED_ROTATED_BEFORE = 17
# Attempt -> (present while start <= t < end, native session and when Herdr joins it).
FEED_TASKS = {("impl", "impl"): ((B + 100, B + 500), "c0c0c0c0-0000-4000-8000-000000000012", B + 150),
              ("tests", "tests"): ((B + 400, B + 600), "c0c0c0c0-0000-4000-8000-000000000013", B + 450),
              ("tests", "tests-2"): ((B + 600, None), "c0c0c0c0-0000-4000-8000-000000000014", B + 680),
              ("survey", "survey"): ((B + 420, None), None, None)}
FEED_RUNS = ((f"{adapter.iso(B + 60)}/5100", range(B + 60, B + 481, 30)),
             (f"{adapter.iso(B + 660)}/5200", range(B + 660, B + 961, 30)))
FEED_FIXTURE = Path(__file__).resolve().parent.parent / "assets/fleet/feed"


def dump(record):
    return json.dumps(record, separators=(",", ":")) + "\n"


class FeedWriter:
    """Firstmate's side: append each record once it is recorded, rotating the
    active file into events.v1.<first-seq>.jsonl before FEED_ROTATED_BEFORE."""

    def __init__(self, directory):
        self.directory = directory
        directory.mkdir(parents=True, exist_ok=True)
        self.active = directory / "events.v1.jsonl"
        self.written = 0

    def advance(self, now):
        for record in FEED_RECORDS[self.written:]:
            if record["recorded_at"] > now:
                break
            if record["seq"] == FEED_ROTATED_BEFORE:
                os.replace(self.active, self.directory / "events.v1.1.jsonl")
            with self.active.open("a") as stream:
                stream.write(dump(record))
            self.written += 1

    def pointer(self, homes=()):
        return {"schema": adapter.FEED, "id": FEED_HOME_ID, "path": str(self.active), "present": True,
                "head_seq": FEED_RECORDS[self.written - 1]["seq"] if self.written else None,
                "homes": list(homes)}


def feed_rows(now):
    """The snapshot's task rows and Herdr panes at `now`: an attempt's last
    status line is the newest the feed dates at or before it."""
    tasks, panes = [], {}
    for (task, gen), ((start, end), session, bound) in FEED_TASKS.items():
        if not (start <= now and (end is None or now < end)):
            continue
        lines = [(r["at"], r["data"]["verb"], r["data"]["note"], *([r["data"]["key"]] if r["data"]["key"] else []))
                 for r in FEED_RECORDS if r["type"] == "task.status" and r["task"]["spawn_gen"] == GENS[gen]
                 and r["data"]["verb"] and r["seq"] != 19]
        tasks.append(fm_task(task, GENS[gen], now, lines))
        if session and now >= bound:
            panes[f"crew:w1:{task}"] = pane(session)
    return tasks, panes


def feed_journal(directory):
    """What the adapter writes while it tails the feed fixture, line by line."""
    writer = FeedWriter(directory / "lifecycle")
    records, previous = [], None
    for run, polls in FEED_RUNS:
        bridge = adapter.Bridge(FLEET, run, max_gap=60, checkpoint=120)
        bridge.recover(records)
        feeds = adapter.Feeds(directory / "fleet.feeds.json", max_gap=60, checkpoint=120)
        for now in range(polls.start - 300, polls.stop, 30):
            writer.advance(now)  # Firstmate writes whether or not an adapter runs.
            if now not in polls:
                continue
            tasks, panes = feed_rows(now)
            snap = fm_snapshot(now, tasks)
            snap["lifecycle"] = writer.pointer()
            manifest = adapter.build_manifest(snap, snap, panes, previous, observed=adapter.iso(now))
            records += adapter.lifecycle(snap, manifest, bridge, feeds)
            feeds.save()
            previous = manifest
        records += bridge.close() + feeds.close()
        feeds.save()
    return records, writer


class FeedTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def poll(self, feeds, now, pointer):
        snap = fm_snapshot(now, [])
        snap["lifecycle"] = pointer
        return feeds.poll(snap)

    def test_every_type_translates_with_honest_times(self):
        by_seq = {r["seq"]: r for r in FEED_RECORDS}
        translate = lambda seq: [line["event"] for line in adapter.translate(by_seq[seq], FEED_HOME_ID)]
        [spawned] = translate(11)
        self.assertEqual(spawned, {
            "id": f"firstmate:{FEED_HOME_ID}#spawned/tests/{GENS['tests']}",
            "source": {"kind": "firstmate", "home": FEED_HOME_ID, "seq": 11},
            "at": adapter.iso(B + 400), "at_quality": "firstmate",
            "attempt": {"task": "tests", "spawn_gen": GENS["tests"]}, "type": "spawned",
            "spawned": {"kind": "ship", "harness": "codex", "project": "/synthetic/project"}})
        # A backfilled spawn's time is its spawn_gen epoch; a backfilled stamp
        # or inbox time is as exact as a live one; an unknown one stays unknown.
        self.assertEqual([(e["type"], e.get("at"), e["at_quality"]) for seq in (2, 3, 4, 5)
                          for e in translate(seq)],
                         [("spawned", adapter.iso(B + 100), "backfill"),
                          ("status", adapter.iso(B + 110), "stamp"),
                          ("steered", adapter.iso(B + 150), "firstmate"),
                          ("steer_acked", None, "unknown")])
        payload = lambda seq: [{k: v for k, v in e.items() if k == e["type"]} for e in translate(seq)]
        self.assertEqual(payload(7), [{"status": {"value": "needs-decision", "key": "design",
                                                  "note": "pick grammar A or B"}}])
        self.assertEqual(payload(10), [{"decision": {"key": "design", "change": "closed",
                                                     "verb": "needs-decision", "closed_by": "resolved"}}])
        self.assertEqual(payload(14), [{"reclassified": {"from": "scout", "to": "ship"}}])
        self.assertEqual(payload(16), [{"torn_down": {"outcome": "merged"}}])
        self.assertEqual(payload(22), [{"busy": {"state": "busy"}}])
        self.assertEqual(payload(25), [{"steer_acked": {"msg": "001"}}])
        self.assertEqual(translate(25)[0]["at_quality"], "observed")
        # A relaunch ends the attempt it replaces, at the relaunch.
        relaunch = translate(17)
        self.assertEqual([(e["type"], e["attempt"]["spawn_gen"], e["at"]) for e in relaunch],
                         [("spawned", GENS["tests-2"], adapter.iso(B + 600)),
                          ("torn_down", GENS["tests"], adapter.iso(B + 600))])
        self.assertEqual(relaunch[1]["torn_down"], {"outcome": "relaunched"})
        # Bookkeeping, prose, unknown types and taskless records are not events.
        for seq in (1, 6, 23):
            self.assertEqual(translate(seq), [])
        self.assertEqual(adapter.translate(dict(by_seq[11], type="task.teleported"), FEED_HOME_ID), [])
        self.assertEqual(adapter.translate(dict(by_seq[11], task=None), FEED_HOME_ID), [])
        # A task without a spawn_gen is the manifest's unresolved attempt.
        orphan = dict(by_seq[11], task={"id": "far", "spawn_gen": None})
        self.assertEqual(adapter.translate(orphan, FEED_HOME_ID)[0]["event"]["attempt"],
                         {"task": "far", "spawn_gen": "unresolved"})

    def test_discovery_from_the_pointer(self):
        writer = FeedWriter(self.root / "home" / "lifecycle")
        writer.advance(B + 450)
        child = FeedWriter(self.root / "child" / "lifecycle")
        with child.active.open("w") as stream:
            stream.write(dump(dict(FEED_RECORDS[10], home={"id": "fmh_child", "path": "/c"}, seq=1)))
        homes = [{"id": "fmh_child", "task": "sm-local", "path": str(child.active), "head_seq": 1,
                  "remote": False},
                 {"id": None, "task": "sm-far", "path": None, "head_seq": None, "remote": True},
                 {"id": "fmh_gone", "task": "sm-gone", "path": str(self.root / "gone/events.v1.jsonl"),
                  "head_seq": None, "remote": False}]
        feeds = adapter.Feeds(self.root / "fleet.feeds.json")
        lines, notes = self.poll(feeds, B + 450, writer.pointer(homes))
        homes_seen = {(e["event"]["source"]["home"], e["event"]["type"]) for e in lines}
        self.assertIn(("fmh_child", "spawned"), homes_seen)
        self.assertIn((FEED_HOME_ID, "coverage"), homes_seen)
        # Only this home's feed covers the fleet.
        self.assertNotIn(("fmh_child", "coverage"), homes_seen)
        self.assertEqual(len([n for n in notes if "sm-far" in n and "remote" in n]), 1)
        self.assertEqual(len([n for n in notes if "sm-gone" in n and "unreadable" in n]), 1)
        # No pointer: the feed is off, and the bridge speaks alone.
        self.assertEqual(self.poll(feeds, B + 460, None), ([], []))
        lines, notes = self.poll(feeds, B + 460, {"schema": "fm-lifecycle.v9", "path": "/x"})
        self.assertEqual(lines, [])
        self.assertIn("not fm-lifecycle.v1", notes[0])
        # Enabled but nothing written yet is not a failure.
        empty = {"schema": adapter.FEED, "id": None, "path": str(self.root / "none/events.v1.jsonl"),
                 "present": False, "head_seq": None, "homes": []}
        self.assertEqual(self.poll(adapter.Feeds(self.root / "other.json"), B, empty), ([], []))

    def test_tailing_follows_rotation_and_waits_on_a_partial_line(self):
        writer = FeedWriter(self.root / "lifecycle")
        feeds = adapter.Feeds(self.root / "fleet.feeds.json")
        writer.advance(B + 470)
        seqs = lambda lines: [e["event"]["source"].get("seq") for e in lines
                              if e["event"]["type"] != "coverage"]
        first, _ = self.poll(feeds, B + 470, writer.pointer())
        self.assertEqual(seqs(first), [2, 3, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15])
        # Half a line is still being written: it waits for its newline.
        text = dump(FEED_RECORDS[15])
        with writer.active.open("a") as stream:
            stream.write(text[:30])
        self.assertEqual(seqs(self.poll(feeds, B + 480, writer.pointer())[0]), [])
        with writer.active.open("a") as stream:
            stream.write(text[30:])
        writer.written += 1
        self.assertEqual(seqs(self.poll(feeds, B + 490, writer.pointer())[0]), [16])
        # Rotation renames the file being read; its tail and the new file follow.
        writer.advance(B + 612)
        self.assertTrue((writer.directory / "events.v1.1.jsonl").exists())
        self.assertEqual(seqs(self.poll(feeds, B + 620, writer.pointer())[0]), [17, 17, 18])
        # A restarted adapter resumes from the saved cursor.
        feeds.save()
        again = adapter.Feeds(self.root / "fleet.feeds.json")
        self.assertEqual(seqs(self.poll(again, B + 630, writer.pointer())[0]), [])
        # A cursor lost with its file restarts from the rotated file that
        # holds the next seq and keeps only what is new.
        again.cursors[str(writer.active)]["file"] = [0, 0]
        writer.advance(B + 703)
        self.assertEqual(seqs(self.poll(again, B + 710, writer.pointer())[0]), [20, 21])

    def test_a_seq_gap_is_reported_not_dropped_silently(self):
        writer = FeedWriter(self.root / "lifecycle")
        writer.advance(B + 612)
        feeds = adapter.Feeds(self.root / "fleet.feeds.json")
        _, notes = self.poll(feeds, B + 612, writer.pointer())
        self.assertEqual(notes, [])
        writer.advance(B + 703)
        _, notes = self.poll(feeds, B + 703, writer.pointer())
        self.assertEqual(notes, ["Firstmate lifecycle feed: 1 events lost in 1 seq gaps (latest 19-19); "
                                 "the timeline lacks them"])
        # It stays reported: the history has a hole.
        feeds.save()
        _, notes = self.poll(adapter.Feeds(self.root / "fleet.feeds.json"), B + 900, writer.pointer())
        self.assertEqual(len(notes), 1)
        # Unreadable lines are counted too.
        with writer.active.open("a") as stream:
            stream.write("{not json\n")
        _, notes = self.poll(feeds, B + 910, writer.pointer())
        self.assertIn("1 unreadable lines skipped", notes[0])

    def test_feed_coverage_spans_adapter_downtime(self):
        records, _ = feed_journal(self.root)
        windows = [(e["coverage"]["from"], e["coverage"]["to"]) for e in events(records, "coverage")
                   if e["source"]["kind"] == "firstmate"]
        t = lambda n: adapter.iso(B + n)
        # From feed.started, chained, straight across B+480..B+660 when no
        # adapter ran: Firstmate kept writing.
        self.assertEqual(windows[0][0], t(200))
        self.assertEqual(windows[-1][1], t(960))
        self.assertTrue(all(a[1] == b[0] for a, b in zip(windows, windows[1:])))

    def test_feed_replaces_the_bridge_for_an_attempt(self):
        records, _ = feed_journal(self.root)
        about = lambda task, gen, source: [
            (e["type"], e.get("at")) for e in events(records)
            if e.get("attempt") == {"task": task, "spawn_gen": GENS[gen]} and e["source"]["kind"] == source]
        # Before the feed began, the bridge saw impl; once the feed backfilled
        # its spawn, the bridge only joins its session.
        self.assertEqual(about("impl", "impl", "bridge"),
                         [("spawned", adapter.iso(B + 100)), ("status", adapter.iso(B + 110)),
                          ("bound", adapter.iso(B + 150))])
        self.assertEqual([k for k, _ in about("impl", "impl", "firstmate")],
                         ["spawned", "status", "steered", "steer_acked", "status", "decision",
                          "status", "decision", "status", "torn_down"])
        # The relaunch happened while no adapter ran: the feed has it at its
        # real time and the bridge, noticing later, writes nothing of it.
        self.assertEqual(about("tests", "tests", "bridge"), [("bound", adapter.iso(B + 450))])
        self.assertIn(("torn_down", adapter.iso(B + 600)), about("tests", "tests", "firstmate"))
        self.assertEqual(about("tests", "tests-2", "bridge"), [("bound", adapter.iso(B + 690))])
        # Reach itself: whole life after a spawn; otherwise from the feed's start.
        reach = adapter.Reach()
        reach.add({"source": {"kind": "firstmate", "home": "h"}, "type": "coverage",
                   "at": adapter.iso(B + 60), "coverage": {"from": adapter.iso(B), "to": adapter.iso(B + 60)}})
        reach.add({"source": {"kind": "firstmate", "home": "h"}, "type": "status",
                   "at": adapter.iso(B + 30), "attempt": {"task": "old", "spawn_gen": "g"}})
        self.assertFalse(reach.covers(("old", "g"), "status", B - 1))
        self.assertTrue(reach.covers(("old", "g"), "status", B))
        self.assertFalse(reach.covers(("old", "g"), "spawned", B + 10))
        self.assertFalse(reach.covers(("old", "g"), "torn_down", B + 10))
        self.assertFalse(reach.covers(("other", "g"), "status", B + 10))

    def test_feed_fixture_is_adapter_output(self):
        # Regenerate with ZOE_REGENERATE_CREW=1 after changing the scenario.
        records, writer = feed_journal(self.root)
        journal = "".join(json.dumps(r, sort_keys=True) + "\n" for r in records)
        feed = {name: (writer.directory / name).read_text()
                for name in ("events.v1.1.jsonl", "events.v1.jsonl")}
        if os.environ.get("ZOE_REGENERATE_CREW") == "1":
            (FEED_FIXTURE / "lifecycle").mkdir(parents=True, exist_ok=True)
            (FEED_FIXTURE / "fleet.events.jsonl").write_text(journal)
            for name, text in feed.items():
                (FEED_FIXTURE / "lifecycle" / name).write_text(text)
        self.assertEqual((FEED_FIXTURE / "fleet.events.jsonl").read_text(), journal)
        for name, text in feed.items():
            self.assertEqual((FEED_FIXTURE / "lifecycle" / name).read_text(), text)


if __name__ == "__main__":
    unittest.main()
