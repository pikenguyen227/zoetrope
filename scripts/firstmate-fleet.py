#!/usr/bin/env python3
"""Read Firstmate/Herdr registrations and publish a durable Zoetrope manifest.

Live mode is only available inside Herdr. Offline fixtures call build_manifest;
they never use a socket, launch an agent, or impersonate a Herdr environment.
"""
import argparse
import copy
import datetime as dt
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

SCHEMA = "zoetrope.fleet.v1"


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def identity(key):
    return key["provider"], key["session_id"]


def registered(pane):
    session = pane.get("agent_session") or {}
    if (session.get("agent") in ("codex", "claude") and session.get("kind") == "id"
            and isinstance(session.get("value"), str) and session["value"].strip()):
        return {"provider": session["agent"], "session_id": session["value"]}
    return None


def observation(value, source, when):
    return {"value": str(value or "unknown"), "source": str(source or "firstmate.snapshot"),
            "observed_at": when}


def check_snapshot(snapshot):
    if snapshot.get("schema") != "fm-fleet-snapshot.v1":
        raise ValueError("expected fm-fleet-snapshot.v1")
    if not isinstance(snapshot.get("tasks"), list) or not snapshot.get("fm_home"):
        raise ValueError("snapshot is missing tasks/fm_home")


def unreachable(task):
    # fm-crew-state.sh folds a Herdr it cannot run into "backend unreachable".
    state = task.get("current_state") or {}
    return (state.get("source") == "none"
            and (state.get("detail") or "").startswith("backend unreachable"))


def build_manifest(before, after, panes, previous=None, captain=None, observed=None):
    """Join only stable generations/endpoints captured on both sides of pane reads.

    `panes` is keyed by Firstmate's exact session:pane target. `captain` is an
    explicitly chosen pane record, a current member rather than inferred ancestry.
    """
    check_snapshot(before)
    check_snapshot(after)
    home = after["fm_home"]
    if home != before["fm_home"]:
        raise ValueError("Firstmate home changed during observation")
    fleet_id = "firstmate:" + home
    when = observed or after.get("generated") or now()
    previous = copy.deepcopy(previous or {})
    if previous and (previous.get("schema") != SCHEMA or previous.get("fleet_id") != fleet_id):
        raise ValueError("existing manifest belongs to another fleet")
    sessions = {identity(s["key"]): s for s in previous.get("sessions", [])}
    attempts = {(t["id"], t["spawn_gen"]): t for t in previous.get("tasks", [])}
    diagnostics = []
    for task in attempts.values():
        task["runtime"] = observation("not observed", "firstmate.snapshot", when)
    if captain:
        key = registered(captain)
        if key:
            sessions[identity(key)] = {"key": key, "label": "Captain · " + key["provider"]}
        else:
            diagnostics.append("Captain pane has no supported registered session yet")
    earlier = {task["id"]: task for task in before["tasks"]}
    records = {r.get("id"): r for r in after.get("backlog", {}).get("records", [])}
    for current in after["tasks"]:
        task_id = current["id"]
        generation = current.get("spawn_gen")
        if not generation:
            diagnostics.append(f"{task_id}: launch generation unavailable; session join deferred")
        generation = generation or "unresolved"
        attempt = (task_id, generation)
        old = earlier.get(task_id, {})
        endpoint = current.get("endpoint") or {}
        target = endpoint.get("target")
        stable = (bool(current.get("spawn_gen"))
                  and old.get("spawn_gen") == current.get("spawn_gen")
                  and (old.get("endpoint") or {}).get("target") == target)
        state = current.get("current_state") or {}
        task = {"id": task_id, "spawn_gen": generation, "label": task_id,
                "project": current.get("project"),
                "session": (attempts.get(attempt) or {}).get("session"),
                "state": observation(state.get("state"), state.get("source"),
                                     state.get("observed_at") or when),
                "runtime": observation("unavailable", "herdr.pane.get", when),
                "depends_on": records.get(task_id, {}).get("blocked_by_ids", [])}
        if current.get("remote") or current.get("backend") != "herdr":
            diagnostics.append(f"{task_id}: only local Herdr endpoints are supported")
        elif not stable:
            diagnostics.append(f"{task_id}: generation/endpoint changed; join deferred")
        elif target:
            pane = panes.get(target, {})
            key = registered(pane)
            if key and key["provider"] == current.get("harness"):
                known = task["session"]
                if known and key != known:
                    # Same launch generation unexpectedly names another session.
                    # Preserve history; do not silently reassign task ownership.
                    diagnostics.append(f"{task_id}: session changed without a new spawn generation")
                else:
                    task["session"] = key
                    sessions.setdefault(identity(key), {"key": key, "label": task_id})
                    task["runtime"] = observation(pane.get("agent_status"), "herdr.pane.get", when)
            else:
                diagnostics.append(f"{task_id}: waiting for matching native session registration")
        if generation != "unresolved":
            attempts.pop((task_id, "unresolved"), None)
        attempts[attempt] = task
    local = [t for t in after["tasks"] if not t.get("remote") and t.get("backend") == "herdr"]
    if local and all(unreachable(t) for t in local):
        diagnostics.append("Firstmate reads every task as backend unreachable; "
                           "task state is unknown until herdr is on the snapshot's PATH")
    # Capture continuation only when two distinct native IDs are explicitly
    # registered under distinct launch generations of the SAME Firstmate task.
    links = {link["id"]: link for link in previous.get("links", [])}
    previous_attempts = previous.get("tasks", [])
    for current in after["tasks"]:
        attempt = attempts.get((current["id"], current.get("spawn_gen")))
        if not attempt or not attempt.get("session"):
            continue
        if any(t["id"] == attempt["id"] and t["spawn_gen"] == attempt["spawn_gen"]
               and t.get("session") for t in previous_attempts):
            continue
        prior = [t for t in previous_attempts if t["id"] == current["id"]
                 and t["spawn_gen"] != attempt["spawn_gen"] and t.get("session")
                 and t.get("project") == attempt.get("project")]
        if prior:
            last = max(prior, key=lambda t: t["state"]["observed_at"])
            if last["session"] != attempt["session"]:
                link_id = json.dumps(["attempt", current["id"], last["spawn_gen"], attempt["spawn_gen"]])
                links.setdefault(link_id, {"id": link_id, "from": last["session"],
                    "to": attempt["session"], "kind": "continues",
                    "evidence": f"Firstmate task {current['id']}: observed launch generation {last['spawn_gen']} -> {attempt['spawn_gen']}",
                    "observed_at": when})
    return {"schema": SCHEMA, "fleet_id": fleet_id, "label": "Firstmate · Control Tower",
            "observed_at": when, "sessions": list(sessions.values()),
            "tasks": list(attempts.values()), "links": list(links.values()),
            "diagnostics": diagnostics}


