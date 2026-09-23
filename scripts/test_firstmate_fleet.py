import copy
import errno
import importlib.util
import itertools
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

    def test_the_workspace_names_the_fleet(self):
        snap = snapshot()
        named = adapter.build_manifest(snap, snap, {}, observed=WHEN, workspace="Control Tower")
        self.assertEqual(named["label"], "Control Tower")
        self.assertEqual(self.build()["label"], "Firstmate · Control Tower")

    def test_workspace_name_asks_herdr_for_the_captains_workspace(self):
        answer = {"result": {"workspace": {"workspace_id": "wA", "label": " Control Tower "}}}
        with mock.patch.object(adapter, "run_json", return_value=answer) as run, \
                mock.patch.dict(os.environ, {"HERDR_WORKSPACE_ID": "wB"}):
            self.assertEqual(adapter.workspace_name("herdr", {"workspace_id": "wA"}), "Control Tower")
            self.assertEqual(run.call_args.args[0], ["herdr", "workspace", "get", "wA"])
            # Without a Captain pane, the adapter's own workspace.
            adapter.workspace_name("herdr", None)
            self.assertEqual(run.call_args.args[0], ["herdr", "workspace", "get", "wB"])
        # Unknown, unreachable or unnamed: None, so the fleet keeps its default name.
        with mock.patch.dict(os.environ, clear=True):
            self.assertIsNone(adapter.workspace_name("herdr", None))
        for failure in (RuntimeError("no socket"), {"result": {}}, {"result": {"workspace": {"label": ""}}}):
            effect = {"side_effect": failure} if isinstance(failure, Exception) else {"return_value": failure}
            with mock.patch.object(adapter, "run_json", **effect):
                self.assertIsNone(adapter.workspace_name("herdr", {"workspace_id": "wA"}))

    def test_only_the_registered_captain_stands(self):
        def captain(session=None):
            record = {"pane_id": "w1:p1", "workspace_id": "w1"}
            if session:
                record["agent_session"] = {"agent": "claude", "kind": "id", "value": session}
            return record

        def build(previous, record):
            snap = snapshot()
            return adapter.build_manifest(snap, snap, {"main:w1:p2": pane()}, previous, record, WHEN)

        runtime = lambda m, sid: next(s for s in m["sessions"]
                                      if s["key"]["session_id"] == sid).get("runtime")
        first = build(None, captain("captain-one"))
        self.assertIsNone(runtime(first, "captain-one"))
        # A later Captain registers: the earlier one stays, no longer registered.
        second = build(first, captain("captain-two"))
        self.assertEqual([s["key"]["session_id"] for s in second["sessions"]],
                         ["captain-one", "native-one", "captain-two"])
        self.assertEqual(runtime(second, "captain-one")["value"], "not observed")
        self.assertIsNone(runtime(second, "captain-two"))
        self.assertIsNone(runtime(second, "native-one"))  # a worker is judged by its task
        # The pane goes quiet (or is a Shell): the last Captain registered stands,
        # and the diagnostic says so rather than waiting for one.
        quiet = build(second, captain())
        self.assertEqual(runtime(quiet, "captain-one")["value"], "not observed")
        self.assertIsNone(runtime(quiet, "captain-two"))
        self.assertIn("Captain pane w1:p1 has no supported registered session; "
                      "showing the last Captain registered", quiet["diagnostics"])
        self.assertIn("Captain pane has no supported registered session yet",
                      build(None, captain())["diagnostics"])
        # The earlier Captain registering again takes its place back.
        back = build(quiet, captain("captain-one"))
        self.assertIsNone(runtime(back, "captain-one"))
        self.assertEqual(runtime(back, "captain-two")["value"], "not observed")
        # Archiving the earlier Captain needs no confirmation; the standing one does.
        with mock.patch.object(adapter, "recover", return_value=quiet), \
                mock.patch.object(adapter, "journal_records", return_value=[]):
            key = lambda sid: {"provider": "claude", "session_id": sid}
            self.assertFalse(adapter.Worker(Path("fleet.json"), session=key("captain-one")).registered())
            self.assertTrue(adapter.Worker(Path("fleet.json"), session=key("captain-two")).registered())

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


def identify(row, offset):
    """Give a task row's last status line the feed identity Firstmate
    publishes for it."""
    stream = row["spawn_gen"]
    row["paths"]["status_log"]["last_event"].update(
        offset=offset, stream=stream, lifecycle_key=f"status/{row['id']}/{stream}/@{offset}")
    return row


