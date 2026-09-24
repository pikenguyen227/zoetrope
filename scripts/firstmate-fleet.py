#!/usr/bin/env python3
"""Read Firstmate/Herdr registrations and publish a durable Zoetrope manifest.

Live mode is only available inside Herdr. Offline fixtures call build_manifest;
they never use a socket, launch an agent, or impersonate a Herdr environment.

Beside the manifest, the journal records lifecycle events (spawned, bound,
status, torn_down, ...) and the windows they cover, for the fleet timeline.
Firstmate's own fm-lifecycle.v1 feeds are the source (see Feeds); where no
feed speaks, the adapter bridges lifecycle from successive snapshots (see
Bridge). The no-mistakes validation run Firstmate attributes to a task is
read through two allow-listed CLI calls and journalled as it changes (see
Validation); the adapter never drives a run or its daemon.

--archive and --delete remove records: the whole fleet, or one worker with
--worker. Archive moves them into a backup-<time> directory beside the
manifest; delete removes them permanently, after a confirmation. While a
collector runs, its viewer asks it instead (keys C, A and D), since the
collector would write back anything removed under it. Agent transcripts and
the Firstmate home are never touched.
"""
import argparse
import contextlib
import copy
import datetime as dt
import errno
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

SCHEMA = "zoetrope.fleet.v1"
# The viewer exits with this status after writing a request to the file named
# in ZOE_FLEET_REQUEST (REQUEST_EXIT in src/fleet/native.rs).
REQUEST_EXIT = 75
NOT_OBSERVED = "not observed"
# The fleet's name when no Herdr workspace names it.
LABEL = "Firstmate · Control Tower"
JOURNAL = "zoetrope.fleet.journal.v2"
LEGACY_JOURNAL = "zoetrope.fleet.journal.v1"
FEED = "fm-lifecycle.v1"
FEED_CURSORS = "zoetrope.fleet.feeds.v1"


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


def pane_of(target):
    """The pane ID in a Firstmate `session:pane` target, or a bare pane ID."""
    return target.partition(":")[2] if target.count(":") == 2 else target


def place(pane, known=None):
    """Where a pane sits, by Herdr's own IDs, with the last names `known` gave
    those same IDs. Renaming a tab or a workspace changes none of the IDs, so
    the names are only ever display."""
    where = {k: pane[k] for k in ("pane_id", "tab_id", "workspace_id")
             if isinstance(pane.get(k), str) and pane[k]}
    for kind in ("workspace", "tab"):
        if (known or {}).get(kind) and known.get(kind + "_id") == where.get(kind + "_id"):
            where[kind] = known[kind]
    return where


def named(where, names):
    """`where` with the live names of its tab and workspace, keeping the last
    ones known when Herdr cannot name them now."""
    where = dict(where)
    for kind in ("workspace", "tab"):
        label = names(kind, where[kind + "_id"]) if where.get(kind + "_id") else None
        if label:
            where[kind] = label
    return where


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


def build_manifest(before, after, panes, previous=None, captain=None, observed=None,
                   names=None, own_workspace=None):
    """Join only stable generations/endpoints captured on both sides of pane reads.

    `panes` is keyed by Firstmate's exact session:pane target. `captain` is an
    explicitly chosen pane record, a current member rather than inferred ancestry.
    `names(kind, id)` is Herdr's live name for a "workspace" or "tab" ID, or
    None; `own_workspace` is the workspace this collector runs in.

    Identity never rests on a name, since the captain renames spaces and tabs
    at will: a Captain is the session its pane registers, a supervisor card is
    its session, and each carries its Herdr IDs (`herdr`) so its current names
    can be read again. The crew root takes the standing Captain's workspace
    name; a Captain's and a secondmate's card, its tab's name.
    """
    names = names or (lambda kind, ident: None)
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
    # Every local task's pane is crew (a worker or a secondmate), never a
    # Captain, whatever its tab is called.
    crew_panes = {}
    for current in after["tasks"]:
        target = (current.get("endpoint") or {}).get("target")
        if target and not current.get("remote") and current.get("backend") == "herdr":
            crew_panes[pane_of(target)] = current["id"]
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
        pane = panes.get(target, {}) if target else {}
        where = None
        if current.get("kind") == "secondmate":
            # A secondmate is a supervisor: its card reads its tab's name,
            # else the last one known, else its task ID.
            prior = (sessions.get(identity(task["session"])) or {}) if task["session"] else {}
            where = (named(place(pane, prior.get("herdr")), names) if pane.get("pane_id")
                     else prior.get("herdr") or {})
            task["label"] = where.get("tab") or task_id
        if current.get("remote") or current.get("backend") != "herdr":
            diagnostics.append(f"{task_id}: only local Herdr endpoints are supported")
        elif not stable:
            diagnostics.append(f"{task_id}: generation/endpoint changed; join deferred")
        elif target:
            key = registered(pane)
            if key and key["provider"] == current.get("harness"):
                known = task["session"]
                if known and key != known:
                    # Same launch generation unexpectedly names another session.
                    # Preserve history; do not silently reassign task ownership.
                    diagnostics.append(f"{task_id}: session changed without a new spawn generation")
                else:
                    task["session"] = key
                    # Its task names it, never an earlier collection's guess
                    # (such as a Captain label when its pane was taken for one).
                    spec = sessions.setdefault(identity(key), {"key": key, "label": task_id})
                    spec["label"] = task["label"]
                    if where:
                        spec["herdr"] = where
                    else:
                        spec.pop("herdr", None)
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
    # A session no attempt names is a Captain. One Captain stands at a time:
    # the pane's, or else the last registered. Earlier ones keep their
    # membership and history, but are no longer registered.
    joined = {identity(t["session"]) for t in attempts.values() if t.get("session")}
    standing = registered(captain) if captain else None
    # The pane is only where this collector looks. A crew pane is never a
    # Captain, whatever its tab or workspace is called: a secondmate's own
    # autostart can hand this collector its pane.
    crew = crew_panes.get((captain or {}).get("pane_id"))
    if standing and not crew and identity(standing) in joined:
        crew = next(t["id"] for t in attempts.values()
                    if t.get("session") and identity(t["session"]) == identity(standing))
    if crew:
        standing = None
    if standing:
        # Newest registration last: it is the Captain that stands.
        known = sessions.pop(identity(standing), {})
        where = named(place(captain, known.get("herdr")), names)
        sessions[identity(standing)] = {"key": standing, "herdr": where,
                                        "label": where.get("tab") or "Captain · " + standing["provider"]}
    captains = [k for k in sessions if k not in joined]
    for k in captains:
        sessions[k].pop("runtime", None)
        if k != captains[-1]:
            sessions[k]["runtime"] = observation(NOT_OBSERVED, "herdr.pane.get", when)
    head = sessions[captains[-1]] if captains else {}
    if head and not standing and head.get("herdr"):
        # The last Captain registered still stands: read its names again.
        head["herdr"] = named(head["herdr"], names)
        head["label"] = head["herdr"].get("tab") or head["label"]
    if captain and not standing:
        what = (f"Captain pane {captain.get('pane_id')} is task {crew}'s pane, not a Captain" if crew
                else f"Captain pane {captain.get('pane_id')} has no supported registered session")
        diagnostics.append(f"{what}; showing the last Captain registered" if captains else
                           f"{what}; no Captain registered yet" if crew else
                           "Captain pane has no supported registered session yet")
    # The crew is named after the standing Captain's workspace, else the
    # Captain pane's, else this collector's unless a crew pane's is (the Fleet
    # tab opens where it was invoked, a secondmate's space included).
    title = (head.get("herdr") or {}).get("workspace")
    if not title and captain and not crew and captain.get("workspace_id"):
        title = names("workspace", captain["workspace_id"])
    crew_workspaces = {p.get("workspace_id") for p in panes.values()} | {
        (captain or {}).get("workspace_id") if crew else None}
    if not title and own_workspace and own_workspace not in crew_workspaces:
        title = names("workspace", own_workspace)
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
    return {"schema": SCHEMA, "fleet_id": fleet_id, "label": title or LABEL,
            "observed_at": when, "sessions": list(sessions.values()),
            "tasks": list(attempts.values()), "links": list(links.values()),
            "diagnostics": diagnostics}