def run_json(argv, env=None, timeout=45):
    # All arguments are separate argv entries. Stop only this observation's
    # process group on timeout, including any shell children it started.
    with subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, env=env, start_new_session=True) as process:
        try:
            out, err = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise RuntimeError(f"observation timed out: {argv[0]}")
        if process.returncode:
            raise RuntimeError((err or out or f"command failed: {argv[0]}").strip()[:600])
        data = json.loads(out)
        if isinstance(data, dict) and data.get("error"):
            raise RuntimeError(str(data["error"])[:600])
        return data


def pane_get(herdr, target):
    if target.count(":") == 1:
        # An explicit pane ID from the plugin invocation uses its inherited
        # current Herdr context. Firstmate endpoints below include a session.
        data = run_json([herdr, "pane", "get", target], timeout=8)
        pane = data["result"]["pane"]
        if pane.get("pane_id") != target:
            raise ValueError("pane identity changed during observation")
        return pane
    session, separator, pane_id = target.partition(":")
    if not separator or not session or ":" not in pane_id:
        raise ValueError(f"invalid Firstmate Herdr target: {target}")
    data = run_json([herdr, "pane", "get", pane_id, "--session", session], timeout=8)
    pane = data["result"]["pane"]
    if pane.get("pane_id") != pane_id:
        raise ValueError("pane identity changed during observation")
    return pane


def snapshot_env(home, herdr):
    # Firstmate reads crew state by running `herdr` from PATH. A plugin pane may
    # only know Herdr through HERDR_BIN_PATH, so lend the snapshot that directory.
    env = dict(os.environ, FM_HOME=str(home))
    if os.sep in herdr:
        directory = os.path.dirname(os.path.abspath(herdr))
        env["PATH"] = os.pathsep.join(filter(None, (directory, env.get("PATH"))))
    return env


def collect(home, herdr, previous, captain_target):
    env = snapshot_env(home, herdr)
    command = [str(home / "bin" / "fm-fleet-snapshot.sh"), "--json"]
    before = run_json(command, env)
    check_snapshot(before)
    panes = {}
    errors = []
    for task in before["tasks"]:
        target = (task.get("endpoint") or {}).get("target")
        if target and not task.get("remote") and task.get("backend") == "herdr":
            try:
                panes[target] = pane_get(herdr, target)
            except (RuntimeError, ValueError, KeyError) as error:
                errors.append(f"{task['id']}: {error}")
    captain = None
    if captain_target:
        try:
            captain = pane_get(herdr, captain_target)
        except (RuntimeError, ValueError, KeyError) as error:
            errors.append(f"Captain: {error}")
    after = run_json(command, env)
    manifest = build_manifest(before, after, panes, previous, captain)
    manifest["diagnostics"].extend(errors)
    return manifest


