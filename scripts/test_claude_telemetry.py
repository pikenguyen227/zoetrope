import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('claude-telemetry.py')
spec = importlib.util.spec_from_file_location('telemetry', SCRIPT)
telemetry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(telemetry)


class TelemetryTests(unittest.TestCase):
    def test_only_quota_and_identity_are_persisted(self):
        data = {'session_id': 'test-123', 'transcript_path': '/private/log', 'token': 'secret',
                'context_window': {'total_input_tokens': 3000},
                'rate_limits': {'five_hour': {'used_percentage': 25.5, 'resets_at': 2000000000}}}
        with tempfile.TemporaryDirectory() as folder:
            env = {**os.environ, 'ZOE_TELEMETRY_DIR': folder, 'ZOE_STATUSLINE_COMMAND': 'printf existing-line'}
            run = subprocess.run([sys.executable, str(SCRIPT), '--statusline'], input=json.dumps(data),
                                 text=True, capture_output=True, env=env, check=True)
            saved = json.loads((Path(folder) / 'claude-test-123.json').read_text())
            self.assertEqual(run.stdout, 'existing-line')
            self.assertEqual(set(saved), {'schema', 'session_id', 'quota'})
            self.assertEqual(saved['quota']['windows'][0]['used_percent'], 25.5)
            self.assertNotIn('secret', json.dumps(saved))

    def test_missing_is_unknown_and_bad_identity_is_rejected(self):
        self.assertEqual(telemetry.snapshot({'session_id':'valid'})['quota']['windows'], [])
        self.assertIsNone(telemetry.snapshot({'session_id':'../bad'}))
        self.assertEqual(telemetry.snapshot({'session_id':'valid', 'rate_limits':{'five_hour':{'used_percentage':float('nan')}}})['quota']['windows'], [])

    def test_launch_preserves_worker_settings_and_existing_statusline(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            config = root / 'config'; config.mkdir()
            (config / 'settings.json').write_text(json.dumps({'statusLine': {'type':'command','command':'echo original','padding':2}}))
            args, previous = telemetry.launch_args(['--settings', '{"feedbackDrafts":"off","attribution":{"commit":""}}', '--permission-mode', 'auto', 'prompt'],root, config)
            self.assertEqual(previous, 'echo original')
            overlay = json.loads(args[1]); self.assertEqual(overlay['feedbackDrafts'], 'off')
            self.assertEqual(overlay['statusLine']['padding'], 2)
            self.assertEqual(overlay['attribution'], {'commit':''})
            self.assertIn('--statusline', overlay['statusLine']['command'])
            self.assertEqual(args[2:], ['--permission-mode','auto','prompt'])

    def test_explicit_statusline_and_terminator_are_respected(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            args, command=telemetry.launch_args(['--settings={"statusLine":{"command":"echo custom"}}','--','--settings'],root,root)
            self.assertEqual(command,'echo custom')
            self.assertEqual(args[-2:], ['--','--settings'])

    def test_malformed_overlay_fails_without_modifying_files(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            with self.assertRaises(ValueError): telemetry.launch_args(['--settings','{oops}'],root,root)
            self.assertEqual(list(root.iterdir()),[])

if __name__ == '__main__':
    unittest.main()