def run_text(argv, env=None, timeout=45, cwd=None):
    """(exit status, stdout, stderr) of one observation command.

    All arguments are separate argv entries. Stop only this observation's
    process group on timeout, including any shell children it started."""
    with subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, env=env, cwd=cwd, start_new_session=True) as process:
        try:
            out, err = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise TimeoutError(f"observation timed out: {argv[0]}")
        return process.returncode, out, err


def run_json(argv, env=None, timeout=45):
    try:
        code, out, err = run_text(argv, env, timeout)
    except TimeoutError as error:
        raise RuntimeError(str(error))
    if code:
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


def snapshot_env(home, herdr, no_mistakes=None):
    # Firstmate reads crew state by running `herdr` from PATH. A plugin pane may
    # only know Herdr through HERDR_BIN_PATH, so lend the snapshot that directory.
    # It attributes a validation run only when `no-mistakes` is on PATH too, and
    # a Herdr pane's PATH rarely holds it: lend its directory as well, last, so
    # it shadows nothing the snapshot would otherwise find.
    env = dict(os.environ, FM_HOME=str(home))
    if os.sep in herdr:
        directory = os.path.dirname(os.path.abspath(herdr))
        env["PATH"] = os.pathsep.join(filter(None, (directory, env.get("PATH"))))
    if no_mistakes:
        directory = os.path.dirname(os.path.abspath(no_mistakes))
        if directory not in (env.get("PATH") or "").split(os.pathsep):
            env["PATH"] = os.pathsep.join(filter(None, (env.get("PATH"), directory)))
    return env


def find_no_mistakes(environ=None):
    """The no-mistakes CLI, as (absolute path, None), or (None, why not):
    ZOE_NO_MISTAKES_BIN when set, else the first on PATH. Without it Firstmate
    attributes no validation run, so the why is a manifest diagnostic."""
    environ = os.environ if environ is None else environ
    named = environ.get("ZOE_NO_MISTAKES_BIN")
    if named:
        if os.path.isfile(named) and os.access(named, os.X_OK):
            return os.path.abspath(named), None
        return None, (f"ZOE_NO_MISTAKES_BIN {named!r} is not an executable file, so no-mistakes "
                      "is not run: no validation run is attributed or read, so no card shows one")
    found = shutil.which("no-mistakes", path=environ.get("PATH", os.defpath))
    if found:
        return os.path.abspath(found), None
    return None, ("no-mistakes is not on this collector's PATH; set ZOE_NO_MISTAKES_BIN to it: "
                  "no validation run is attributed or read, so no card shows one")


def collect(home, herdr, previous, captain_target):
    """One observation: the joined manifest and the snapshot it was built from."""
    no_mistakes, missing = find_no_mistakes()
    env = snapshot_env(home, herdr, no_mistakes)
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
    cache = {}

    def names(kind, ident):
        if (kind, ident) not in cache:
            cache[kind, ident] = herdr_name(herdr, kind, ident)
        return cache[kind, ident]

    manifest = build_manifest(before, after, panes, previous, captain, names=names,
                              own_workspace=os.environ.get("HERDR_WORKSPACE_ID"))
    manifest["diagnostics"].extend(errors)
    if missing:
        manifest["diagnostics"].append(missing)
    return manifest, after


def herdr_name(herdr, kind, ident):
    """Herdr's current name for a "workspace" or "tab" ID; None when Herdr
    cannot name it. Names are display only: the captain renames them freely."""
    try:
        data = run_json([herdr, kind, "get", ident], timeout=8)
        label = data["result"][kind]["label"]
    except (RuntimeError, ValueError, KeyError, TypeError):
        return None
    label = label.strip() if isinstance(label, str) else ""
    return label if label and label.isprintable() else None


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
    ("observed"). A status line carries the feed key Firstmate publishes for
    it (last_event.lifecycle_key) when it could establish one: that key, not
    the words or the time, is which line it is. Only the last status line is
    visible per poll, so lines that land between polls, and anything while no
    bridge runs, are lost. Coverage events say when the bridge was watching,
    so the timeline shows those gaps instead of a steady state it never saw.

    Where a Firstmate feed has recorded a fact (Reach), the bridge keeps
    quiet about it: it still joins sessions, which no feed knows, but writes
    no spawned, status or torn_down the feed already holds.
    """

    def __init__(self, fleet_id, run, max_gap=60, checkpoint=60):
        self.fleet_id = fleet_id
        self.run = run
        self.max_gap = max_gap
        self.checkpoint = checkpoint
        # (task, spawn_gen) -> {"session": key or None, "down": bool,
        #                       "status": (digest, stamp, feed key) of the last line, or None}
        self.attempts = {}
        self.window = None  # [first, last] poll epochs of the open coverage window
        self.written = None  # how far that window's coverage lines reach
        self.reach = Reach()

    def recover(self, records):
        """Resume from journal records, so a restart re-emits nothing it wrote."""
        for record in records:
            if record.get("schema") != JOURNAL or record.get("kind") != "lifecycle":
                continue
            event = record.get("event") or {}
            if (event.get("source") or {}).get("kind") != "bridge":
                self.reach.add(event)
                continue
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
                state["status"] = (status.get("digest"), event.get("at") if stamped else None,
                                   status.get("lifecycle_key"))
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
                line = self.line("spawned", ident, at, quality, key, spawned=spawned)
                if not self.reach.holds(line["event"]):
                    lines.append(line)
            state = self.state(key)
            last = ((task.get("paths") or {}).get("status_log") or {}).get("last_event") or {}
            raw, verb = last.get("raw") or "", last.get("state") or ""
            if raw and verb:
                age = last.get("age_seconds")
                stamped = isinstance(age, int) and not isinstance(age, bool) and age >= 0
                at, quality = (now - age, "stamp") if stamped else (now, "observed")
                digest = hashlib.sha256(raw.encode()).hexdigest()[:16]
                lifecycle_key = last.get("lifecycle_key")
                if not (isinstance(lifecycle_key, str) and lifecycle_key):
                    lifecycle_key = None  # Firstmate could not establish it, or predates it.
                mark = (digest, iso(at) if stamped else None, lifecycle_key)
                status = {"value": verb, "digest": digest}
                if lifecycle_key:
                    status["lifecycle_key"] = lifecycle_key
                found = re.search(r"\[key=([^\]\s]+)\]", raw)
                if found:
                    status["key"] = found.group(1)
                note = (last.get("note") or "").strip()
                if note:
                    status["note"] = note[:200]
                suffix = f"/{lifecycle_key}" if lifecycle_key else ""
                line = self.line("status", f"{ident}/{at}/{digest}{suffix}", at, quality, key, status=status)
                if not same_line(state["status"], mark) and not self.reach.holds(line["event"]):
                    lines.append(line)
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
                line = self.line("torn_down", f"{key[0]}/{key[1]}", now, "observed", key)
                if not self.reach.holds(line["event"]):
                    lines.append(line)
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
                         coverage={"from": iso(start), "to": iso(self.window[1]),
                                   "max_gap": math.ceil(self.max_gap)})

    def close(self):
        """End the open window at its last poll, e.g. when the bridge stops."""
        lines = []
        if self.window and self.written != self.window[1]:
            lines.append(self.segment())
        self.window = None
        self.written = None
        return lines


def same_line(before, now):
    """Whether two sightings, each (digest, stamp, feed key), are the same
    status line: by feed key when both carry one, so identical words at two
    offsets are two lines; otherwise, as before the key existed, by words and
    stamp."""
    if before is None:
        return False
    if before[2] and now[2]:
        return before[2] == now[2]
    return before[:2] == now[:2]


class Reach:
    """What Firstmate's own feed has recorded, so its events replace the
    bridge's (Lifecycle::superseded in src/fleet/journal.rs applies the same
    rule to what the journal already holds).

    A bridged spawn, teardown or status gives way only to the feed's own
    record of it: the attempt's spawn, its teardown, or, for a status, the
    dated feed event whose key is the line's lifecycle_key (an undated one
    never places, so it cannot stand in). A status without that
    key falls back to a feed status stamped at the same moment. So a status
    the feed lost is bridged, and one the bridge sees before Firstmate records
    it is written and gives way once the feed has it. A status line with
    neither key nor stamp has no identity the two sides share, so its bridged
    record always stands, even beside the feed's.
    """

    def __init__(self):
        self.marks = set()  # what the feed holds, as mark() of its events

    @staticmethod
    def mark(event):
        attempt = event.get("attempt")
        if not isinstance(attempt, dict):
            return None
        key = (attempt.get("task"), attempt.get("spawn_gen"), event.get("type"))
        if event.get("type") in ("spawned", "torn_down"):
            return key
        if event.get("type") == "status" and event.get("at_quality") == "stamp":
            return key + (event.get("at"),)
        return None

    @staticmethod
    def feed_key(event):
        """A feed event's own key: its ID is firstmate:<home>#<key> (translate)."""
        prefix = f"firstmate:{(event.get('source') or {}).get('home')}#"
        ident = event.get("id")
        if isinstance(ident, str) and ident.startswith(prefix) and len(ident) > len(prefix):
            return ident[len(prefix):]
        return None

    def add(self, event):
        if (event.get("source") or {}).get("kind") != "firstmate":
            return
        mark = self.mark(event)
        if mark is not None:
            self.marks.add(mark)
        key = self.feed_key(event) if event.get("type") == "status" and event.get("at") else None
        if key is not None:
            self.marks.add(("line", key))

    def holds(self, event):
        if event.get("type") == "status":
            key = (event.get("status") or {}).get("lifecycle_key")
            if key:
                return ("line", key) in self.marks
        mark = self.mark(event)
        return mark is not None and mark in self.marks


