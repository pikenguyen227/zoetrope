#!/usr/bin/env python3
"""Per-user Claude launcher and minimal, local status-line quota bridge.

No requests, credentials, prompt content, or transcript bodies are persisted.
The viewer reads these snapshots; only Claude's status-line process writes them.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

def default_store():
    """Use the configured Firstmate home's private Fleet state root."""
    if 'ZOE_TELEMETRY_DIR' in os.environ:
        return Path(os.environ['ZOE_TELEMETRY_DIR'])
    values = {}
    config = Path(os.environ.get('XDG_CONFIG_HOME', str(Path.home() / '.config'))) / 'zoe-fleet/config'
    try:
        for line in config.read_text().splitlines():
            if '=' in line and not line.startswith('#'):
                key, value = line.split('=', 1)
                if key in ('FM_HOME', 'ZOE_FLEET_STATE_DIR', 'ZOE_TELEMETRY_DIR'):
                    values[key] = value
    except FileNotFoundError:
        pass
    home = os.environ.get('FM_HOME', values.get('FM_HOME', ''))
    if not home:
        return None
    home = str(Path(home).resolve())
    if not values.get('FM_HOME') or str(Path(values['FM_HOME']).resolve()) != home:
        values = {}
    digest = hashlib.sha256(home.encode()).hexdigest()[:12]
    state = os.environ.get('ZOE_FLEET_STATE_DIR', values.get('ZOE_FLEET_STATE_DIR',
        str(Path(os.environ.get('XDG_STATE_HOME', str(Path.home() / '.local/state'))) / 'zoe-fleet' / digest)))
    return Path(os.environ.get('ZOE_TELEMETRY_DIR', values.get('ZOE_TELEMETRY_DIR',
        str(Path(state) / 'telemetry'))))


def snapshot(data):
    session = data.get('session_id', '')
    if not isinstance(session, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,160}', session):
        return None
    windows = []
    rates = data.get('rate_limits') or {}
    for name, minutes in [('five_hour', 300), ('seven_day', 10080)]:
        w = rates.get(name) or {}
        used = w.get('used_percentage')
        reset = w.get('resets_at')
        if isinstance(used, (float, int)) and not isinstance(used, bool) and math.isfinite(used) and 0 <= used <= 100:
            windows.append({'minutes': minutes, 'used_percent': used,
                            'resets_at': reset if isinstance(reset, int) and not isinstance(reset, bool) else None})
    return {'schema': 'zoe.usage/v1', 'session_id': session,
            'quota': {'bucket': 'claude', 'observed_at': datetime.now(timezone.utc).isoformat(), 'windows': windows}}


def statusline():
    raw = sys.stdin.read()
    try:
        data = json.loads(raw)
        record = snapshot(data)
        if record:
            store = default_store()
            if store:
                previous = os.umask(0o077)
                try:
                    store.mkdir(parents=True, exist_ok=True)
                finally:
                    os.umask(previous)
                fd, name = tempfile.mkstemp(prefix='.usage-', dir=store)
                try:
                    with os.fdopen(fd, 'w') as output:
                        json.dump(record, output)
                    os.replace(name, store / f"claude-{record['session_id']}.json")
                finally:
                    if os.path.exists(name):
                        os.unlink(name)
    except (ValueError, TypeError, AttributeError, OSError):
        pass  # Optional telemetry must never break the user's status line.
    command = os.environ.get('ZOE_STATUSLINE_COMMAND')
    if command:
        try:
            subprocess.run(command, shell=True, input=raw, text=True, timeout=4, check=False)
        except (OSError, subprocess.TimeoutExpired):
            pass
    else:
        print('Zoe · local usage reporting')


def settings(value):
    try:
        result = json.loads(value if value.lstrip().startswith('{') else Path(value).read_text())
    except (OSError, ValueError) as error:
        raise ValueError('Cannot read --settings; leaving Claude arguments unchanged') from error
    if not isinstance(result, dict):
        raise ValueError('--settings must contain an object')
    return result


def launch_args(args, cwd, user_config):
    """Preserve existing settings and status-line output, including worker overlays."""
    config = {}
    for file in [user_config / 'settings.json', cwd / '.claude/settings.json', cwd / '.claude/settings.local.json']:
        if file.is_file():
            config.update(settings(str(file)))
    forwarded, overlay = [], {}
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == '--':
            forwarded.extend(args[i:]); break
        if arg == '--settings':
            i += 1
            if i >= len(args):
                raise ValueError('Missing --settings argument')
            overlay.update(settings(args[i]))
        elif arg.startswith('--settings='):
            overlay.update(settings(arg.split('=', 1)[1]))
        else:
            forwarded.append(arg)
        i += 1
    existing = overlay.get('statusLine', config.get('statusLine')) or {}
    command = existing.get('command', '') if isinstance(existing, dict) else ''
    bridge = f'{shlex.quote(sys.executable)} {shlex.quote(str(Path(__file__).resolve()))} --statusline'
    # Re-running an already wrapped launch must not recursively invoke the bridge.
    if str(Path(__file__).resolve()) in command:
        command = os.environ.get('ZOE_STATUSLINE_COMMAND', '')
    overlay['statusLine'] = {**(existing if isinstance(existing, dict) else {}), 'type': 'command', 'command': bridge}
    return ['--settings', json.dumps(overlay, separators=(',', ':')), *forwarded], command


def main():
    if sys.argv[1:2] == ['--statusline']:
        statusline(); return
    parser = argparse.ArgumentParser()
    parser.add_argument('--launch', required=True)
    opts, args = parser.parse_known_args()
    if args[:1] == ['--']:
        args = args[1:]
    try:
        forwarded, previous = launch_args(args, Path.cwd(), Path(os.environ.get('CLAUDE_CONFIG_DIR', Path.home() / '.claude')))
        os.environ['ZOE_STATUSLINE_COMMAND'] = previous
        store = default_store()
        if store:
            os.environ.setdefault('ZOE_TELEMETRY_DIR', str(store))
    except ValueError as error:
        print(f'Zoe telemetry skipped: {error}', file=sys.stderr)
        forwarded = args
    os.execv(opts.launch, [opts.launch, *forwarded])


if __name__ == '__main__':
    main()
