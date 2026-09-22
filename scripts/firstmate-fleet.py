#!/usr/bin/env python3
"""Read Firstmate/Herdr registrations and publish a durable Zoetrope manifest.

Live mode is only available inside Herdr. Offline fixtures call build_manifest;
they never use a socket, launch an agent, or impersonate a Herdr environment.

Beside the manifest, the journal records lifecycle events (spawned, bound,
status, torn_down) and the coverage windows in which this adapter was
observing, for the fleet timeline. See Bridge.
"""
import argparse
import copy
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

SCHEMA = "zoetrope.fleet.v1"
JOURNAL = "zoetrope.fleet.journal.v2"
LEGACY_JOURNAL = "zoetrope.fleet.journal.v1"


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
    """One observation: the joined manifest and the snapshot it was built from."""
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
    return manifest, after


def iso(epoch):
    return dt.datetime.fromtimestamp(epoch, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def snapshot_epoch(snapshot):
    """When the snapshot observed, in whole seconds, as Firstmate states it."""
    epoch = snapshot.get("generated_epoch")
    if isinstance(epoch, int) and not isinstance(epoch, bool):
        return epoch
    stamp = dt.datetime.fromisoformat(str(snapshot.get("generated")).replace("Z", "+00:00"))
    if stamp.tzinfo is None:
        raise ValueError("snapshot generated time has no zone")
    return int(stamp.timestamp())


def spawn_epoch(spawn_gen):
    # fm-spawn.sh writes s<epoch>.<pid>.<rand>. Any other shape stays opaque.
    match = re.match(r"s(\d{9,12})\.", spawn_gen or "")
    return int(match.group(1)) if match else None


class Bridge:
    """Turn successive Firstmate snapshots into lifecycle events.

    Times are only as good as their evidence, and each event says which
    (at_quality): a status line's own [at=] stamp comes back exactly as
    generated - last_event.age_seconds ("stamp"); a spawn from the epoch in its
    spawn_gen ("derived"); everything else is when this bridge noticed it
    ("observed"). Only the last status line is visible per poll, so lines that
    land between polls, and anything while no bridge runs, are lost. Coverage
    events say when the bridge was watching, so the timeline shows those gaps
    instead of a steady state it never saw.
    """

    def __init__(self, fleet_id, run, max_gap=60, checkpoint=60):
        self.fleet_id = fleet_id
        self.run = run
        self.max_gap = max_gap
        self.checkpoint = checkpoint
        # (task, spawn_gen) -> {"session": key or None, "status": mark or None, "down": bool}
        self.attempts = {}
        self.window = None  # [first, last] poll epochs of the open coverage window
        self.written = None  # how far that window's coverage lines reach

    def recover(self, records):
        """Resume from journal records, so a restart re-emits nothing it wrote."""
        for record in records:
            if record.get("schema") != JOURNAL or record.get("kind") != "lifecycle":
                continue
            event = record.get("event") or {}
            attempt = event.get("attempt")
            if not isinstance(attempt, dict):
                continue
            state = self.state((attempt.get("task"), attempt.get("spawn_gen")))
            kind = event.get("type")
            if kind == "bound":
                state["session"] = event.get("session")
            elif kind == "status":
                status = event.get("status") or {}
                stamped = event.get("at_quality") == "stamp"
                state["status"] = (status.get("digest"), event.get("at") if stamped else None)
            elif kind == "torn_down":
                state["down"] = True

    def state(self, key):
        return self.attempts.setdefault(key, {"session": None, "status": None, "down": False})

    def line(self, kind, ident, at, quality, attempt=None, **payload):
        event = {"id": f"bridge:{self.fleet_id}#{kind}/{ident}",
                 "source": {"kind": "bridge", "run": self.run},
                 "at": iso(at), "at_quality": quality}
        if attempt:
            event["attempt"] = {"task": attempt[0], "spawn_gen": attempt[1]}
        event["type"] = kind
        event.update(payload)
        return {"schema": JOURNAL, "kind": "lifecycle", "event": event}

    def observe(self, snapshot, manifest):
        """Lifecycle lines for one successful poll: `snapshot` is what it saw,
        `manifest` what it joined from that."""
        now = snapshot_epoch(snapshot)
        lines = []
        present = set()
        for task in snapshot["tasks"]:
            gen = task.get("spawn_gen")
            if not gen or task.get("remote"):
                continue  # No attempt identity, or a remote home: out of reach.
            key = (task["id"], gen)
            present.add(key)
            ident = f"{task['id']}/{gen}"
            if key not in self.attempts:
                epoch = spawn_epoch(gen)
                at, quality = (epoch, "derived") if epoch is not None and epoch <= now else (now, "observed")
                spawned = {k: task.get(k) for k in ("kind", "harness", "project") if task.get(k)}
                lines.append(self.line("spawned", ident, at, quality, key, spawned=spawned))
            state = self.state(key)
            last = ((task.get("paths") or {}).get("status_log") or {}).get("last_event") or {}
            raw, verb = last.get("raw") or "", last.get("state") or ""
            if raw and verb:
                age = last.get("age_seconds")
                stamped = isinstance(age, int) and not isinstance(age, bool) and age >= 0
                at, quality = (now - age, "stamp") if stamped else (now, "observed")
                digest = hashlib.sha256(raw.encode()).hexdigest()[:16]
                mark = (digest, iso(at) if stamped else None)
                if state["status"] != mark:
                    status = {"value": verb, "digest": digest}
                    found = re.search(r"\[key=([^\]\s]+)\]", raw)
                    if found:
                        status["key"] = found.group(1)
                    note = (last.get("note") or "").strip()
                    if note:
                        status["note"] = note[:200]
                    lines.append(self.line("status", f"{ident}/{at}/{digest}", at, quality, key,
                                           status=status))
                    state["status"] = mark
        for task in manifest["tasks"]:
            key = (task["id"], task["spawn_gen"])
            state = self.attempts.get(key)
            session = task.get("session")
            if session and key in present and state["session"] != session:
                lines.append(self.line("bound", f"{key[0]}/{key[1]}/{session['session_id']}", now,
                                       "observed", key, session=session))
                state["session"] = session
        for key, state in self.attempts.items():
            if key not in present and not state["down"]:
                lines.append(self.line("torn_down", f"{key[0]}/{key[1]}", now, "observed", key))
                state["down"] = True
        return self.cover(now, bool(lines)) + lines

    def cover(self, now, due=False):
        """Extend the open coverage window to `now`, writing a segment when
        other events need one or a checkpoint is due. Segments chain end to
        start; a pause longer than max_gap starts a new window."""
        lines = []
        if self.window and now - self.window[1] > self.max_gap:
            lines += self.close()
        if not self.window:
            self.window = [now, now]
            due = True
        self.window[1] = now
        if due or now - self.written >= self.checkpoint:
            lines.append(self.segment())
        return lines

    def segment(self):
        start = self.window[0] if self.written is None else self.written
        self.written = self.window[1]
        return self.line("coverage", f"{self.run}/{start}-{self.window[1]}", self.window[1], "observed",
                         coverage={"from": iso(start), "to": iso(self.window[1])})

    def close(self):
        """End the open window at its last poll, e.g. when the bridge stops."""
        lines = []
        if self.window and self.written != self.window[1]:
            lines.append(self.segment())
        self.window = None
        self.written = None
        return lines


def semantic(value):
    if isinstance(value, dict):
        return {k: semantic(v) for k, v in value.items() if k != "observed_at"}
    if isinstance(value, list):
        return [semantic(v) for v in value]
    return value


def journal_records(output):
    """Every complete, parseable journal record, oldest first."""
    journal = output.with_suffix(".events.jsonl")
    if not journal.exists():
        return
    # A trailing interrupted append is ignored; native transcript contents
    # never enter this file.
    with journal.open() as stream:
        for line in stream:
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if isinstance(record, dict):
                yield record


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
    # Replay the latest complete checkpoint, from either journal version.
    for record in journal_records(output):
        if (record.get("schema") == LEGACY_JOURNAL
                or (record.get("schema") == JOURNAL and record.get("kind") == "manifest")):
            accept(record.get("manifest"))
    return latest[1] if latest else None


def append_journal(output, records):
    """Append whole lines in one write, after dropping an interrupted one."""
    if not records:
        return
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
        stream.write("".join(json.dumps(record) + "\n" for record in records))
        stream.flush()
        os.fsync(stream.fileno())


def publish(output, manifest, previous, lifecycle=()):
    records = list(lifecycle)
    if previous is None or semantic(previous) != semantic(manifest):
        records.append({"schema": JOURNAL, "kind": "manifest", "manifest": manifest})
    append_journal(output, records)
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
    bridge = None
    viewer = None
    try:
        while True:
            try:
                manifest, snapshot = collect(home, os.environ.get("HERDR_BIN_PATH", "herdr"),
                                             previous, args.captain)
                if bridge is None:
                    bridge = Bridge(manifest["fleet_id"], f"{now()}/{os.getpid()}",
                                    max_gap=max(60, 4 * args.interval))
                    bridge.recover(journal_records(output))
                publish(output, manifest, previous, bridge.observe(snapshot, manifest))
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
        if bridge:
            try:
                append_journal(output, bridge.close())
            except OSError:
                pass
        lock.close()


if __name__ == "__main__":
    sys.exit(main())