# Firstmate's at_source as the journal's at_quality. An inbox time is the one
# Firstmate stamped on the steering record it wrote, so its own clock. Any
# source this adapter does not know claims no more than having been noticed.
AT_QUALITY = {"stamp": "stamp", "firstmate": "firstmate", "inbox": "firstmate",
              "observed": "observed"}


def translate(record, home):
    """An fm-lifecycle.v1 record as journal lifecycle events: none for the
    feed's own bookkeeping (feed.started, feed.backfilled), a status line
    without a verb (continuation prose), or a type this adapter does not know."""
    kind, data, task = record.get("type"), record.get("data"), record.get("task")
    key = record.get("key")
    if not (isinstance(task, dict) and isinstance(task.get("id"), str) and task["id"]
            and isinstance(key, str) and key and isinstance(data, dict)):
        return []
    text = lambda value: value if isinstance(value, str) and value.strip() else None
    if kind == "task.spawned":
        payload = {"spawned": {k: data[k] for k in ("kind", "harness", "project") if text(data.get(k))}}
    elif kind == "task.reclassified":
        change = {side: (data.get(side) or {}).get("kind") for side in ("from", "to")
                  if isinstance(data.get(side), dict)}
        payload = {"reclassified": {k: v for k, v in change.items() if text(v)}}
    elif kind == "task.status":
        if not text(data.get("verb")):
            return []
        status = {"value": data["verb"]}
        status.update({k: data[k] for k in ("key", "note") if text(data.get(k))})
        payload = {"status": status}
    elif kind == "task.decision":
        if not text(data.get("key")) or data.get("change") not in ("opened", "replaced", "closed"):
            return []
        payload = {"decision": {k: data[k] for k in ("key", "change", "verb", "closed_by")
                                if text(data.get(k))}}
    elif kind in ("task.steered", "task.steer_acked"):
        if not text(data.get("msg")):
            return []
        payload = {kind[5:]: {"msg": data["msg"]}}
    elif kind == "task.torn_down":
        payload = {"torn_down": {"outcome": data["outcome"]} if text(data.get("outcome")) else {}}
    elif kind == "task.busy":
        if not text(data.get("state")):
            return []
        payload = {"busy": {"state": data["state"]}}
    else:
        return []
    at = record.get("at")
    if isinstance(at, int) and not isinstance(at, bool) and at >= 0:
        quality = AT_QUALITY.get(record.get("at_source"), "observed")
        if record.get("backfill") is True and record.get("at_source") == "firstmate":
            # Replayed later: a backfilled spawn's time is its spawn_gen epoch.
            quality = "backfill"
    else:
        at, quality = None, "unknown"
    source = {"kind": "firstmate", "home": home}
    seq = record.get("seq")
    if isinstance(seq, int) and not isinstance(seq, bool):
        source["seq"] = seq
    line = {"id": f"firstmate:{home}#{key}", "source": source}
    if at is not None:
        line["at"] = iso(at)
    line.update(at_quality=quality,
                attempt={"task": task["id"], "spawn_gen": text(task.get("spawn_gen")) or "unresolved"},
                type=next(iter(payload)), **payload)
    return [{"schema": JOURNAL, "kind": "lifecycle", "event": line}]


def feed_files(active):
    """A feed's files in seq order: each rotated `<stem>.<first-seq>.jsonl`,
    then the active file, as (first seq or None, path)."""
    rotated = re.compile(re.escape(active.stem) + r"\.(\d+)" + re.escape(active.suffix))
    found = sorted((int(m.group(1)), path) for path in active.parent.iterdir()
                   if (m := rotated.fullmatch(path.name)))
    return found + [(None, active)]


def read_feed(active, cursor):
    """Complete records past `cursor`, oldest first, and where reading stopped.

    `cursor` is {"file": [device, inode], "offset": bytes}: reading resumes in
    that file wherever rotation renamed it, then runs through every later
    file. When it is gone, or shrank, reading restarts at the rotated file that
    should hold the next seq (`cursor["seq"] + 1`) and the caller drops what it
    already has by seq. A final line without its newline is still being
    written and waits. Returns (records, unreadable lines, file, offset)."""
    for attempt in range(2):
        with contextlib.ExitStack() as stack:
            opened = []
            for first, path in feed_files(active):
                try:
                    stream = stack.enter_context(path.open("rb"))
                except FileNotFoundError:
                    continue  # Rotated away between listing and opening.
                info = os.fstat(stream.fileno())
                opened.append((first, stream, [info.st_dev, info.st_ino], info.st_size))
            start = next((i for i, (_, _, ident, size) in enumerate(opened)
                          if ident == cursor.get("file") and size >= cursor.get("offset", 0)), None)
            if start is None and cursor.get("file") and attempt == 0:
                continue  # Perhaps renamed mid-listing: look once more.
            offset = cursor.get("offset", 0) if start is not None else 0
            if start is None:
                start = max([i for i, (first, *_) in enumerate(opened)
                             if first is not None and first <= cursor.get("seq", 0) + 1], default=0)
            records, bad = [], 0
            file, end = cursor.get("file"), cursor.get("offset", 0)
            for i in range(start, len(opened)):
                _, stream, ident, _ = opened[i]
                position = offset if i == start else 0
                stream.seek(position)
                data = stream.read()
                complete = data.rfind(b"\n") + 1
                for line in data[:complete].splitlines():
                    if not line.strip():
                        continue
                    try:
                        record = json.loads(line)
                    except ValueError:
                        record = None
                    seq = record.get("seq") if isinstance(record, dict) else None
                    if (record is None or record.get("schema") != FEED or not isinstance(seq, int)
                            or isinstance(seq, bool) or seq < 1):
                        bad += 1
                        continue
                    records.append(record)
                file, end = ident, position + complete
            return records, bad, file, end
    return [], 0, cursor.get("file"), cursor.get("offset", 0)


