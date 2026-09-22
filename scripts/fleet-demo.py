#!/usr/bin/env python3
"""Generate synthetic Codex transcripts; optionally stream them into the Fleet UI.

No model, account, Herdr server or real transcript is accessed.
"""
import argparse
import datetime as dt
import json
from pathlib import Path
import subprocess
import tempfile
import time

IDS = ["11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222",
       "33333333-3333-4333-8333-333333333333", "44444444-4444-4444-8444-444444444444"]


def record(kind, payload, at):
    return json.dumps({"timestamp": at.isoformat().replace("+00:00", "Z"),
                       "type": kind, "payload": payload}) + "\n"


def seed(output, at):
    output.mkdir(parents=True, exist_ok=True)
    day = output / "sessions" / at.strftime("%Y/%m/%d")
    day.mkdir(parents=True, exist_ok=True)
    paths = []
    labels = ["Captain · Codex", "Worker · Implementation", "Worker · Tests"]
    for i, session_id in enumerate(IDS):
        path = day / f"rollout-{at:%Y-%m-%dT%H-%M-%S}-{session_id}.jsonl"
        payload = {"id": session_id, "session_id": session_id, "cwd": "/synthetic/fleet-demo",
                   "originator": "codex_exec", "cli_version": "0.155.1", "source": "exec"}
        if i == 3:
            payload.update({"session_id": IDS[1], "parent_thread_id": IDS[1],
                "thread_source": "subagent", "source": {"subagent": {"thread_spawn": {
                    "parent_thread_id": IDS[1], "depth": 1, "agent_path": "/root/research"}}}})
        text = record("session_meta", payload, at)
        text += record("turn_context", {"model": "gpt-6-astra"}, at)
        text += record("event_msg", {"type": "token_count", "info": {"total_token_usage": {
            "input_tokens": 10000*(i+1), "output_tokens": 800*(i+1), "cached_input_tokens":5000*(i+1)}},
            "rate_limits": {"limit_id":"codex", "primary":{"used_percent":32, "window_minutes":300,
            "resets_at":int(at.timestamp())+9000}, "secondary":{"used_percent":61, "window_minutes":10080,
            "resets_at":int(at.timestamp())+200000}}}, at)
        text += record("response_item", {"type": "function_call", "name": "shell",
            "call_id": "shared-call-id", "arguments": json.dumps({"command": "printf 'synthetic fixture'"})}, at + dt.timedelta(seconds=1))
        text += record("response_item", {"type": "function_call_output", "call_id": "shared-call-id",
            "output": "Process exited with code 0\nsynthetic fixture"}, at + dt.timedelta(seconds=2))
        path.write_text(text)
        paths.append(path)
    when = at.isoformat().replace("+00:00", "Z")
    key = lambda i: {"provider": "codex", "session_id": IDS[i]}
    manifest = {"schema": "zoetrope.fleet.v1", "fleet_id": "synthetic-demo", "label": "Fleet demo · SYNTHETIC",
        "observed_at": when,
        "sessions": [{"key": key(i), "label": labels[i], "file": str(paths[i].relative_to(output))} for i in range(3)],
        "tasks": [{"id": f"demo-worker-{i}", "spawn_gen": "attempt-1", "label": labels[i], "session": key(i),
                   "state": {"value": "working", "source": "synthetic fixture", "observed_at": when}} for i in (1, 2)],
        "links": [{"id": f"delegate-{i}", "from": key(0), "to": key(i), "kind": "delegates",
                   "evidence": "synthetic fixture assignment", "observed_at": when} for i in (1, 2)]}
    manifest_path = output / "fleet.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest_path, paths


def play(binary):
    with tempfile.TemporaryDirectory(prefix="zoetrope-fleet-demo-") as name:
        manifest, paths = seed(Path(name), dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=3))
        viewer = subprocess.Popen([str(binary), "fleet", str(manifest)])
        n = 0
        try:
            while viewer.poll() is None:
                i = n % len(paths)
                at = dt.datetime.now(dt.timezone.utc)
                with paths[i].open("a") as stream:
                    stream.write(record("response_item", {"type": "function_call", "name": "shell",
                        "call_id": f"demo-{n}", "arguments": json.dumps({"command": f"printf 'synthetic step {n}'"})}, at))
                time.sleep(1)
                with paths[i].open("a") as stream:
                    stream.write(record("response_item", {"type": "function_call_output", "call_id": f"demo-{n}",
                        "output": "Process exited with code 0\nsynthetic output"}, dt.datetime.now(dt.timezone.utc)))
                n += 1
        except KeyboardInterrupt:
            pass
        finally:
            if viewer.poll() is None:
                viewer.terminate()
                viewer.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--output", type=Path)
    mode.add_argument("--play", type=Path)
    args = parser.parse_args()
    if args.play:
        play(args.play)
    else:
        manifest, _ = seed(args.output, dt.datetime(2026, 9, 22, tzinfo=dt.timezone.utc))
        print(manifest)


if __name__ == "__main__":
    main()
