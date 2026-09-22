import copy
import importlib.util
import json
import os
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
        cases = (([unreachable], True), ([timed_out], True), ([unreachable, timed_out], True),
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


if __name__ == "__main__":
    unittest.main()