class Feeds:
    """Tail the fm-lifecycle.v1 feeds Firstmate's snapshot points at: this
    home's and each local secondmate home's. A remote secondmate's feed is not
    mirrored here, so it is skipped with a diagnostic, as is a feed that cannot
    be read; neither fails a collection.

    Each feed keeps a cursor in <manifest>.feeds.json: the file and byte
    offset read up to, the last seq, and how far this home's coverage lines
    reach. A cleared or trimmed journal keeps it, so removed history does not
    come back. Seqs are gap-free, so a missing one is an event Firstmate could
    not write: it is counted and reported, never papered over, and this home's
    coverage breaks from the last record before it to the first after it.

    Firstmate writes its feed whether or not this adapter runs, so this
    home's feed covers from its feed.started to the latest read: one coverage
    segment per read that brought anything, and a checkpoint at least once a
    minute. Secondmate homes add no coverage, since coverage is fleet-wide.
    """

    def __init__(self, path, max_gap=60, checkpoint=60):
        self.path = path
        self.max_gap = max_gap
        self.checkpoint = checkpoint
        self.cursors = {}
        self.dirty = False
        self.read_at = {}  # this home's feed -> when it was last read
        try:
            state = json.loads(path.read_text())
        except (OSError, ValueError):
            return
        if isinstance(state, dict) and state.get("schema") == FEED_CURSORS:
            self.cursors = {k: v for k, v in (state.get("feeds") or {}).items() if isinstance(v, dict)}

    def segment(self, cursor, path, now):
        start = cursor["written"] if cursor["written"] is not None else cursor["since"]
        home = cursor["home"] or path
        cursor["written"] = now
        return {"schema": JOURNAL, "kind": "lifecycle", "event": {
            "id": f"firstmate:{home}#coverage/{start}-{now}",
            "source": {"kind": "firstmate", "home": home},
            "at": iso(now), "at_quality": "observed", "type": "coverage",
            "coverage": {"from": iso(start), "to": iso(now), "max_gap": math.ceil(self.max_gap)}}}

    def close(self):
        """End this home's coverage at its latest read, e.g. when the adapter
        stops; the next run's first segment carries on from there."""
        lines = []
        for path, now in self.read_at.items():
            cursor = self.cursors.get(path)
            if cursor and cursor.get("written") is not None and now > cursor["written"]:
                lines.append(self.segment(cursor, path, now))
                self.dirty = True
        self.read_at.clear()
        return lines

    def save(self):
        if self.dirty:
            write_atomic(self.path, json.dumps({"schema": FEED_CURSORS, "feeds": self.cursors},
                                               indent=2, sort_keys=True) + "\n")
            self.dirty = False

    def poll(self, snapshot):
        """Journal lines for what every feed recorded since the last poll, and
        diagnostics. Without a pointer (the feed is off, or Firstmate predates
        it) there is nothing to read and the bridge speaks alone."""
        pointer = snapshot.get("lifecycle")
        if pointer is None:
            return [], []
        if not isinstance(pointer, dict) or pointer.get("schema") != FEED:
            return [], ["Firstmate's lifecycle feed is not fm-lifecycle.v1; bridging from snapshots"]
        feeds = [("Firstmate lifecycle feed", pointer.get("path"), pointer.get("id"), True)]
        notes = []
        for child in pointer.get("homes") or []:
            if not isinstance(child, dict):
                continue
            name = f"secondmate {child.get('task') or child.get('id') or '?'}"
            if child.get("remote") or not child.get("path"):
                notes.append(f"{name}: its lifecycle feed is remote and not mirrored here; not read")
                continue
            feeds.append((f"{name} lifecycle feed", child["path"], child.get("id"), False))
        lines = []
        for name, path, home, primary in feeds:
            if not isinstance(path, str) or not os.path.isabs(path):
                notes.append(f"{name}: no absolute path to read")
                continue
            try:
                found, note = self.read(path, home, primary, snapshot_epoch(snapshot))
            except OSError as error:
                notes.append(f"{name} unreadable, skipped: {error.strerror or error}")
                continue
            lines += found
            if note:
                notes.append(f"{name}: {note}")
        return lines, notes

    def read(self, path, home, primary, now):
        active = Path(path)
        if not active.parent.is_dir():
            if primary:
                return [], None  # Enabled, nothing written yet.
            raise FileNotFoundError(errno.ENOENT, "no such directory", str(active.parent))
        cursor = dict(self.cursors.get(path) or {"file": None, "offset": 0, "seq": 0, "home": home,
                                                 "since": None, "written": None, "recorded": None,
                                                 "lost": 0, "gaps": 0, "gap": None, "bad": 0})
        records, bad, file, offset = read_feed(active, cursor)
        if records and cursor["seq"] and max(r["seq"] for r in records) < cursor["seq"] \
                and file != cursor["file"]:
            # Nothing past the cursor, only a shorter history: a new feed.
            cursor.update(seq=0, since=None, recorded=None)
        lines = []
        for record in records:
            seq = record["seq"]
            if seq <= cursor["seq"]:
                continue  # Already read, before a rotation or restart.
            recorded = record.get("recorded_at")
            recorded = recorded if isinstance(recorded, int) and not isinstance(recorded, bool) else None
            if seq > cursor["seq"] + 1:
                cursor["lost"] += seq - cursor["seq"] - 1
                cursor["gaps"] += 1
                cursor["gap"] = [cursor["seq"] + 1, seq - 1]
                if primary and cursor["since"] is not None:
                    start = cursor["written"] if cursor["written"] is not None else cursor["since"]
                    before = max(start, cursor.get("recorded") or start)
                    if before > start:
                        lines.append(self.segment(cursor, path, before))
                    cursor["written"] = max(before, recorded if recorded is not None else now)
            cursor["seq"] = seq
            if recorded is not None:
                cursor["recorded"] = recorded
            identity = (record.get("home") or {}).get("id")
            if isinstance(identity, str) and identity:
                cursor["home"] = identity
            if cursor["since"] is None:
                begun = record.get("at") if record.get("type") == "feed.started" else None
                begun = begun if isinstance(begun, int) else record.get("recorded_at")
                cursor["since"] = begun if isinstance(begun, int) and not isinstance(begun, bool) else None
            lines += translate(record, cursor["home"] or path)
        cursor["bad"] += bad
        if primary and cursor["since"] is not None:
            start = cursor["written"] if cursor["written"] is not None else cursor["since"]
            if start <= now and (lines or cursor["written"] is None or now - start >= self.checkpoint):
                lines.append(self.segment(cursor, path, now))
            self.read_at[path] = now
        cursor.update(file=file, offset=offset)
        if self.cursors.get(path) != cursor or lines:
            self.cursors[path] = cursor
            self.dirty = True
        notes = []
        if cursor["lost"]:
            first, last = cursor["gap"]
            notes.append(f"{cursor['lost']} events lost in {cursor['gaps']} seq gaps (latest "
                         f"{first}-{last}); the timeline lacks them")
        if cursor["bad"]:
            notes.append(f"{cursor['bad']} unreadable lines skipped")
        return lines, "; ".join(notes) or None


NM_TIMEOUT = 5
RUN_ID = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}")  # a ULID
CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
RUN_STATUSES = {"pending", "running", "fixing", "ci", "completed", "failed", "cancelled"}
RUN_ENDED = {"completed", "failed", "cancelled"}
STEP_STATUSES = {"pending", "running", "fixing", "awaiting_approval", "completed", "skipped", "failed"}
STEP_SETTLED = {"completed", "skipped", "failed"}
OUTCOMES = {"checks-passed", "passed", "passed-with-override", "passed-with-skips", "failed", "cancelled"}
STEP_NAME = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}")
DURATION = re.compile(r"(?:(\d+)h)?(?:(\d+)m)?(?:(\d+(?:\.\d+)?)s)?(?:(\d+)ms)?")
TOON_KEY = re.compile(r"(?P<key>[A-Za-z_][A-Za-z0-9_]*)"
                      r"(?:\[(?P<count>\d+)\](?:\{(?P<fields>[A-Za-z0-9_,]*)\})?)?:(?: (?P<value>.*))?")


class Drift(ValueError):
    """no-mistakes printed something this adapter does not understand. It is
    reported, never guessed at: nothing is journalled from that read."""


class Unreadable(RuntimeError):
    """The CLI answered with its own error instead of a run."""


def run_epoch(run_id):
    """When a run was created, from its ULID: its first ten characters count
    milliseconds since the epoch. None for anything that is not a ULID."""
    if not isinstance(run_id, str) or not RUN_ID.fullmatch(run_id):
        return None
    ms = 0
    for char in run_id[:10]:
        ms = ms * 32 + CROCKFORD.index(char)
    return ms // 1000


class Rows:
    """A TOON table or list, `key[N]{a,b}:` or `key[N]:`, with its rows kept
    unsplit until read (toon_table): a part this adapter does not read (help,
    next_action, ...) cannot fail a read."""

    def __init__(self, count, fields, rows, inline):
        self.count, self.fields, self.rows, self.inline = count, fields, rows, inline


