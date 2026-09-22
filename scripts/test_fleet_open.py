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
    def launch(self, context):
        """Run a copy of open.sh against a stub adapter; return its argv."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            (root / "fleet-plugin").mkdir(parents=True)
            (root / "scripts").mkdir()
            shutil.copy(OPEN, root / "fleet-plugin" / "open.sh")
            argv = Path(tmp) / "argv.json"
            (root / "scripts" / "firstmate-fleet.py").write_text(
                f"import json, sys\njson.dump(sys.argv[1:], open({str(argv)!r}, 'w'))\n")
            binary = Path(tmp) / "zoe-fleet"
            binary.write_text("#!/bin/sh\n")
            binary.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "ZOE_", "FM_"))}
            env.update(HERDR_ENV="1", ZOE_FLEET_BIN=str(binary), ZOE_FLEET_STATE_DIR=str(Path(tmp) / "state"),
                ZOE_TELEMETRY_DIR=str(Path(tmp) / "telemetry"), FM_HOME=str(Path(tmp) / "firstmate"),
                HERDR_PLUGIN_CONTEXT_JSON=json.dumps(context))
            run = subprocess.run([BASH, str(root / "fleet-plugin" / "open.sh")], env=env,
                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertTrue(argv.exists(), f"adapter never started: {run.stderr}")
            return json.loads(argv.read_text())

    def test_without_focused_pane_starts_the_adapter(self):
        self.assertNotIn("--captain", self.launch({}))

    def test_focused_pane_names_the_captain(self):
        argv = self.launch({"focused_pane_id": "w1:p2"})
        self.assertEqual(argv[argv.index("--captain") + 1], "w1:p2")


if __name__ == "__main__":
    unittest.main()
