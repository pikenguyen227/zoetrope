import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

OPEN = Path(__file__).resolve().parent.parent / "fleet-plugin" / "open.sh"
# Herdr runs `bash open.sh`; on macOS that is the system bash 3.2, where an
# empty array under `set -u` is an unbound variable.
BASH = "/bin/bash"
CAPTAIN = OPEN.parent / "captain.py"
HOME = object()  # a stub pane's cwd: the Firstmate home


def agent(pane_id, cwd=HOME, provider="claude", focused=False):
    """A Herdr pane record running a registered agent session."""
    return {"pane_id": pane_id, "cwd": cwd, "focused": focused, "agent": provider,
            "agent_session": {"agent": provider, "kind": "id", "value": "s-" + pane_id}}


def shell(pane_id, focused=False):
    """A Herdr pane record running a plain shell (the captain's Shell tab)."""
    return {"pane_id": pane_id, "cwd": "/Users/captain", "focused": focused,
            "agent_status": "unknown"}


@unittest.skipUnless(os.access(BASH, os.X_OK), "needs /bin/bash")
class OpenTests(unittest.TestCase):
    def launch(self, context, path=None, installed=False, panes=(), endpoints=(), snapshot=True):
        """Run a copy of open.sh against a stub adapter; return its argv and
        the no-mistakes it was handed. `path`: the pane's PATH; `installed`:
        a no-mistakes in .tools/bin beside the Fleet binary. `panes`: what a
        stub Herdr lists, each a pane record whose "cwd" of HOME names the
        Firstmate home; `endpoints`: the pane IDs Firstmate's tasks run in;
        `snapshot`: False when Firstmate cannot be read."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            (root / "fleet-plugin").mkdir(parents=True)
            (root / "scripts").mkdir()
            shutil.copy(OPEN, root / "fleet-plugin" / "open.sh")
            shutil.copy(CAPTAIN, root / "fleet-plugin" / "captain.py")
            home = Path(tmp) / "firstmate"
            (home / "bin").mkdir(parents=True)
            listed = [dict(p, cwd=str(home) if p.get("cwd") == HOME else p.get("cwd")) for p in panes]
            herdr = Path(tmp) / "herdr"
            herdr.write_text("#!/bin/sh\ncat <<'JSON'\n"
                             + json.dumps({"result": {"panes": listed}}) + "\nJSON\n")
            herdr.chmod(0o755)
            tasks = [{"id": f"t{i}", "endpoint": {"target": "default:" + pane}}
                     for i, pane in enumerate(endpoints)]
            fm = home / "bin" / "fm-fleet-snapshot.sh"
            fm.write_text("#!/bin/sh\n" + ("cat <<'JSON'\n" + json.dumps(
                {"fm_home": str(home), "tasks": tasks}) + "\nJSON\n" if snapshot else "exit 3\n"))
            fm.chmod(0o755)
            argv = Path(tmp) / "argv.json"
            (root / "scripts" / "firstmate-fleet.py").write_text(
                "import json, os, sys\n"
                f"json.dump([sys.argv[1:], os.environ.get('ZOE_NO_MISTAKES_BIN')], open({str(argv)!r}, 'w'))\n")
            tools = Path(tmp) / ".tools" / "bin"
            if installed:
                tools.mkdir(parents=True)
                (tools / "no-mistakes").write_text("#!/bin/sh\n")
                (tools / "no-mistakes").chmod(0o755)
            binary = Path(tmp) / "zoe-fleet"
            binary.write_text("#!/bin/sh\n")
            binary.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "ZOE_", "FM_"))}
            env.update(HERDR_ENV="1", ZOE_FLEET_BIN=str(binary), ZOE_FLEET_STATE_DIR=str(Path(tmp) / "state"),
                ZOE_TELEMETRY_DIR=str(Path(tmp) / "telemetry"), FM_HOME=str(home), HERDR_BIN_PATH=str(herdr),
                HERDR_PLUGIN_CONTEXT_JSON=json.dumps(context))
            if path is not None:
                env["PATH"] = path
            run = subprocess.run([BASH, str(root / "fleet-plugin" / "open.sh")], env=env,
                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertTrue(argv.exists(), f"adapter never started: {run.stderr}")
            args, no_mistakes = json.loads(argv.read_text())
            if no_mistakes:
                no_mistakes = os.path.relpath(no_mistakes, os.path.realpath(tmp))
            return args, no_mistakes

    def test_without_focused_pane_starts_the_adapter(self):
        self.assertNotIn("--captain", self.launch({})[0])

    def captain(self, argv):
        return argv[argv.index("--captain") + 1] if "--captain" in argv else None

    def test_a_focused_plain_shell_is_never_the_captain(self):
        # Observed: the view bound to the captain's focused Shell tab wA:pZ.
        argv, _ = self.launch({"focused_pane_id": "wA:pZ"}, panes=[shell("wA:pZ", focused=True)])
        self.assertIsNone(self.captain(argv))

    def test_the_coordinator_is_found_while_another_pane_is_focused(self):
        panes = [shell("wA:pZ", focused=True), agent("wA:p5")]
        argv, _ = self.launch({"focused_pane_id": "wA:pZ"}, panes=panes)
        self.assertEqual(self.captain(argv), "wA:p5")
        argv, _ = self.launch({}, panes=[shell("wA:pZ"), agent("wA:p5", provider="codex")])
        self.assertEqual(self.captain(argv), "wA:p5")

    def test_a_focused_coordinator_is_the_captain(self):
        argv, _ = self.launch({"focused_pane_id": "wA:p5"}, panes=[agent("wA:p5", focused=True)])
        self.assertEqual(self.captain(argv), "wA:p5")
        # It settles a choice between two coordinators, but focus alone never does.
        both = [agent("wA:p5", focused=True), agent("wB:p1")]
        self.assertEqual(self.captain(self.launch({"focused_pane_id": "wA:p5"}, panes=both)[0]), "wA:p5")
        self.assertIsNone(self.captain(self.launch({"focused_pane_id": "wA:pZ"},
                                                   panes=both + [shell("wA:pZ", focused=True)])[0]))

    def test_crew_secondmate_and_other_home_panes_are_never_the_captain(self):
        # A crew pane starts in the Firstmate home too; a secondmate runs its
        # own home; another agent may run anywhere. Each is focused in turn.
        panes = [agent("w1:p2"), agent("w2:p2", cwd="/tmp/secondmate-home"),
                 agent("w3:p2", cwd="/Users/captain/project"), shell("wA:pZ")]
        for focused in ("w1:p2", "w2:p2", "w3:p2", "wA:pZ"):
            argv, _ = self.launch({"focused_pane_id": focused}, panes=panes,
                                  endpoints=("w1:p2", "w2:p2"))
            self.assertIsNone(self.captain(argv), focused)
        argv, _ = self.launch({"focused_pane_id": "w1:p2"}, panes=panes + [agent("wA:p5")],
                              endpoints=("w1:p2", "w2:p2"))
        self.assertEqual(self.captain(argv), "wA:p5")

    def test_no_coordinator_passes_no_captain(self):
        # Nothing runs the coordinator, or Firstmate cannot say which panes
        # are crew: the collector keeps its last-registered Captain.
        self.assertIsNone(self.captain(self.launch({"focused_pane_id": "wA:pZ"},
                                                   panes=[shell("wA:pZ", focused=True)])[0]))
        self.assertIsNone(self.captain(self.launch({}, panes=[agent("wA:p5")], snapshot=False)[0]))

    def test_a_herdr_pane_path_is_handed_the_installed_no_mistakes(self):
        # Herdr's own PATH: no-mistakes lives only beside the Fleet binary, and
        # without it Firstmate attributes no validation run (p had nothing).
        pane = "/usr/bin:/bin:/usr/sbin:/sbin"
        self.assertEqual(self.launch({}, path=pane, installed=True)[1],
                         os.path.join(".tools", "bin", "no-mistakes"))
        self.assertIsNone(self.launch({}, path=pane)[1])


if __name__ == "__main__":
    unittest.main()