def toon(text):
    """The TOON `axi status` prints, as nested dicts: `key: value` is a string
    (a quoted one unescaped), `key:` an object of the lines one level in, and
    a table or list is Rows. Indentation is two spaces a level, and a key
    appears once per object; anything else is Drift."""
    lines = []
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        body = line.lstrip(" ")
        indent = len(line) - len(body)
        if indent % 2 or body[0].isspace():
            raise Drift(f"line {number}: indentation")
        lines.append((indent // 2, body.rstrip(), number))
    return toon_block(lines, 0)


def toon_block(lines, depth):
    node, i = {}, 0
    while i < len(lines):
        level, body, number = lines[i]
        match = TOON_KEY.fullmatch(body)
        if level != depth or not match:
            raise Drift(f"line {number}: {'indentation' if level != depth else 'not a key'}")
        key = match["key"]
        if key in node:
            raise Drift(f"line {number}: {key} twice")
        start = i = i + 1
        while i < len(lines) and lines[i][0] > depth:
            i += 1
        children = lines[start:i]
        if match["count"] is not None:
            if any(level != depth + 1 for level, _, _ in children):
                raise Drift(f"line {number}: {key} rows are not one level in")
            fields = match["fields"].split(",") if match["fields"] is not None else None
            node[key] = Rows(int(match["count"]), fields, [body for _, body, _ in children],
                             match["value"])
        elif match["value"] is not None:
            if children:
                raise Drift(f"line {number}: {key} has a value and lines under it")
            node[key] = toon_scalar(match["value"], f"line {number}")
        else:
            node[key] = toon_block(children, depth + 1)
    return node


def toon_scalar(text, where):
    if not text.startswith('"'):
        return text
    try:
        value = json.loads(text)
    except ValueError:
        raise Drift(f"{where}: unterminated quote") from None
    if not isinstance(value, str):
        raise Drift(f"{where}: not a string")
    return value


def toon_cells(row, where):
    """One table row's cells: comma-separated, a quoted cell unescaped."""
    cells, i = [], 0
    while True:
        if row.startswith('"', i):
            j = i + 1
            while j < len(row) and row[j] != '"':
                j += 2 if row[j] == "\\" else 1
            if j >= len(row):
                raise Drift(f"{where}: unterminated quote")
            cells.append(toon_scalar(row[i:j + 1], where))
            i = j + 1
        else:
            j = row.find(",", i)
            j = len(row) if j < 0 else j
            cells.append(row[i:j])
            i = j
        if i == len(row):
            return cells
        if row[i] != ",":
            raise Drift(f"{where}: text after a quoted cell")
        i += 1


def toon_table(node, key, need):
    """Table `key`'s rows as dicts: Drift unless it has the columns in `need`
    and exactly as many rows as its header counts."""
    table = node.get(key)
    if not isinstance(table, Rows) or table.fields is None:
        raise Drift(f"{key} is not a table")
    missing = [field for field in need if field not in table.fields]
    if missing:
        raise Drift(f"{key} has no {', '.join(missing)} column")
    if len(table.rows) != table.count:
        raise Drift(f"{key} counts {table.count} rows but has {len(table.rows)}")
    found = []
    for n, row in enumerate(table.rows, 1):
        cells = toon_cells(row, f"{key} row {n}")
        if len(cells) != len(table.fields):
            raise Drift(f"{key} row {n} has {len(cells)} cells for {len(table.fields)} columns")
        found.append(dict(zip(table.fields, cells)))
    return found


def whole(text, where):
    if not text.isdigit():
        raise Drift(f"{where} {text!r} is not a count")
    return int(text)


def seconds(text, where):
    """A Go-style age such as 2m10s, in whole seconds; None when empty."""
    match = DURATION.fullmatch(text)
    if not text:
        return None
    if not match or not any(match.groups()):
        raise Drift(f"{where} {text!r} is not a duration")
    h, m, s, ms = (float(g) if g else 0 for g in match.groups())
    return int(h * 3600 + m * 60 + s + ms / 1000)


def read_run(text, run_id):
    """One `axi status --run` read as (state, round ages).

    The state is what the journal records: the run's status, every step's
    status, round and findings, a waiting gate, outcome, PR and error, and
    nothing that changes on every read (activity ages, process IDs), so a run
    that did not change reads the same twice. The round ages are how long each
    active step's current round had run, in seconds, for a derived start.
    Raises Unreadable for the CLI's own error and Drift for anything else it
    does not understand."""
    doc = toon(text)
    run = doc.get("run", doc.get("other_branch_run"))
    if run is None:
        if isinstance(doc.get("error"), str):
            raise Unreadable(doc["error"])
        raise Drift("no run object")
    if not isinstance(run, dict):
        raise Drift("run is not an object")
    if run.get("id") != run_id:
        raise Drift("the output names another run")
    status = run.get("status")
    if not isinstance(status, str) or status not in RUN_STATUSES:
        raise Drift(f"run status {status!r}")
    steps, names = [], {}
    for row in toon_table(run, "steps", ("step", "status")):
        name, value = row["step"], row["status"]
        if not STEP_NAME.fullmatch(name) or name in names:
            raise Drift(f"step name {name!r}")
        if value not in STEP_STATUSES:
            raise Drift(f"{name} status {value!r}")
        step = {"step": name, "status": value}
        if "findings" in row:
            step["findings"] = whole(row["findings"], f"{name} findings")
        if value in STEP_SETTLED and "duration_ms" in row:
            # Time the step ran, parked time excluded: a fact about the step,
            # not a place on the timeline. It changes while a step is active.
            step["duration_ms"] = whole(row["duration_ms"], f"{name} duration_ms")
        steps.append(step)
        names[name] = step
    ages = {}
    if "active_steps" in run:
        for row in toon_table(run, "active_steps", ("step", "status")):
            step = names.get(row["step"])
            if step is None:
                raise Drift(f"active step {row['step']!r} is not a step")
            if (row.get("round") or "").strip():
                step["round"] = row["round"].strip()[:64]
            age = seconds(row.get("round_active_for") or "", f"{row['step']} round_active_for")
            if age is not None:
                ages[row["step"]] = age
    state = {"status": status, "steps": steps}
    for field in ("branch", "pr"):
        if isinstance(run.get(field), str) and run[field].strip():
            state[field] = run[field][:1024]
    gate = doc.get("gate")
    if gate is not None:
        if not isinstance(gate, dict) or not isinstance(gate.get("step"), str) or gate["step"] not in names:
            raise Drift("the gate names no step")
        findings = toon_table(gate, "findings", ("action",)) if "findings" in gate else []
        state["gate"] = {"step": gate["step"], "findings": len(findings),
                         "ask_user": sum(1 for f in findings if f["action"] == "ask-user")}
        if isinstance(gate.get("status"), str) and gate["status"]:
            state["gate"]["status"] = gate["status"][:64]
    if "outcome" in doc:
        if not isinstance(doc["outcome"], str) or doc["outcome"] not in OUTCOMES:
            raise Drift(f"outcome {doc['outcome']!r}")
        state["outcome"] = doc["outcome"]
    if isinstance(doc.get("error"), str) and doc["error"].strip():
        state["error"] = doc["error"][:1024]
    return state, ages


class NoMistakes:
    """The no-mistakes CLI, limited to two reads: `axi status --run <id>`,
    which reads a run's record without the daemon's socket, and `daemon
    status`, which says whether the daemon behind that record still runs.
    Both run from / with the CLI's network update check off and a short
    timeout. Nothing here responds to, aborts, reruns, syncs or attaches to a
    run, touches the daemon, or opens no-mistakes' database."""

    def __init__(self, binary=None, timeout=NM_TIMEOUT):
        self.binary = binary or find_no_mistakes()[0] or "no-mistakes"
        self.timeout = timeout

    def call(self, *argv):
        if argv != ("daemon", "status") and not (len(argv) == 4 and argv[:3] == ("axi", "status", "--run")):
            raise ValueError(f"not an allow-listed no-mistakes read: {' '.join(argv)}")
        env = dict(os.environ, NO_MISTAKES_NO_UPDATE_CHECK="1")
        return run_text([self.binary, *argv], env, self.timeout, cwd="/")

    def daemon(self):
        """up, down, or unanswered (a timeout proves nothing), as Firstmate
        reads the same probe."""
        try:
            code, _, _ = self.call("daemon", "status")
        except TimeoutError:
            return "unanswered"
        return "up" if code == 0 else "down"

    def status(self, run_id):
        code, out, err = self.call("axi", "status", "--run", run_id)
        if not out.strip():
            raise Unreadable((err or f"no output, exit status {code}").strip()[:600])
        return out


def epoch_of(stamp):
    return int(dt.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp())


def sighting(payload):
    """The state a `validation` event recorded, as read_run returns it."""
    state = {k: copy.deepcopy(payload[k]) for k in
             ("status", "branch", "steps", "gate", "outcome", "pr", "error") if k in payload}
    for step in state.get("steps", []):
        step.pop("round_since", None)
    return state


class Validation:
    """Journal the no-mistakes run Firstmate attributes to each task.

    Firstmate publishes the attribution, the snapshot task's validation_run;
    this adapter never matches runs by branch. It reads each attributed run
    through NoMistakes, parses the output strictly (read_run), and journals a
    `validation` event only when what it read changed:

    - `started` at the run's creation time, from its ID ("derived");
    - `seen` for a change, `parked` when a gate newly waits on a decision,
      `ended` when the run newly reached completed, failed or cancelled. Each
      carries the whole state read and `since`, the read before it: the change
      happened after `since` and at or before `at`, this read ("observed").
      The CLI's times are relative, so no event claims a source time; an
      active round's start is derived from its age, to the second;
    - `daemon_down` when `no-mistakes daemon status` answers that the daemon
      is down, so a record still saying running is a dead instrument's: the
      view reads the run as unverified, and an open gate stays open. The
      record is still read, and taken only when it says the run ended;
    - `gone` when the run's record can no longer be found.

    `validation_coverage` events are the windows in which each run was read,
    like the bridge's, broken whenever a read fails: a stretch nobody read
    shows as unverified rather than steady. A read that fails or is not
    understood writes no event and is a manifest diagnostic. A run is read
    until it ends, even after its task leaves the snapshot (runs outlive their
    workers), and across restarts: recover() resumes from the journal and
    re-emits nothing it wrote.
    """

    def __init__(self, fleet_id, run, max_gap=60, checkpoint=60, reader=None, clock=None):
        self.fleet_id = fleet_id
        self.run = run
        self.max_gap = max_gap
        self.checkpoint = checkpoint
        self.reader = reader or NoMistakes()
        self.clock = clock or (lambda: int(time.time()))
        # (task, spawn_gen, run id) -> what was written and read about it.
        self.runs = {}

    def track(self, key):
        return self.runs.setdefault(key, {"started": False, "state": None, "read": None,
                                          "down": False, "final": False,
                                          "window": None, "written": None})

    def recover(self, records):
        """Resume from journal records, so a restart re-emits nothing it wrote
        and keeps reading the runs that had not ended."""
        for record in records:
            if record.get("schema") != JOURNAL or record.get("kind") != "lifecycle":
                continue
            event = record.get("event") or {}
            kind = event.get("type")
            payload = event.get(kind) if kind in ("validation", "validation_coverage") else None
            attempt = attempt_of(event)
            if ((event.get("source") or {}).get("kind") != "no_mistakes" or not isinstance(payload, dict)
                    or not attempt or run_epoch(payload.get("run")) is None):
                continue
            state = self.track(attempt + (payload["run"],))
            try:
                if kind == "validation_coverage":
                    state["read"] = max(state["read"] or 0, epoch_of(payload["to"]))
                    continue
                phase = payload.get("phase")
                if phase == "started":
                    state["started"] = True
                elif phase in ("seen", "parked", "ended"):
                    state.update(state=sighting(payload), down=False,
                                 read=max(state["read"] or 0, epoch_of(event["at"])))
                    state["final"] |= phase == "ended"
                elif phase == "daemon_down":
                    state["down"] = True
                elif phase == "gone":
                    state["final"] = True
            except (KeyError, TypeError, ValueError):
                continue

    def line(self, key, kind, ident, at, quality, payload):
        task, gen, run = key
        return {"schema": JOURNAL, "kind": "lifecycle", "event": {
            "id": f"validation:{self.fleet_id}#{task}/{gen}/{run}/{ident}",
            "source": {"kind": "no_mistakes", "run": self.run},
            "at": iso(at), "at_quality": quality,
            "attempt": {"task": task, "spawn_gen": gen},
            "type": kind, kind: payload}}

    def event(self, key, phase, at, quality="observed", ident=None, **payload):
        return self.line(key, "validation", ident or f"{phase}/{at}", at, quality,
                         dict({"run": key[2], "phase": phase}, **payload))

    def observe(self, snapshot):
        """Journal lines and manifest diagnostics for one poll."""
        lines, notes = [], []
        for task in snapshot["tasks"]:
            found, gen = task.get("validation_run"), task.get("spawn_gen")
            if found is None or not gen or task.get("remote"):
                continue  # Nothing attributed, or an attempt out of reach.
            run_id = found.get("id") if isinstance(found, dict) else None
            if run_epoch(run_id) is None:
                notes.append(f"{task['id']}: validation run {run_id!r} is not a no-mistakes run ID; not read")
                continue
            key = (task["id"], gen, run_id)
            state = self.track(key)
            if not state["started"]:
                now, epoch = self.clock(), run_epoch(run_id)
                at, quality = (epoch, "derived") if epoch <= now else (now, "observed")
                lines.append(self.event(key, "started", at, quality, ident="started"))
                state["started"] = True
        due = [key for key, state in self.runs.items() if not state["final"]]
        if not due:
            return lines, notes
        try:
            daemon = self.reader.daemon()
        except OSError as error:
            daemon = "unanswered"
            notes.append(f"no-mistakes could not be run: {error.strerror or error}")
        if daemon != "up":
            unverified = 0
            for key in due:
                if daemon == "down":
                    found, note = self.read(key, ended_only=True)
                    lines += found
                    if note:
                        notes.append(f"{key[0]}: validation run {key[2]} {note}")
                    if self.runs[key]["final"]:
                        continue
                lines += self.stop(key)
                unverified += 1
                if daemon == "down" and not self.runs[key]["down"]:
                    lines.append(self.event(key, "daemon_down", self.clock()))
                    self.runs[key]["down"] = True
            if unverified:
                notes.append(f"no-mistakes daemon is down: {unverified} validation run(s) unverified"
                             if daemon == "down" else "no-mistakes daemon did not answer; validation unverified")
            return lines, notes
        for key in due:
            found, note = self.read(key)
            lines += found
            if note:
                notes.append(f"{key[0]}: validation run {key[2]} {note}")
        return lines, notes

    def read(self, key, ended_only=False):
        """Lines for one run's read, and a diagnostic when it failed. With
        `ended_only` (the daemon is down) only a run that ended is taken: a
        record still saying running may be a dead daemon's."""
        state = self.runs[key]
        try:
            read, ages = read_run(self.reader.status(key[2]), key[2])
        except Unreadable as error:
            lines = self.stop(key)
            if str(error) != f'run "{key[2]}" not found':
                return lines, f"unreadable: {error}"
            state["final"] = True
            return lines + [self.event(key, "gone", self.clock(), ident="gone")], \
                "is gone from no-mistakes; its last read stays"
        except Drift as error:
            return self.stop(key), f"output not understood ({error}); unverified until it reads again"
        except (OSError, TimeoutError) as error:
            return self.stop(key), f"unreadable: {getattr(error, 'strerror', None) or error}"
        if ended_only and read["status"] not in RUN_ENDED:
            return self.stop(key), None
        now = self.clock()
        lines = []
        if read != state["state"] or state["down"]:
            before = state["state"] or {}
            if read["status"] in RUN_ENDED and before.get("status") not in RUN_ENDED:
                phase = "ended"
            elif read.get("gate") and read["gate"] != before.get("gate"):
                phase = "parked"
            else:
                phase = "seen"
            payload = copy.deepcopy(read)
            for step in payload["steps"]:
                if step["step"] in ages:
                    step["round_since"] = iso(now - ages[step["step"]])
            if state["read"] is not None:
                payload["since"] = iso(state["read"])
            digest = hashlib.sha256(json.dumps(read, sort_keys=True).encode()).hexdigest()[:16]
            lines.append(self.event(key, phase, now, ident=f"{phase}/{now}/{digest}", **payload))
            state.update(state=read, down=False)
        state["read"] = now
        lines = self.cover(key, now, bool(lines)) + lines
        if read["status"] in RUN_ENDED:
            state["final"] = True
            lines += self.stop(key)
        return lines, None

    def cover(self, key, now, due=False):
        """Extend the run's open coverage window to `now`, as Bridge.cover."""
        state, lines = self.runs[key], []
        if state["window"] and now - state["window"][1] > self.max_gap:
            lines += self.stop(key)
        if not state["window"]:
            state["window"] = [now, now]
            due = True
        state["window"][1] = now
        if due or now - state["written"] >= self.checkpoint:
            lines.append(self.segment(key))
        return lines

    def segment(self, key):
        state = self.runs[key]
        start = state["window"][0] if state["written"] is None else state["written"]
        state["written"] = state["window"][1]
        return self.line(key, "validation_coverage", f"coverage/{self.run}/{start}-{state['written']}",
                         state["written"], "observed",
                         {"run": key[2], "from": iso(start), "to": iso(state["written"]),
                          "max_gap": math.ceil(self.max_gap)})

    def stop(self, key):
        """End the run's coverage at its last read: a read failed, the run
        ended, or the adapter stops."""
        state, lines = self.runs[key], []
        if state["window"] and state["written"] != state["window"][1]:
            lines.append(self.segment(key))
        state["window"] = state["written"] = None
        return lines

    def close(self):
        return [line for key in list(self.runs) for line in self.stop(key)]


def lifecycle(snapshot, manifest, bridge, feeds, validation=None):
    """One poll's lifecycle lines. The feeds are read first, so the bridge
    knows what they already hold; their diagnostics join the manifest's, as
    do validation's."""
    found, notes = feeds.poll(snapshot)
    manifest["diagnostics"].extend(notes)
    for line in found:
        bridge.reach.add(line["event"])
    lines = found + bridge.observe(snapshot, manifest)
    if validation is not None:
        read, notes = validation.observe(snapshot)
        manifest["diagnostics"].extend(notes)
        lines += read
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


def write_atomic(path, text):
    fd, name = tempfile.mkstemp(prefix=".fleet-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def publish(output, manifest, previous, lifecycle=()):
    records = list(lifecycle)
    if previous is None or semantic(previous) != semantic(manifest):
        records.append({"schema": JOURNAL, "kind": "manifest", "manifest": manifest})
    append_journal(output, records)
    write_atomic(output, json.dumps(manifest, indent=2) + "\n")


def take_lock(output):
    """The collector's lock on this fleet, or None while another process has it."""
    handle = output.with_suffix(".lock").open("a")
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        handle.close()
        return None
    return handle


def backup_dir(output, name=None):
    """A new backup-<local time>[-name] directory beside the manifest."""
    stamp = time.strftime("%Y%m%d-%H%M%S")
    if name:
        stamp += "-" + (re.sub(r"[^A-Za-z0-9._-]+", "-", name).strip("-.")[:40] or "worker")
    candidate, n = output.parent / f"backup-{stamp}", 1
    while True:
        try:
            candidate.mkdir(mode=0o700)
            return candidate
        except FileExistsError:
            n += 1
            candidate = output.parent / f"backup-{stamp}-{n}"


def clear(output, action):
    """Archive or delete the whole fleet: the manifest and its journal, which
    holds every lifecycle record and manifest checkpoint. Archive moves them
    into a new backup directory; delete unlinks them. Either way the next
    collection starts empty. The caller holds the lock. Returns what happened."""
    files = [p for p in (output, output.with_suffix(".events.jsonl")) if p.exists()]
    if not files:
        return "nothing to clear"
    names = " and ".join(p.name for p in files)
    if action == "archive":
        backup = backup_dir(output)
        for path in files:
            os.replace(path, backup / path.name)
        return f"archived {names} to {backup}"
    for path in files:
        path.unlink()
    return f"deleted {names} permanently"


def attempt_of(event):
    attempt = event.get("attempt")
    return (attempt.get("task"), attempt.get("spawn_gen")) if isinstance(attempt, dict) else None


def split(manifest, session, attempts):
    """A manifest without one worker, and the worker's own part of it."""
    own = lambda task: (task["id"], task["spawn_gen"]) in attempts
    sessions, tasks = manifest.get("sessions", []), manifest.get("tasks", [])
    kept = dict(manifest, sessions=[s for s in sessions if s.get("key") != session],
                tasks=[t for t in tasks if not own(t)],
                links=[l for l in manifest.get("links", []) if session not in (l.get("from"), l.get("to"))])
    removed = dict(manifest, sessions=[s for s in sessions if session and s.get("key") == session],
                   tasks=[t for t in tasks if own(t)], links=[], diagnostics=[])
    return kept, removed


class Worker:
    """One worker's records: a member session with every attempt joined to it
    (by the manifest or a journal `bound`), or one attempt without a session.
    Removal is scoped by those attempts, so it never takes another's records."""

    def __init__(self, output, session=None, attempt=None):
        self.output = output
        self.manifest = recover(output) or {"sessions": [], "tasks": [], "links": []}
        self.records = list(journal_records(output))
        self.session = session
        self.attempts = {attempt} if attempt else set()
        if session:
            for task in self.manifest["tasks"]:
                if task.get("session") == session:
                    self.attempts.add((task["id"], task["spawn_gen"]))
            for record in self.records:
                event = record.get("event") or {}
                if (record.get("kind") == "lifecycle" and event.get("type") == "bound"
                        and event.get("session") == session and attempt_of(event)):
                    self.attempts.add(attempt_of(event))
        member = [s for s in self.manifest["sessions"] if session and s["key"] == session]
        if not member and not self.attempts:
            raise ValueError("no such worker in this fleet")
        self.label = member[0]["label"] if member else attempt[0]

    @classmethod
    def named(cls, output, name):
        """By native session ID, task ID (its newest attempt) or task/spawn_gen."""
        manifest = recover(output) or {"sessions": [], "tasks": []}
        for spec in manifest["sessions"]:
            if spec["key"]["session_id"] == name:
                return cls(output, session=spec["key"])
        tasks = [t for t in manifest["tasks"]
                 if t["id"] == name or f"{t['id']}/{t['spawn_gen']}" == name]
        if not tasks:
            raise ValueError(f"no worker named {name!r} in this fleet")
        task = tasks[-1]
        if task.get("session"):
            return cls(output, session=task["session"])
        return cls(output, attempt=(task["id"], task["spawn_gen"]))

    def registered(self):
        """Whether the adapter, when it last observed, still registered it. A
        session with no attempt (a Captain) counts as registered until a
        later Captain takes its place."""
        if self.session and not self.attempts:
            spec = next((s for s in self.manifest["sessions"] if s["key"] == self.session), {})
            return (spec.get("runtime") or {}).get("value") != NOT_OBSERVED
        tasks = {(t["id"], t["spawn_gen"]): t for t in self.manifest["tasks"]}
        return any(key in tasks and (tasks[key].get("runtime") or {}).get("value") != NOT_OBSERVED
                   for key in self.attempts)

    def lifecycle(self):
        return sum(1 for r in self.records if r.get("kind") == "lifecycle"
                   and attempt_of(r.get("event") or {}) in self.attempts)

    def describe(self, action):
        stake = (f"{self.label} is STILL REGISTERED and will reappear on the next collection. "
                 if self.registered() else "")
        what = (f"its manifest entry, {self.lifecycle()} lifecycle records and its part of "
                "every checkpoint, from the live journal. Its transcript stays.")
        if action == "archive":
            return f"Archive {self.label}: {what}"
        return f"{stake}DELETE {self.label}: {what} This cannot be undone."

    def remove(self, action, confirmed_registered=False):
        """Take this worker's records out of the live manifest and journal.
        Archive writes them to a backup directory, as a manifest and journal of
        their own; delete drops them. The caller holds the lock."""
        if self.registered():
            if action == "archive":
                raise ValueError(f"{self.label} is still registered: archive it once it finishes")
            if not confirmed_registered:
                raise ValueError(f"{self.label} is still registered: deleting it needs confirmation")
        journal = self.output.with_suffix(".events.jsonl")
        kept, removed, count = [], [], 0
        text = journal.read_text() if journal.exists() else ""
        for line in text.splitlines(keepends=True):
            if not line.endswith("\n"):
                continue  # An interrupted final append; the next one drops it too.
            try:
                record = json.loads(line)
            except ValueError:
                kept.append(line)
                continue
            if not isinstance(record, dict):
                kept.append(line)
            elif record.get("kind") == "lifecycle" and isinstance(record.get("event"), dict):
                mine = attempt_of(record["event"]) in self.attempts
                (removed if mine else kept).append(line)
                count += mine
            elif isinstance(record.get("manifest"), dict):
                mine, theirs = split(record["manifest"], self.session, self.attempts)
                kept.append(json.dumps(dict(record, manifest=mine)) + "\n")
                if theirs["sessions"] or theirs["tasks"]:
                    removed.append(json.dumps(dict(record, manifest=theirs)) + "\n")
            else:
                kept.append(line)
        manifest, own = split(self.manifest, self.session, self.attempts)
        if action == "archive":
            backup = backup_dir(self.output, self.label)
            write_atomic(backup / journal.name, "".join(removed))
            if own.get("schema"):
                write_atomic(backup / self.output.name, json.dumps(own, indent=2) + "\n")
            outcome = f"archived {self.label} to {backup}"
        else:
            outcome = f"deleted {self.label} permanently"
        if journal.exists():
            write_atomic(journal, "".join(kept))
        if self.output.exists() and manifest.get("schema"):
            write_atomic(self.output, json.dumps(manifest, indent=2) + "\n")
        return f"{outcome} ({count} lifecycle records); its transcript is untouched"


def carry_out(output, path):
    """Do what the viewer asked before it exited; a note for the next viewer."""
    try:
        request = json.loads(path.read_text())
    except (OSError, ValueError):
        return "the fleet request could not be read; nothing changed"
    finally:
        path.unlink(missing_ok=True)
    action = request.get("action") if isinstance(request, dict) else None
    if action not in ("archive", "delete"):
        return "unknown fleet request; nothing changed"
    try:
        if request.get("scope") == "all":
            return clear(output, action)
        attempt = request.get("attempt")
        attempt = (attempt["task"], attempt["spawn_gen"]) if isinstance(attempt, dict) else None
        worker = Worker(output, session=request.get("session"), attempt=attempt)
        return worker.remove(action, confirmed_registered=request.get("confirmed_registered") is True)
    except (OSError, ValueError, KeyError, TypeError) as error:
        return f"{action} refused: {error}"


def confirm(text):
    print(text)
    try:
        return input("Type y to confirm: ").strip() == "y"
    except EOFError:
        return False


def manage(args, parser):
    """--archive / --delete without a collector: the same actions as its viewer."""
    action = "archive" if args.archive else "delete"
    output = args.output.absolute()
    if not output.parent.is_dir():
        print("nothing to clear")
        return 0
    os.umask(0o077)
    lock = take_lock(output)
    if lock is None:
        parser.error("a collector owns this fleet: use its viewer (C, A, D) or close it first")
    try:
        if args.worker:
            worker = Worker.named(output, args.worker)
            if action == "delete" and not confirm(worker.describe(action)):
                print("cancelled; nothing changed")
                return 1
            print(worker.remove(action, confirmed_registered=True))
        else:
            if action == "delete" and not confirm(
                    f"DELETE {output.name} and {output.with_suffix('.events.jsonl').name} in "
                    f"{output.parent}: every member, lifecycle record and checkpoint. Backups and "
                    "transcripts stay. This cannot be undone."):
                print("cancelled; nothing changed")
                return 1
            print(clear(output, action))
    except (OSError, ValueError, KeyError) as error:
        print(error, file=sys.stderr)
        return 1
    finally:
        lock.close()
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, help="Firstmate home (required to collect)")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--captain", help="explicit Herdr session:pane target")
    parser.add_argument("--watch", action="store_true")
    parser.add_argument("--interval", type=float, default=5)
    parser.add_argument("--view", type=Path, help="Fleet-capable binary to run after initial collection")
    removal = parser.add_mutually_exclusive_group()
    removal.add_argument("--archive", action="store_true",
                         help="move the records into a backup-<time> directory beside them and exit")
    removal.add_argument("--delete", action="store_true",
                         help="delete the records permanently, after a confirmation, and exit")
    parser.add_argument("--worker", help="with --archive/--delete: only this worker, by native "
                        "session ID, task ID or task/spawn_gen")
    args = parser.parse_args()
    if args.worker and not (args.archive or args.delete):
        parser.error("--worker needs --archive or --delete")
    if args.archive or args.delete:
        # Only files change here, so this works outside Herdr too.
        return manage(args, parser)
    if args.home is None:
        parser.error("--home is required to collect")
    if os.environ.get("HERDR_ENV") != "1":
        parser.error("run the Firstmate adapter inside a genuine Herdr pane (HERDR_ENV=1)")
    if args.interval < 1:
        parser.error("interval must be at least one second")
    home = args.home.resolve(strict=True)
    output = args.output.absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    os.umask(0o077)
    lock = take_lock(output)
    if lock is None:
        parser.error("a collector already owns this fleet; open its existing manifest")
    previous = recover(output)
    bridge = feeds = validation = None
    viewer = None
    request = output.with_suffix(".request.json")
    note = None
    try:
        while True:
            try:
                manifest, snapshot = collect(home, os.environ.get("HERDR_BIN_PATH", "herdr"),
                                             previous, args.captain)
                if bridge is None:
                    max_gap = max(60, 4 * args.interval)
                    run = f"{now()}/{os.getpid()}"
                    bridge = Bridge(manifest["fleet_id"], run, max_gap=max_gap)
                    bridge.recover(journal_records(output))
                    feeds = Feeds(output.with_suffix(".feeds.json"), max_gap=max_gap)
                    validation = Validation(manifest["fleet_id"], run, max_gap=max_gap)
                    validation.recover(journal_records(output))
                publish(output, manifest, previous,
                        lifecycle(snapshot, manifest, bridge, feeds, validation))
                # After the journal holds what was read: a crash or failed poll between
                # the two re-reads it, and the same IDs change nothing.
                feeds.save()
                previous = manifest
            except (OSError, RuntimeError, ValueError, KeyError) as error:
                if bridge:
                    try:
                        append_journal(output, bridge.close() + validation.close())
                    except OSError:
                        pass
                bridge = feeds = validation = None
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
                env = dict(os.environ)
                if args.watch:
                    # Only a collector that keeps running can carry out requests.
                    request.unlink(missing_ok=True)
                    env["ZOE_FLEET_REQUEST"] = str(request)
                env.pop("ZOE_FLEET_NOTICE", None)
                if note:
                    env["ZOE_FLEET_NOTICE"] = note
                    note = None
                viewer = subprocess.Popen([str(args.view), "fleet", str(output)], env=env)
            if not args.watch:
                return viewer.wait() if viewer else 0
            until = time.monotonic() + args.interval
            asked = False
            while time.monotonic() < until:
                if viewer and viewer.poll() is not None:
                    if viewer.returncode != REQUEST_EXIT:
                        return viewer.returncode
                    asked = True
                    break
                time.sleep(0.2)
            if asked:
                # The viewer asked to archive or delete, and exited: nothing
                # reads the records now. End this run's coverage in the journal
                # being changed, carry the request out, and forget what this
                # process held, so the next poll starts from what remains and
                # a new viewer opens on it. A worker still registered comes
                # back with that poll, as it should.
                if bridge:
                    append_journal(output, bridge.close() + feeds.close() + validation.close())
                    feeds.save()
                note = carry_out(output, request)
                print(f"{note}; collecting a fresh view...", flush=True)
                previous, bridge, viewer = recover(output), None, None
    except KeyboardInterrupt:
        return 0
    finally:
        if viewer and viewer.poll() is None:
            viewer.terminate()
            viewer.wait(timeout=5)
        if bridge:
            try:
                append_journal(output, bridge.close() + feeds.close() + validation.close())
                feeds.save()
            except OSError:
                pass
        lock.close()


if __name__ == "__main__":
    sys.exit(main())