def fed(key, at, verb, note, at_source="stamp"):
    """The journal event the feed's record of a line of t's becomes."""
    record = {"schema": adapter.FEED, "seq": 1, "key": key, "type": "task.status", "at": at,
              "at_source": at_source, "task": {"id": "t", "spawn_gen": "one"},
              "data": {"verb": verb, "key": None, "note": note}}
    return adapter.translate(record, "fmh")[0]["event"]


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

    def test_a_status_the_feed_lost_is_bridged_at_its_stamp(self):
        # The feed holds t's whole life and covers without a break, but lost
        # the status stamped B+90 (its record was never written).
        bridge = self.bridge()
        feed = lambda kind, at, **extra: dict(
            {"source": {"kind": "firstmate", "home": "h"}, "type": kind, "at": adapter.iso(at),
             "attempt": {"task": "t", "spawn_gen": "one"}}, **extra)
        bridge.reach.add(feed("coverage", B + 120, attempt=None,
                              coverage={"from": adapter.iso(B), "to": adapter.iso(B + 120)}))
        bridge.reach.add(feed("spawned", B))
        bridge.reach.add(feed("status", B + 60, at_quality="stamp"))
        statuses = [(B + 60, "working", "held"), (B + 90, "needs-decision", "lost", "design")]
        held, _ = observe(bridge, B + 70, [fm_task("t", "one", B + 70, statuses)])
        self.assertEqual(events(held, "status"), [])
        lost, _ = observe(bridge, B + 100, [fm_task("t", "one", B + 100, statuses)])
        self.assertEqual([(e["at"], e["at_quality"], e["status"]["value"]) for e in events(lost, "status")],
                         [(adapter.iso(B + 90), "stamp", "needs-decision")])

    def test_an_unstamped_status_is_always_bridged(self):
        # No identity links an unstamped line to the feed's record of it, so
        # the bridge writes it even beside the feed's (a duplicate is
        # accepted), and a repeated one the feed lost is not missed.
        bridge = self.bridge()
        bridge.reach.add({"source": {"kind": "firstmate", "home": "h"}, "type": "spawned",
                          "at": adapter.iso(B), "at_quality": "firstmate",
                          "attempt": {"task": "t", "spawn_gen": "one"}})
        bridge.reach.add({"source": {"kind": "firstmate", "home": "h"}, "type": "status",
                          "at": adapter.iso(B + 65), "at_quality": "observed",
                          "attempt": {"task": "t", "spawn_gen": "one"},
                          "status": {"value": "working"}})
        statuses = [(B + 60, "working", "on it"), (B + 90, "working", "on it again")]
        seen = []
        for now in (B + 70, B + 100):
            lines, _ = observe(bridge, now, [fm_task("t", "one", now, statuses, stamped=False)])
            seen += [(e["at"], e["at_quality"], e["status"]["note"]) for e in events(lines, "status")]
        self.assertEqual(seen, [(adapter.iso(B + 70), "observed", "on it"),
                                (adapter.iso(B + 100), "observed", "on it again")])

    def test_a_keyed_status_gives_way_only_to_the_feed_event_of_its_key(self):
        # Firstmate names each polled line by its feed key, so the feed's
        # record of a line replaces the bridged copy exactly, stamped or not.
        bridge = self.bridge()
        bridge.reach.add(fed("status/t/one/@0", B + 60, "working", "on it"))
        statuses = [(B + 60, "working", "on it"), (B + 60, "blocked", "ci", "ci")]
        held, _ = observe(bridge, B + 70, [identify(fm_task("t", "one", B + 70, statuses[:1],
                                                            stamped=False), 0)])
        self.assertEqual(events(held, "status"), [])
        # Another line stamped at that same moment is not the feed's record,
        # though a stamp alone would have matched it.
        other, _ = observe(bridge, B + 100, [identify(fm_task("t", "one", B + 100, statuses), 30)])
        self.assertEqual([(e["at"], e["status"]["value"], e["status"]["lifecycle_key"])
                          for e in events(other, "status")],
                         [(adapter.iso(B + 60), "blocked", "status/t/one/@30")])

    def test_an_undated_feed_status_does_not_hold_its_key(self):
        # A feed record without an at never places, so the bridge still
        # writes the line it names.
        bridge = self.bridge()
        undated = fed("status/t/one/@0", None, "working", "on it", at_source=None)
        self.assertEqual((undated.get("at"), undated["at_quality"]), (None, "unknown"))
        bridge.reach.add(undated)
        row = identify(fm_task("t", "one", B + 70, [(B + 60, "working", "on it")]), 0)
        lines, _ = observe(bridge, B + 70, [row])
        self.assertEqual([e["status"]["lifecycle_key"] for e in events(lines, "status")],
                         ["status/t/one/@0"])

    def test_a_status_without_identity_is_matched_as_before(self):
        # All three identity fields null (or absent, as test_an_unstamped_
        # status_is_always_bridged shows): a stamp still matches the feed's
        # record, and an unstamped line is bridged beside it.
        for stamped, expected in ((True, []), (False, [adapter.iso(B + 70)])):
            bridge = self.bridge()
            bridge.reach.add(fed("status/t/one/@0", B + 60, "working", "on it"))
            row = fm_task("t", "one", B + 70, [(B + 60, "working", "on it")], stamped=stamped)
            row["paths"]["status_log"]["last_event"].update(offset=None, stream=None, lifecycle_key=None)
            lines, _ = observe(bridge, B + 70, [row])
            self.assertEqual([e["at"] for e in events(lines, "status")], expected, stamped)
            self.assertNotIn("lifecycle_key", json.dumps(lines))

    def test_identical_unstamped_lines_at_two_offsets_are_two_lines(self):
        def row(now, offset):
            task = fm_task("t", "one", now, [(B, "working", "on it")], stamped=False)
            task["paths"]["status_log"]["last_event"]["raw"] = "working: on it"
            return identify(task, offset) if offset is not None else task
        bridge, seen = self.bridge(), []
        for now, offset in ((B + 10, 0), (B + 20, 0), (B + 30, 40)):
            lines, _ = observe(bridge, now, [row(now, offset)])
            seen += lines
        self.assertEqual([(e["at"], e["status"]["lifecycle_key"]) for e in events(seen, "status")],
                         [(adapter.iso(B + 10), "status/t/one/@0"),
                          (adapter.iso(B + 30), "status/t/one/@40")])
        # A restart knows the line it last wrote, and a poll without identity
        # compares it by words, as before.
        again = adapter.Bridge(FLEET, "run-2")
        again.recover(seen)
        for now, offset in ((B + 40, 40), (B + 50, None)):
            lines, _ = observe(again, now, [row(now, offset)])
            self.assertEqual(events(lines, "status"), [])

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
    status line is the newest the feed dates at or before it. Firstmate
    publishes impl's lines' feed identity; the other attempts' lines carry
    none, so they are matched by stamp."""
    tasks, panes = [], {}
    for (task, gen), ((start, end), session, bound) in FEED_TASKS.items():
        if not (start <= now and (end is None or now < end)):
            continue
        records = [r for r in FEED_RECORDS if r["type"] == "task.status" and r["task"]["spawn_gen"] == GENS[gen]
                   and r["data"]["verb"] and r["seq"] != 19]
        lines = [(r["at"], r["data"]["verb"], r["data"]["note"], *([r["data"]["key"]] if r["data"]["key"] else []))
                 for r in records]
        row = fm_task(task, GENS[gen], now, lines)
        seen = [r for r in records if r["at"] <= now]
        if gen == "impl" and seen:
            identify(row, seen[-1]["data"]["offset"])
        tasks.append(row)
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
        # A relaunch is the new attempt's spawn; it invents no teardown.
        self.assertEqual([(e["type"], e["attempt"]["spawn_gen"], e["at"]) for e in translate(17)],
                         [("spawned", GENS["tests-2"], adapter.iso(B + 600))])
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
        self.assertEqual(seqs(self.poll(feeds, B + 620, writer.pointer())[0]), [17, 18])
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
        # adapter ran: Firstmate kept writing. Only the lost seq 19 breaks it,
        # from what was covered before it to seq 20's recording.
        self.assertEqual(windows[0][0], t(200))
        self.assertEqual(windows[-1][1], t(960))
        self.assertEqual([(a[1], b[0]) for a, b in zip(windows, windows[1:]) if a[1] != b[0]],
                         [(t(660), t(703))])

    def test_feed_replaces_the_bridge_for_an_attempt(self):
        records, _ = feed_journal(self.root)
        about = lambda task, gen, source: [
            (e["type"], e.get("at")) for e in events(records)
            if e.get("attempt") == {"task": task, "spawn_gen": GENS[gen]} and e["source"]["kind"] == source]
        # Before the feed began, the bridge saw impl; once the feed backfilled
        # its spawn, the bridge only joins its session, and writes a stamped
        # status only when it sees it before Firstmate has recorded it. The
        # feed then records it at that same stamp, which it replaces.
        self.assertEqual([e for e in about("impl", "impl", "bridge") if e[0] != "status"],
                         [("spawned", adapter.iso(B + 100)), ("bound", adapter.iso(B + 150))])
        for task, gen in (("impl", "impl"), ("tests", "tests"), ("tests", "tests-2")):
            self.assertLessEqual({e for e in about(task, gen, "bridge") if e[0] == "status"},
                                 set(about(task, gen, "firstmate")))
        self.assertEqual([k for k, _ in about("impl", "impl", "firstmate")],
                         ["spawned", "status", "steered", "steer_acked", "status", "decision",
                          "status", "decision", "status", "torn_down"])
        # The relaunch happened while no adapter ran: the feed has the new
        # spawn at its real time; the old attempt's end is the bridge's, noticed.
        self.assertEqual(about("tests", "tests", "bridge"),
                         [("bound", adapter.iso(B + 450)), ("torn_down", adapter.iso(B + 660))])
        self.assertNotIn("torn_down", [k for k, _ in about("tests", "tests", "firstmate")])
        self.assertEqual([e for e in about("tests", "tests-2", "bridge") if e[0] != "status"],
                         [("bound", adapter.iso(B + 690))])
        # Reach itself: a bridged fact gives way only to the feed's own record.
        reach = adapter.Reach()
        fact = lambda source, kind, at, quality="observed", task="old", **extra: dict(
            {"source": {"kind": source, "home": "h"}, "type": kind, "at": adapter.iso(at),
             "at_quality": quality, "attempt": {"task": task, "spawn_gen": "g"}}, **extra)
        said = lambda value, key=None: {"status": dict({"value": value}, **({"key": key} if key else {}))}
        reach.add(fact("firstmate", "coverage", B + 60, attempt=None,
                       coverage={"from": adapter.iso(B), "to": adapter.iso(B + 60)}))
        reach.add(fact("firstmate", "spawned", B - 50, "firstmate"))
        reach.add(fact("firstmate", "status", B + 120, "stamp", **said("working")))
        reach.add(fact("firstmate", "status", B + 30, **said("blocked", "ci")))
        reach.add(fact("bridge", "torn_down", B + 200))
        self.assertTrue(reach.holds(fact("bridge", "spawned", B, "derived")))
        self.assertFalse(reach.holds(fact("bridge", "torn_down", B + 200)))
        # A stamped status, by its stamp.
        self.assertTrue(reach.holds(fact("bridge", "status", B + 120, "stamp", **said("working"))))
        self.assertFalse(reach.holds(fact("bridge", "status", B + 130, "stamp", **said("working"))))
        # A status without a stamp has no identity the feed shares, so none
        # holds it: not one saying the same, and not a repeat the feed lost.
        for at in (B + 20, B + 40, B + 110, B + 130):
            for value in (said("blocked", "ci"), said("working")):
                self.assertFalse(reach.holds(fact("bridge", "status", at, **value)))

    def test_a_failed_poll_rereads_what_it_did_not_publish(self):
        writer = FeedWriter(self.root / "lifecycle")
        output = self.root / "fleet.json"
        polls = iter([B + 300, B + 330, B + 360])

        def collect(home, herdr, previous, captain):
            try:
                now = next(polls)
            except StopIteration:
                raise KeyboardInterrupt
            writer.advance(now)
            tasks, panes = feed_rows(now)
            snap = fm_snapshot(now, tasks)
            snap["lifecycle"] = writer.pointer()
            return adapter.build_manifest(snap, snap, panes, previous, observed=adapter.iso(now)), snap

        append, calls = adapter.append_journal, []

        def failing(path, records):
            calls.append(len(records))
            if len(calls) == 2:
                raise OSError(errno.ENOSPC, "no space left on device")
            append(path, records)

        clock = itertools.count(step=10)
        arguments = ["firstmate-fleet.py", "--home", str(self.root), "--output", str(output),
                     "--watch", "--interval", "1"]
        with mock.patch("sys.argv", arguments), mock.patch.dict(os.environ, {"HERDR_ENV": "1"}), \
                mock.patch.object(adapter, "collect", collect), \
                mock.patch.object(adapter, "append_journal", failing), \
                mock.patch.object(adapter.time, "monotonic", lambda: next(clock)), \
                mock.patch("sys.stdout"):
            self.assertEqual(adapter.main(), 0)
        journal = [json.loads(line) for line in output.with_suffix(".events.jsonl").read_text().splitlines()]
        seqs = {r["event"]["source"].get("seq") for r in journal if r["kind"] == "lifecycle"}
        # Seqs 7 and 8 were read by the poll that could not write them.
        self.assertEqual(seqs - {None}, {r["seq"] for r in FEED_RECORDS if r["recorded_at"] <= B + 360
                                         and adapter.translate(r, FEED_HOME_ID)})

    def test_a_failed_poll_keeps_the_coverage_the_bridge_watched(self):
        output = self.root / "fleet.json"
        polls = iter([B, B + 5, B + 10, B + 15, None, B + 100])

        def collect(home, herdr, previous, captain):
            try:
                now = next(polls)
            except StopIteration:
                raise KeyboardInterrupt
            if now is None:
                raise RuntimeError("herdr hiccup")
            snap = fm_snapshot(now, [])
            return adapter.build_manifest(snap, snap, {}, previous, observed=adapter.iso(now)), snap

        clock = itertools.count(step=10)
        arguments = ["firstmate-fleet.py", "--home", str(self.root), "--output", str(output),
                     "--watch", "--interval", "1"]
        with mock.patch("sys.argv", arguments), mock.patch.dict(os.environ, {"HERDR_ENV": "1"}), \
                mock.patch.object(adapter, "collect", collect), \
                mock.patch.object(adapter.time, "monotonic", lambda: next(clock)), \
                mock.patch("sys.stdout"), mock.patch("sys.stderr"):
            self.assertEqual(adapter.main(), 0)
        journal = [json.loads(line) for line in output.with_suffix(".events.jsonl").read_text().splitlines()]
        windows = {(e["coverage"]["from"], e["coverage"]["to"]) for e in events(journal, "coverage")}
        self.assertIn((adapter.iso(B), adapter.iso(B + 15)), windows)

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



# The validation fixture (assets/fleet/validation): the crew scenario's two
# adapter runs also read the no-mistakes run Firstmate attributes to impl and
# to tests. Each read returns the newest status capture under status/.
NM_FIXTURE = Path(__file__).resolve().parent.parent / "assets/fleet/validation"
IMPL_RUN, TESTS_RUN = "01M342G5B2S1MP13RVNVA11DAT", "01M342X98JT3STSRVNVA11DAT1"
# Run -> (task it is attributed to, from when, [(from when, capture read)]).
NM_RUNS = {IMPL_RUN: ("impl", B + 470, [(B + 470, "impl-review"), (B + 540, "impl-test"),
                                        (B + 645, "impl-ci"), (B + 830, "impl-passed")]),
           TESTS_RUN: ("tests", B + 896, [(B + 896, "tests-review"), (B + 940, "tests-parked")])}
NM_DOWN = {B + 930}  # when `daemon status` answers down


def capture(name):
    return (NM_FIXTURE / "status" / f"{name}.toon").read_text()


class FakeNoMistakes:
    """no-mistakes as the scenario's clock sees it, recording every call."""

    def __init__(self, clock, runs=None, down=()):
        self.clock, self.runs, self.down, self.calls = clock, runs or NM_RUNS, down, []

    def daemon(self):
        self.calls.append(("daemon", "status"))
        answer = self.down(self.clock()) if callable(self.down) else self.clock() in self.down
        return answer if isinstance(answer, str) else ("down" if answer else "up")

    def status(self, run_id):
        self.calls.append(("axi", "status", "--run", run_id))
        seen = [name for start, name in self.runs[run_id][2] if start <= self.clock()]
        if not seen:
            raise adapter.Unreadable(f'run "{run_id}" not found')
        return seen[-1] if seen[-1].startswith(("run:", "error:")) else capture(seen[-1])


