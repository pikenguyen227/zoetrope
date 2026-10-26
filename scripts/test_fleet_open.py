import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest
import hashlib

OPEN = Path(__file__).resolve().parent.parent / "fleet-plugin" / "open.sh"
# Herdr runs `bash open.sh`; on macOS that is the system bash 3.2, where an
# empty array under `set -u` is an unbound variable.
BASH = "/bin/bash"
CAPTAIN = OPEN.parent / "captain.py"
CONFIG_HELPER = OPEN.parent.parent / "scripts" / "fleet-config.sh"
LAUNCHER = OPEN.parent.parent / "bin" / "zoe-fleet-tower"
INSTALLER = OPEN.parent.parent / "scripts" / "install-fleet.sh"
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
    def launch(self, context, path=None, installed=False, panes=(), endpoints=(), snapshot=True,
               config=None, overrides=None, missing_home=False):
        """Run a copy of open.sh against a stub adapter; return its argv and
        the no-mistakes it was handed. `path`: the pane's PATH; `installed`:
        a no-mistakes in a legacy .tools/bin. `panes`: what a
        stub Herdr lists, each a pane record whose "cwd" of HOME names the
        Firstmate home; `endpoints`: the pane IDs Firstmate's tasks run in;
        `snapshot`: False when Firstmate cannot be read."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            (root / "fleet-plugin").mkdir(parents=True)
            (root / "scripts").mkdir()
            shutil.copy(OPEN, root / "fleet-plugin" / "open.sh")
            shutil.copy(CAPTAIN, root / "fleet-plugin" / "captain.py")
            shutil.copy(CONFIG_HELPER, root / "scripts" / "fleet-config.sh")
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
            config_home = Path(tmp) / "user"
            config_dir = config_home / ".config" / "zoe-fleet"
            substitutions = {"@HOME@": str(home), "@BIN@": str(binary),
                             "@STATE@": str(Path(tmp) / "state"),
                             "@TELEMETRY@": str(Path(tmp) / "telemetry"),
                             "@NO_MISTAKES@": str(tools / "no-mistakes")}
            def expand(value):
                for marker, replacement in substitutions.items():
                    value = value.replace(marker, replacement)
                return value
            if config is not None:
                config_dir.mkdir(parents=True)
                (config_dir / "config").write_text("".join(f"{k}={expand(v)}\n" for k, v in config.items()))
            env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "ZOE_", "FM_", "XDG_"))}
            env.update(HOME=str(config_home), XDG_CONFIG_HOME=str(config_home / ".config"),
                XDG_STATE_HOME=str(config_home / ".local/state"), HERDR_ENV="1", HERDR_BIN_PATH=str(herdr),
                HERDR_PLUGIN_CONTEXT_JSON=json.dumps(context))
            if not missing_home:
                env.update(ZOE_FLEET_BIN=str(binary), ZOE_FLEET_STATE_DIR=str(Path(tmp) / "state"),
                    ZOE_TELEMETRY_DIR=str(Path(tmp) / "telemetry"), FM_HOME=str(home))
            if overrides:
                env.update({k: expand(v) for k, v in overrides.items()})
            if path is not None:
                env["PATH"] = path
            run = subprocess.run([BASH, str(root / "fleet-plugin" / "open.sh")], env=env,
                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            if missing_home and not (config or {}).get("FM_HOME") and not (overrides or {}).get("FM_HOME"):
                self.assertNotEqual(run.returncode, 0)
                return run.stderr
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertTrue(argv.exists(), f"adapter never started: {run.stderr}")
            args, no_mistakes = json.loads(argv.read_text())
            if no_mistakes:
                no_mistakes = os.path.relpath(os.path.realpath(no_mistakes), os.path.realpath(tmp))
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

    def test_no_mistakes_requires_path_or_an_explicit_setting(self):
        pane = "/usr/bin:/bin:/usr/sbin:/sbin"
        self.assertIsNone(self.launch({}, path=pane, installed=True)[1])
        self.assertIsNone(self.launch({}, path=pane)[1])

    def test_config_supplies_home_binary_and_state(self):
        config = {"FM_HOME": "@HOME@", "ZOE_FLEET_BIN": "@BIN@",
                  "ZOE_FLEET_STATE_DIR": "@STATE@", "ZOE_TELEMETRY_DIR": "@TELEMETRY@",
                  "ZOE_NO_MISTAKES_BIN": "@NO_MISTAKES@"}
        argv, no_mistakes = self.launch({}, path="/usr/bin:/bin:/usr/sbin:/sbin",
                                        installed=True, missing_home=True, config=config)
        self.assertTrue(argv[argv.index("--home") + 1].endswith("/firstmate"))
        self.assertTrue(argv[argv.index("--output") + 1].endswith("/state/fleet.json"))
        self.assertTrue(argv[argv.index("--view") + 1].endswith("/zoe-fleet"))
        self.assertEqual(no_mistakes, os.path.join(".tools", "bin", "no-mistakes"))

    def test_environment_values_win_over_config(self):
        config = {"FM_HOME": "/missing/home", "ZOE_FLEET_BIN": "/missing/binary",
                  "ZOE_FLEET_STATE_DIR": "/missing/state"}
        argv, _ = self.launch({}, config=config)
        self.assertTrue(argv[argv.index("--home") + 1].endswith("/firstmate"))
        self.assertTrue(argv[argv.index("--output") + 1].endswith("/state/fleet.json"))

    def test_config_failure_waits_before_closing_the_pane(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            (root / "fleet-plugin").mkdir(parents=True)
            (root / "scripts").mkdir()
            shutil.copy(OPEN, root / "fleet-plugin" / "open.sh")
            shutil.copy(CONFIG_HELPER, root / "scripts" / "fleet-config.sh")
            env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "ZOE_", "FM_", "XDG_"))}
            env.update(HOME=str(Path(tmp) / "user"), XDG_CONFIG_HOME=str(Path(tmp) / "user/config"),
                       HERDR_ENV="1")
            pane = subprocess.Popen([BASH, str(root / "fleet-plugin" / "open.sh")], env=env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True)
            try:
                with self.assertRaises(subprocess.TimeoutExpired):
                    pane.wait(timeout=1)
                _, error = pane.communicate("\n", timeout=10)
            finally:
                if pane.poll() is None:
                    pane.kill()
                    pane.communicate()
            self.assertEqual(pane.returncode, 1)
            self.assertIn("FM_HOME is unset", error)

    def test_missing_home_refuses_without_owner_layout_default(self):
        error = self.launch({}, missing_home=True, installed=True)
        self.assertIn("FM_HOME is unset", error)
        self.assertNotIn("Agentic", error)
        self.assertNotIn(".tools", error)

    def test_default_state_is_scoped_to_the_resolved_home(self):
        argv, _ = self.launch({}, missing_home=True,
                              config={"FM_HOME": "@HOME@", "ZOE_FLEET_BIN": "@BIN@"})
        home = argv[argv.index("--home") + 1]
        digest = hashlib.sha256(home.encode()).hexdigest()[:12]
        self.assertTrue(argv[argv.index("--output") + 1].endswith(
            f"/zoe-fleet/{digest}/fleet.json"))
        self.assertNotIn(".tools", " ".join(argv))
        self.assertNotIn("Agentic", " ".join(argv))


@unittest.skipUnless(os.access(BASH, os.X_OK), "needs /bin/bash")
class LauncherTests(unittest.TestCase):
    def test_setup_writes_private_config_and_inspect_uses_its_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            runtime = root / "runtime"
            (runtime / "bin").mkdir(parents=True)
            (runtime / "scripts").mkdir()
            shutil.copy(LAUNCHER, runtime / "bin" / "zoe-fleet-tower")
            shutil.copy(CONFIG_HELPER, runtime / "scripts" / "fleet-config.sh")
            (runtime / "scripts" / "fleet-demo.py").write_text(
                "import sys\nprint(' '.join(sys.argv[1:]))\n")
            home = root / "firstmate"
            (home / "bin").mkdir(parents=True)
            snapshot = home / "bin" / "fm-fleet-snapshot.sh"
            snapshot.write_text("#!/bin/sh\n")
            snapshot.chmod(0o755)
            binary = runtime / "bin" / "zoe-fleet"
            binary.write_text("#!/bin/sh\nprintf '%s\\n' \"$*\"\n")
            binary.chmod(0o755)
            linked = root / "tools" / "zoe-fleet-tower"
            linked.parent.mkdir()
            linked.symlink_to(runtime / "bin" / "zoe-fleet-tower")
            herdr = root / "stub-herdr"
            herdr.write_text("#!/bin/sh\nprintf 'pikenguyen227.zoetrope-fleet\\n'\n")
            herdr.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if not k.startswith(("FM_", "ZOE_", "XDG_", "HERDR_"))}
            env.update(HOME=str(root / "user"), XDG_CONFIG_HOME=str(root / "user/config"),
                       XDG_STATE_HOME=str(root / "user/state"), HERDR_BIN_PATH=str(herdr))
            setup = subprocess.run([BASH, str(linked), "setup", "--home", str(home)],
                                   env=env, capture_output=True, text=True)
            self.assertEqual(setup.returncode, 0, setup.stderr)
            config = root / "user/config/zoe-fleet/config"
            self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
            values = dict(line.split("=", 1) for line in config.read_text().splitlines())
            self.assertEqual(values["FM_HOME"], str(home.resolve()))
            self.assertNotIn("ZOE_FLEET_BIN", values)
            self.assertNotIn("ZOE_FLEET_RUNTIME", values)
            self.assertNotIn("ZOE_FLEET_STATE_DIR", values)
            self.assertNotIn("ZOE_TELEMETRY_DIR", values)
            digest = hashlib.sha256(str(home.resolve()).encode()).hexdigest()[:12]
            state = root / "user/state/zoe-fleet" / digest
            for directory in (state, state / "telemetry"):
                self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
            doctor = subprocess.run([BASH, str(linked), "doctor"], env=env,
                                    capture_output=True, text=True)
            self.assertEqual(doctor.returncode, 0, doctor.stderr)
            self.assertIn("Herdr plugin: linked", doctor.stdout)
            self.assertIn("State writable:", doctor.stdout)
            inspect = subprocess.run([BASH, str(linked), "inspect"], env=env,
                                     capture_output=True, text=True)
            self.assertEqual(inspect.returncode, 0, inspect.stderr)
            self.assertEqual(inspect.stdout.strip(), f"fleet {state}/fleet.json --inspect")
            other = root / "other-home"
            other.mkdir()
            moved = subprocess.run([BASH, str(linked), "inspect"], env={**env, "FM_HOME": str(other)},
                                   capture_output=True, text=True)
            self.assertEqual(moved.returncode, 0, moved.stderr)
            other_digest = hashlib.sha256(str(other.resolve()).encode()).hexdigest()[:12]
            self.assertEqual(moved.stdout.strip(),
                             f"fleet {root / 'user/state/zoe-fleet' / other_digest}/fleet.json --inspect")
            demo = subprocess.run([BASH, str(linked), "demo"], env=env,
                                  capture_output=True, text=True)
            self.assertEqual(demo.returncode, 0, demo.stderr)
            self.assertEqual(demo.stdout.strip(), f"--play {binary.resolve()}")
            upgraded = root / "upgraded"
            shutil.copytree(runtime, upgraded)
            (upgraded / "scripts" / "fleet-demo.py").write_text(
                "import sys\nprint('upgraded', ' '.join(sys.argv[1:]))\n")
            newer = subprocess.run([BASH, str(upgraded / "bin" / "zoe-fleet-tower"), "demo"], env=env,
                                   capture_output=True, text=True)
            self.assertEqual(newer.returncode, 0, newer.stderr)
            self.assertEqual(newer.stdout.strip(),
                             f"upgraded --play {(upgraded / 'bin' / 'zoe-fleet').resolve()}")
            with config.open("a") as handle:
                handle.write(f"ZOE_FLEET_RUNTIME={runtime}\n")
            pinned = subprocess.run([BASH, str(upgraded / "bin" / "zoe-fleet-tower"), "demo"], env=env,
                                    capture_output=True, text=True)
            self.assertEqual(pinned.returncode, 0, pinned.stderr)
            self.assertEqual(pinned.stdout.strip(), f"--play {runtime}/bin/zoe-fleet")
            shutil.rmtree(home)
            stale = subprocess.run([BASH, str(linked), "demo"], env=env,
                                   capture_output=True, text=True)
            self.assertEqual(stale.returncode, 0, stale.stderr)
            self.assertEqual(stale.stdout.strip(), f"--play {runtime}/bin/zoe-fleet")

    def test_explicit_state_stays_with_the_configured_home(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home = root / "firstmate"
            (home / "bin").mkdir(parents=True)
            snapshot = home / "bin" / "fm-fleet-snapshot.sh"
            snapshot.write_text("#!/bin/sh\n")
            snapshot.chmod(0o755)
            other = root / "other"
            other.mkdir()
            binary = root / "zoe-fleet"
            binary.write_text("#!/bin/sh\nprintf '%s\\n' \"$*\"\n")
            binary.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if not k.startswith(("FM_", "ZOE_", "XDG_", "HERDR_"))}
            env.update(HOME=str(root / "user"), XDG_CONFIG_HOME=str(root / "user/config"),
                       XDG_STATE_HOME=str(root / "user/state"))
            setup = subprocess.run([BASH, str(LAUNCHER), "setup", "--home", str(home), "--bin", str(binary),
                                    "--state", str(root / "legacy")], env=env, capture_output=True, text=True)
            self.assertEqual(setup.returncode, 0, setup.stderr)
            same = subprocess.run([BASH, str(LAUNCHER), "inspect"], env=env, capture_output=True, text=True)
            self.assertEqual(same.stdout.strip(), f"fleet {(root / 'legacy').resolve()}/fleet.json --inspect")
            moved = subprocess.run([BASH, str(LAUNCHER), "inspect"], env={**env, "FM_HOME": str(other)},
                                   capture_output=True, text=True)
            self.assertEqual(moved.returncode, 0, moved.stderr)
            digest = hashlib.sha256(str(other.resolve()).encode()).hexdigest()[:12]
            self.assertEqual(moved.stdout.strip(),
                             f"fleet {root / 'user/state/zoe-fleet' / digest}/fleet.json --inspect")

    def test_setup_rejects_a_home_without_snapshot(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home = root / "home"; home.mkdir()
            env = {**os.environ, "HOME": str(root / "user"),
                   "XDG_CONFIG_HOME": str(root / "user/config")}
            run = subprocess.run([BASH, str(LAUNCHER), "setup", "--home", str(home)],
                                 env=env, capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn("fm-fleet-snapshot.sh", run.stderr)
            self.assertFalse((root / "user/config/zoe-fleet/config").exists())


@unittest.skipUnless(os.access(BASH, os.X_OK), "needs /bin/bash")
class InstallerTests(unittest.TestCase):
    def test_existing_developer_layout_keeps_home_and_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            repo = parent / "zoetrope"
            for directory in ("bin", "scripts"):
                (repo / directory).mkdir(parents=True)
            shutil.copy(LAUNCHER, repo / "bin" / "zoe-fleet-tower")
            shutil.copy(CONFIG_HELPER, repo / "scripts" / "fleet-config.sh")
            shutil.copy(INSTALLER, repo / "scripts" / "install-fleet.sh")
            home = parent / "firstmate/bin"
            home.mkdir(parents=True)
            snapshot = home / "fm-fleet-snapshot.sh"
            snapshot.write_text("#!/bin/sh\n")
            snapshot.chmod(0o755)
            destination = parent / ".tools/bin"
            destination.mkdir(parents=True)
            fake_bin = parent / "fake-bin"
            fake_bin.mkdir()
            cargo = fake_bin / "cargo"
            cargo.write_text("#!/bin/sh\nmkdir -p target/release\nprintf '#!/bin/sh\\n' > target/release/zoe\nchmod 755 target/release/zoe\n")
            cargo.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if not k.startswith(("FM_", "ZOE_", "XDG_"))}
            env.update(HOME=str(parent / "user"), XDG_CONFIG_HOME=str(parent / "user/config"),
                       PATH=f"{fake_bin}:{os.environ['PATH']}")
            run = subprocess.run([BASH, str(repo / "scripts/install-fleet.sh")], env=env,
                                 capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            config = parent / "user/config/zoe-fleet/config"
            values = dict(line.split("=", 1) for line in config.read_text().splitlines())
            self.assertEqual(Path(values["FM_HOME"]).resolve(), (parent / "firstmate").resolve())
            self.assertEqual(Path(values["ZOE_FLEET_STATE_DIR"]).resolve(),
                             (parent / ".tools/state/zoe-fleet").resolve())
            self.assertEqual(Path(values["ZOE_TELEMETRY_DIR"]).resolve(),
                             (parent / ".tools/state/zoe-telemetry").resolve())
            self.assertTrue((destination / "zoe-fleet-tower").is_symlink())
            before = config.read_bytes()
            second = subprocess.run([BASH, str(repo / "scripts/install-fleet.sh")], env=env,
                                    capture_output=True, text=True)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertEqual(config.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
