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


@unittest.skipUnless(os.access(BASH, os.X_OK), "needs /bin/bash")
class OpenTests(unittest.TestCase):
    def launch(self, context, path=None, installed=False):
        """Run a copy of open.sh against a stub adapter; return its argv and
        the no-mistakes it was handed. `path`: the pane's PATH; `installed`:
        a no-mistakes in .tools/bin beside the Fleet binary."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            (root / "fleet-plugin").mkdir(parents=True)
            (root / "scripts").mkdir()
            shutil.copy(OPEN, root / "fleet-plugin" / "open.sh")
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
                ZOE_TELEMETRY_DIR=str(Path(tmp) / "telemetry"), FM_HOME=str(Path(tmp) / "firstmate"),
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

    def test_focused_pane_names_the_captain(self):
        argv, _ = self.launch({"focused_pane_id": "w1:p2"})
        self.assertEqual(argv[argv.index("--captain") + 1], "w1:p2")

    def test_a_herdr_pane_path_is_handed_the_installed_no_mistakes(self):
        # Herdr's own PATH: no-mistakes lives only beside the Fleet binary, and
        # without it Firstmate attributes no validation run (p had nothing).
        pane = "/usr/bin:/bin:/usr/sbin:/sbin"
        self.assertEqual(self.launch({}, path=pane, installed=True)[1],
                         os.path.join(".tools", "bin", "no-mistakes"))
        self.assertIsNone(self.launch({}, path=pane)[1])


if __name__ == "__main__":
    unittest.main()