def attributed(task_id, now, runs=NM_RUNS):
    """The snapshot's validation_run for a task at `now`."""
    found = [run for run, (owner, since, _) in runs.items() if owner == task_id and since <= now]
    return {"id": found[-1], "branch": f"fm/{task_id}", "status": "running", "outcome": None,
            "step": None, "step_status": None, "head": None, "pr": None} if found else None


def validation_journal():
    """What the collector writes for the crew scenario, line by line."""
    records, now = [], [0]
    reader = FakeNoMistakes(lambda: now[0], down=NM_DOWN)
    for run, polls in CREW_RUNS:
        validation = adapter.Validation(FLEET, run, max_gap=60, checkpoint=120, reader=reader,
                                        clock=lambda: now[0])
        validation.recover(records)
        for t in polls:
            now[0] = t
            tasks = []
            for task_id, spec in CREW.items():
                start, end = spec["present"]
                if start <= t and (end is None or t < end):
                    row = fm_task(task_id, spec["gen"], t, spec["statuses"])
                    row["validation_run"] = attributed(task_id, t)
                    tasks.append(row)
            lines, _ = validation.observe(fm_snapshot(t, tasks))
            records += lines
        records += validation.close()
    return [json.dumps(record, sort_keys=True) for record in records]


class ToonTests(unittest.TestCase):
    def read(self, name, run=IMPL_RUN):
        return adapter.read_run(capture(name), run)

    def test_every_capture_reads_as_its_state(self):
        state, ages = self.read("impl-review")
        self.assertEqual(state["status"], "running")
        self.assertEqual([s["step"] for s in state["steps"]],
                         ["intent", "rebase", "review", "test", "document", "lint", "push", "pr", "ci"])
        self.assertEqual(state["steps"][2], {"step": "review", "status": "running", "findings": 0,
                                             "round": "round 1"})
        self.assertEqual(ages, {"review": 12})
        # A settled step keeps how long it ran; an active one changes, so not.
        self.assertEqual(state["steps"][1]["duration_ms"], 2204)
        self.assertNotIn("duration_ms", state["steps"][3])
        state, ages = self.read("impl-ci")
        self.assertEqual(state["pr"], "https://github.com/example/synthetic/pull/12")
        self.assertEqual((state["steps"][-1]["status"], ages), ("running", {"ci": 15}))
        state, ages = self.read("impl-passed")
        self.assertEqual((state["status"], state["outcome"], ages), ("completed", "passed", {}))
        state, _ = self.read("tests-parked", TESTS_RUN)
        self.assertEqual(state["gate"], {"step": "review", "status": "awaiting_approval",
                                          "findings": 2, "ask_user": 1})
        state, _ = adapter.read_run(capture("cancelled"), "01M342B46TCANC311EDRVN0000")
        self.assertEqual((state["outcome"], state["error"]),
                         ("cancelled", "cancelled: superseded by new push"))
        self.assertEqual(state["steps"][0], {"step": "intent", "status": "skipped", "findings": 0,
                                             "duration_ms": 14634})

    def test_nothing_that_changes_on_every_read_is_state(self):
        text = capture("impl-review")
        later = text.replace('"3s ago: claude producing output","41001"', '"1s ago: tool call","41999"')
        later = later.replace("12s,12s", "42s,42s")
        self.assertEqual(adapter.read_run(text, IMPL_RUN)[0], adapter.read_run(later, IMPL_RUN)[0])

    def test_the_clis_own_error_is_unreadable_not_drift(self):
        with self.assertRaises(adapter.Unreadable) as raised:
            adapter.read_run(capture("not-found"), IMPL_RUN)
        self.assertEqual(str(raised.exception), f'run "{IMPL_RUN}" not found')

    def test_parts_it_does_not_read_are_left_alone(self):
        text = capture("impl-test") + "branch_sync:\n  state: pipeline_owned\n  next_action: wait\n" \
            + 'help[3]:\n  Run `no-mistakes axi logs`, then decide\n  "quoted, with commas"\n  x\n'
        self.assertEqual(adapter.read_run(text, IMPL_RUN), self.read("impl-test"))
        # A run an explicit --run names on another branch reads the same.
        self.assertEqual(adapter.read_run(text.replace("run:", "other_branch_run:", 1), IMPL_RUN)[0],
                         self.read("impl-test")[0])

    def test_format_drift_is_reported_never_guessed(self):
        text = capture("impl-review")
        cases = {
            "indentation": text.replace("  status: running", "   status: running"),
            "a tab": text.replace("  status: running", "\tstatus: running"),
            "unknown run status": text.replace("status: running", "status: warming"),
            "unknown step status": text.replace("review,running,0,0", "review,thinking,0,0"),
            "row count": text.replace("steps[9]", "steps[10]"),
            "cell count": text.replace("review,running,0,0", "review,running,0"),
            "no steps": text.replace("steps[9]{step,status,findings,duration_ms}", "stages[9]{a,b,c,d}"),
            "no status column": text.replace("{step,status,findings,duration_ms}",
                                             "{step,state,findings,duration_ms}"),
            "another run": text.replace(IMPL_RUN, TESTS_RUN, 1),
            "a key twice": text.replace("  branch: fm/impl", "  branch: fm/impl\n  branch: fm/other"),
            "unterminated quote": text.replace('"3s ago: claude producing output"', '"3s ago'),
            "a count": text.replace("review,running,0,0", "review,running,none,0"),
            "a duration": text.replace("12s,12s", "twelve,twelve"),
            "an unknown step": text.replace("    review,running,12s", "    reviews,running,12s"),
            "no run": "status: running\n",
            "an outcome": text + "outcome: maybe\n",
            "a gate on no step": text + "gate:\n  step: deploy\n",
            "a gate without actions": text + "gate:\n  step: review\n  findings[1]{id,severity}:\n    r1,info\n",
        }
        for name, case in cases.items():
            with self.subTest(name), self.assertRaises(adapter.Drift):
                adapter.read_run(case, IMPL_RUN)

    def test_a_run_id_is_a_ulid_and_names_its_creation_time(self):
        # The report's live sample: created at 03:36:40.355Z.
        self.assertEqual(adapter.run_epoch("01M365CGN3GNWT3WWN6C80XFVN"), 1790134600)
        self.assertEqual(adapter.run_epoch(IMPL_RUN), B + 465)
        for bad in (None, "", "01M365CGN3", "01M365CGN3GNWT3WWN6C80XFVI", "81M365CGN3GNWT3WWN6C80XFVN",
                    "01m365cgn3gnwt3wwn6c80xfvn", "../../etc/passwd"):
            self.assertIsNone(adapter.run_epoch(bad), bad)