def semantic(value):
    if isinstance(value, dict):
        return {k: semantic(v) for k, v in value.items() if k != "observed_at"}
    if isinstance(value, list):
        return [semantic(v) for v in value]
    return value


def recover(output):
    latest = None

    def accept(candidate):
        nonlocal latest
        if (not isinstance(candidate, dict) or candidate.get("schema") != SCHEMA
                or not isinstance(candidate.get("observed_at"), str)
                or not candidate.get("fleet_id")
                or any(not isinstance(candidate.get(field), list)
                       for field in ("sessions", "tasks", "links"))):
            return
        try:
            stamp = dt.datetime.fromisoformat(candidate["observed_at"].replace("Z", "+00:00"))
            if stamp.tzinfo is None:
                return
        except ValueError:
            return
        if latest is None or stamp >= latest[0]:
            latest = (stamp, candidate)

    if output.exists():
        try:
            accept(json.loads(output.read_text()))
        except (ValueError, OSError):
            pass
    journal = output.with_suffix(".events.jsonl")
    if journal.exists():
        # A trailing interrupted journal append is ignored. Replay the latest
        # complete checkpoint; native transcript contents never enter this file.
        with journal.open() as stream:
            for line in stream:
                try:
                    record = json.loads(line)
                    if isinstance(record, dict) and record.get("schema") == "zoetrope.fleet.journal.v1":
                        accept(record.get("manifest"))
                except ValueError:
                    continue
    return latest[1] if latest else None


def publish(output, manifest, previous):
    if previous is None or semantic(previous) != semantic(manifest):
        journal = output.with_suffix(".events.jsonl")
        # Repair only an interrupted final append, retaining every complete record.
        if journal.exists():
            with journal.open("r+b") as stream:
                stream.seek(0, os.SEEK_END)
                end = stream.tell()
                if end:
                    stream.seek(end - 1)
                    if stream.read(1) != b"\n":
                        cursor = end
                        while cursor:
                            start = max(0, cursor - 4096)
                            stream.seek(start)
                            chunk = stream.read(cursor - start)
                            at = chunk.rfind(b"\n")
                            if at >= 0:
                                stream.truncate(start + at + 1)
                                break
                            cursor = start
                        else:
                            stream.truncate(0)
        with journal.open("a") as stream:
            stream.write(json.dumps({"schema": "zoetrope.fleet.journal.v1", "manifest": manifest}) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
    fd, name = tempfile.mkstemp(prefix=".fleet-", dir=output.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(manifest, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, output)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--captain", help="explicit Herdr session:pane target")
    parser.add_argument("--watch", action="store_true")
    parser.add_argument("--interval", type=float, default=5)
    parser.add_argument("--view", type=Path, help="Fleet-capable binary to run after initial collection")
    args = parser.parse_args()
    if os.environ.get("HERDR_ENV") != "1":
        parser.error("run the Firstmate adapter inside a genuine Herdr pane (HERDR_ENV=1)")
    if args.interval < 1:
        parser.error("interval must be at least one second")
    home = args.home.resolve(strict=True)
    output = args.output.absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    os.umask(0o077)
    lock = output.with_suffix(".lock").open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        parser.error("a collector already owns this fleet; open its existing manifest")
    previous = recover(output)
    viewer = None
    try:
        while True:
            try:
                manifest = collect(home, os.environ.get("HERDR_BIN_PATH", "herdr"), previous, args.captain)
                publish(output, manifest, previous)
                previous = manifest
            except (OSError, RuntimeError, ValueError, KeyError) as error:
                message = f"fleet refresh failed; retaining last snapshot: {error}"
                if previous is None or not args.watch:
                    print(message, file=sys.stderr)
                    return 1
                # Do not print into the viewer's raw terminal while it renders.
                # Keep the observation timestamp old and surface the failure in
                # the manifest banner instead of presenting stale data as fresh.
                failed = copy.deepcopy(previous)
                failed["diagnostics"] = [message]
                publish(output, failed, previous)
                previous = failed
            if args.view and viewer is None:
                viewer = subprocess.Popen([str(args.view), "fleet", str(output)])
            if not args.watch:
                return viewer.wait() if viewer else 0
            until = time.monotonic() + args.interval
            while time.monotonic() < until:
                if viewer and viewer.poll() is not None:
                    return viewer.returncode
                time.sleep(0.2)
    except KeyboardInterrupt:
        return 0
    finally:
        if viewer and viewer.poll() is None:
            viewer.terminate()
            viewer.wait(timeout=5)
        lock.close()


if __name__ == "__main__":
    sys.exit(main())