class NoMistakesTests(unittest.TestCase):
    """The CLI wrapper, against a stand-in binary: never the real no-mistakes."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        self.log = root / "calls.jsonl"
        self.binary = root / "no-mistakes"
        self.binary.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys, time\n"
            f"log = {str(self.log)!r}\n"
            "with open(log, 'a') as out:\n"
            "    out.write(json.dumps({'argv': sys.argv[1:], 'cwd': os.getcwd(),\n"
            "        'update_check': os.environ.get('NO_MISTAKES_NO_UPDATE_CHECK')}) + '\\n')\n"
            "mode = os.environ.get('FAKE_NM', 'up')\n"
            "if mode == 'slow': time.sleep(5)\n"
            "if sys.argv[1:] == ['daemon', 'status']:\n"
            "    print('  \\u25cf daemon running (pid 1)' if mode == 'up' else '  daemon not running')\n"
            "    sys.exit(0 if mode == 'up' else 1)\n"
            "print('error: \"run \\\\\"' + sys.argv[-1] + '\\\\\" not found\"')\n"
            "sys.exit(1)\n")
        self.binary.chmod(0o755)
        self.nm = adapter.NoMistakes(str(self.binary), timeout=2)

    def tearDown(self):
        self.directory.cleanup()

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_only_the_two_reads_are_ever_run(self):
        for argv in (("axi", "respond", "--action", "approve"), ("axi", "abort", "--run", IMPL_RUN),
                     ("axi", "rerun"), ("axi", "sync"), ("attach",), ("daemon", "restart"),
                     ("daemon", "stop"), ("axi", "status"), ("axi", "status", "--run", IMPL_RUN, "--full")):
            with self.subTest(argv), self.assertRaises(ValueError):
                self.nm.call(*argv)
        self.assertFalse(self.log.exists(), "a refused command must never start")

    def test_reads_run_from_the_root_with_the_update_check_off(self):
        with mock.patch.dict(os.environ, {"FAKE_NM": "up"}):
            self.assertEqual(self.nm.daemon(), "up")
            with self.assertRaises(adapter.Unreadable):
                adapter.read_run(self.nm.status(IMPL_RUN), IMPL_RUN)
        for call in self.calls():
            self.assertEqual((call["cwd"], call["update_check"]), ("/", "1"))
        self.assertEqual([c["argv"] for c in self.calls()],
                         [["daemon", "status"], ["axi", "status", "--run", IMPL_RUN]])

    def test_the_daemon_probe_reads_as_firstmate_reads_it(self):
        with mock.patch.dict(os.environ, {"FAKE_NM": "down"}):
            self.assertEqual(self.nm.daemon(), "down")
        with mock.patch.dict(os.environ, {"FAKE_NM": "slow"}):
            self.assertEqual(self.nm.daemon(), "unanswered")


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.now = B
        self.captures = {IMPL_RUN: []}
        self.runs = {IMPL_RUN: ("impl", B, self.captures[IMPL_RUN])}
        self.down = set()
        self.reader = FakeNoMistakes(lambda: self.now, self.runs, lambda t: self.answer(t))
        self.validation = self.collector()

    def answer(self, t):
        return self.down.get(t, "up") if isinstance(self.down, dict) else t in self.down

    def collector(self, run="run-1"):
        return adapter.Validation(FLEET, run, max_gap=60, checkpoint=60, reader=self.reader,
                                  clock=lambda: self.now)

    def poll(self, t, validation=None, present=True, text=None):
        """One poll at `t`; `text` becomes what a read of the run sees from now."""
        self.now = t
        if text is not None:
            self.captures[IMPL_RUN].append((t, text))
        row = fm_task("impl", "g1", t)
        row["validation_run"] = attributed("impl", t, self.runs) if present else None
        return (validation or self.validation).observe(fm_snapshot(t, [row] if present else []))

    def seen(self, lines):
        return [(e["validation"]["phase"], e["at"], e["at_quality"], e["validation"].get("since"))
                for e in events(lines, "validation")]

    def test_a_run_is_journalled_as_it_changes(self):
        lines, notes = self.poll(B + 10, text="impl-review")
        self.assertEqual(notes, [])
        # Created after the adapter's clock says now: its start is only noticed.
        self.assertEqual(self.seen(lines), [("started", adapter.iso(B + 10), "observed", None),
                                            ("seen", adapter.iso(B + 10), "observed", None)])
        review = events(lines, "validation")[1]["validation"]
        self.assertEqual(review["steps"][2]["round_since"], adapter.iso(B - 2))
        self.assertEqual(review["run"], IMPL_RUN)
        # The same read again: only coverage, never a repeat.
        lines, _ = self.poll(B + 20)
        self.assertEqual(self.seen(lines), [])
        # A change: seen, bounded by the read before it.
        lines, _ = self.poll(B + 30, text="impl-test")
        self.assertEqual(self.seen(lines), [("seen", adapter.iso(B + 30), "observed", adapter.iso(B + 20))])
        lines, _ = self.poll(B + 40, text="impl-passed")
        self.assertEqual(self.seen(lines), [("ended", adapter.iso(B + 40), "observed", adapter.iso(B + 30))])
        # Ended: never read again, even while Firstmate still names it.
        calls = len(self.reader.calls)
        lines, _ = self.poll(B + 50)
        self.assertEqual((lines, len(self.reader.calls)), ([], calls))

    def test_a_run_started_before_it_was_seen_keeps_its_derived_start(self):
        lines, _ = self.poll(B + 600, text="impl-review")
        self.assertEqual(self.seen(lines)[0], ("started", adapter.iso(B + 465), "derived", None))

    def test_a_gate_parks_once_and_its_ask_user_findings_are_counted(self):
        runs = {TESTS_RUN: ("impl", B, [])}
        self.runs.clear(); self.runs.update(runs); self.captures = {IMPL_RUN: runs[TESTS_RUN][2]}
        self.poll(B + 900, text=capture("tests-review"))
        lines, _ = self.poll(B + 910, text=capture("tests-parked"))
        parked = events(lines, "validation")
        self.assertEqual([e["validation"]["phase"] for e in parked], ["parked"])
        self.assertEqual(parked[0]["validation"]["gate"]["ask_user"], 1)
        self.assertEqual(self.seen(self.poll(B + 920)[0]), [])

    def test_a_dead_daemon_makes_the_run_unverified_until_it_reads_again(self):
        self.poll(B + 600, text="impl-review")
        self.down = {B + 610, B + 620}
        lines, notes = self.poll(B + 610)
        self.assertEqual(self.seen(lines), [("daemon_down", adapter.iso(B + 610), "observed", None)])
        self.assertEqual(notes, ["no-mistakes daemon is down: 1 validation run(s) unverified"])
        self.assertEqual(self.reader.calls[-1], ("axi", "status", "--run", IMPL_RUN),
                         "read, but a record still running is not taken")
        lines, _ = self.poll(B + 620)
        self.assertEqual(self.seen(lines), [], "said once")
        # Back up, nothing changed: a sighting all the same, to end the doubt.
        lines, _ = self.poll(B + 630)
        self.assertEqual(self.seen(lines), [("seen", adapter.iso(B + 630), "observed", adapter.iso(B + 600))])
        # Unanswered proves nothing either way: no event, no coverage.
        self.down = {B + 640: "unanswered"}
        lines, notes = self.poll(B + 640)
        self.assertEqual((events(lines, "validation"), notes),
                         ([], ["no-mistakes daemon did not answer; validation unverified"]))

    def test_a_run_that_ended_is_taken_while_the_daemon_is_down(self):
        self.poll(B + 600, text="impl-review")
        self.down = {B + 610, B + 620, B + 630}
        self.poll(B + 610)
        lines, notes = self.poll(B + 620, text="impl-passed")
        self.assertEqual(self.seen(lines), [("ended", adapter.iso(B + 620), "observed", adapter.iso(B + 600))])
        self.assertEqual(notes, [])
        calls = len(self.reader.calls)
        self.assertEqual((self.poll(B + 630), len(self.reader.calls)), (([], []), calls))
        # Down from the start: no daemon_down for a run whose end is read.
        runs, self.captures = {TESTS_RUN: ("impl", B, [])}, {IMPL_RUN: []}
        self.runs.clear(); self.runs.update(runs); self.captures[IMPL_RUN] = runs[TESTS_RUN][2]
        self.validation, self.down = self.collector(), {B + 700}
        lines, notes = self.poll(B + 700, text=capture("impl-passed").replace(IMPL_RUN, TESTS_RUN))
        self.assertEqual([p for p, *_ in self.seen(lines)], ["started", "ended"])

    def test_drift_and_failed_reads_write_nothing_and_break_coverage(self):
        self.poll(B + 600, text="impl-review")
        lines, notes = self.poll(B + 610, text="run:\n  id: \"" + IMPL_RUN + "\"\n  status: warming\n")
        self.assertEqual(events(lines, "validation"), [])
        self.assertEqual(len(notes), 1)
        self.assertIn("output not understood (run status 'warming')", notes[0])
        lines, _ = self.poll(B + 620, text="impl-review")
        self.assertEqual(self.seen(lines), [], "unchanged since the last understood read")
        windows = [(e["validation_coverage"]["from"], e["validation_coverage"]["to"])
                   for e in events(lines, "validation_coverage")]
        self.assertEqual(windows, [(adapter.iso(B + 620), adapter.iso(B + 620))], "a new window")

    def test_a_run_record_that_is_gone_ends_the_reading(self):
        self.poll(B + 600, text="impl-review")
        lines, notes = self.poll(B + 610, text=capture("not-found"))
        self.assertEqual(self.seen(lines), [("gone", adapter.iso(B + 610), "observed", None)])
        self.assertIn("is gone from no-mistakes", notes[0])
        calls = len(self.reader.calls)
        self.assertEqual(self.poll(B + 620)[0], [])
        self.assertEqual(len(self.reader.calls), calls)

    def test_coverage_chains_checkpoints_and_splits_on_pauses(self):
        spans = []
        for t in [B + 600, B + 630, B + 660, B + 690, B + 720, B + 900, B + 930]:
            lines, _ = self.poll(t, text="impl-review" if t == B + 600 else None)
            spans += [(e["validation_coverage"]["from"], e["validation_coverage"]["to"],
                       e["validation_coverage"]["max_gap"], e["validation_coverage"]["run"])
                      for e in events(lines, "validation_coverage")]
        spans += [(e["validation_coverage"]["from"], e["validation_coverage"]["to"], 60, IMPL_RUN)
                  for e in events(self.validation.close(), "validation_coverage")]
        i = adapter.iso
        self.assertEqual(spans, [(i(B + 600), i(B + 600), 60, IMPL_RUN), (i(B + 600), i(B + 660), 60, IMPL_RUN),
                                 (i(B + 660), i(B + 720), 60, IMPL_RUN), (i(B + 900), i(B + 900), 60, IMPL_RUN),
                                 (i(B + 900), i(B + 930), 60, IMPL_RUN)])

    def test_a_task_that_leaves_keeps_its_run_read_until_it_ends(self):
        self.poll(B + 600, text="impl-review")
        lines, _ = self.poll(B + 610, present=False, text="impl-test")
        self.assertEqual([e["attempt"] for e in events(lines, "validation")],
                         [{"task": "impl", "spawn_gen": "g1"}])

    def test_restart_resumes_without_re_emitting(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "fleet.json"
            lines, _ = self.poll(B + 600, text="impl-review")
            adapter.append_journal(output, lines + self.validation.close())
            resumed = self.collector("run-2")
            resumed.recover(adapter.journal_records(output))
            # Unchanged: a new coverage window, nothing else.
            lines, _ = self.poll(B + 900, resumed, present=False)
            self.assertEqual([e["type"] for e in events(lines)], ["validation_coverage"])
            # A change across the downtime is bounded by the read before it.
            lines, _ = self.poll(B + 930, resumed, present=False, text="impl-passed")
            self.assertEqual(self.seen(lines), [("ended", adapter.iso(B + 930), "observed", adapter.iso(B + 900))])
            adapter.append_journal(output, lines)
            ended = self.collector("run-3")
            ended.recover(adapter.journal_records(output))
            calls = len(self.reader.calls)
            self.assertEqual(self.poll(B + 960, ended)[0], [])
            self.assertEqual(len(self.reader.calls), calls, "an ended run is not read again")

    def test_attributions_it_cannot_read_are_diagnosed(self):
        row = fm_task("impl", "g1", B)
        row["validation_run"] = {"id": "not-a-run"}
        remote = fm_task("far", "g2", B)
        remote.update(remote=True, validation_run={"id": IMPL_RUN})
        lines, notes = self.validation.observe(fm_snapshot(B, [row, remote]))
        self.assertEqual(lines, [])
        self.assertEqual(notes, ["impl: validation run 'not-a-run' is not a no-mistakes run ID; not read"])
        self.assertEqual(self.reader.calls, [])

    def test_validation_diagnostics_join_the_manifest(self):
        self.captures[IMPL_RUN].append((B, "impl-review"))
        self.down = {B}
        row = fm_task("impl", "g1", B)
        row["validation_run"] = {"id": IMPL_RUN}
        snap = fm_snapshot(B, [row])
        manifest = adapter.build_manifest(snap, snap, {}, observed=adapter.iso(B))
        lines = adapter.lifecycle(snap, manifest, adapter.Bridge(FLEET, "run-1"),
                                  adapter.Feeds(Path(tempfile.mkdtemp()) / "f.json"), self.validation)
        self.assertIn("no-mistakes daemon is down: 1 validation run(s) unverified", manifest["diagnostics"])
        self.assertTrue(events(lines, "validation"))

    def test_every_line_has_the_viewer_schema(self):
        for text in validation_journal():
            record = json.loads(text)
            self.assertEqual((record["schema"], record["kind"]), (adapter.JOURNAL, "lifecycle"))
            event = record["event"]
            self.assertEqual(event["source"]["kind"], "no_mistakes")
            self.assertIn(event["type"], ("validation", "validation_coverage"))
            self.assertIn(event["type"], event)
            self.assertIn(event["at_quality"], ("derived", "observed"))
            self.assertTrue(event["attempt"]["task"] and event["attempt"]["spawn_gen"])

    def test_validation_fixture_is_adapter_output(self):
        # Regenerate with ZOE_REGENERATE_CREW=1 after changing the scenario.
        expected = "".join(line + "\n" for line in validation_journal())
        path = NM_FIXTURE / "fleet.events.jsonl"
        if os.environ.get("ZOE_REGENERATE_CREW") == "1":
            path.write_text(expected)
        self.assertEqual(path.read_text(), expected)

    def test_removing_a_worker_takes_its_validation_records(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "fleet.json"
            lines = [json.loads(text) for text in validation_journal()]
            adapter.append_journal(output, lines)
            gen = CREW["impl"]["gen"]
            worker = adapter.Worker(output, attempt=("impl", gen))
            self.assertIn("deleted", worker.remove("delete", True))
            left = {e["attempt"]["task"] for e in (r["event"] for r in adapter.journal_records(output))}
            self.assertEqual(left, {"tests"})


if __name__ == "__main__":
    unittest.main()
