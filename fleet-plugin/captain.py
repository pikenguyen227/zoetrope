"""Print the Herdr pane running this Firstmate home's coordinator, or nothing.

usage: captain.py HERDR FIRSTMATE_HOME [FOCUSED_PANE]

The Captain is chosen by what a pane RUNS, never by focus or tab label: a
pane whose registered Claude or Codex session was launched in the Firstmate
home, and which no Firstmate task names as its endpoint (every crew pane and
secondmate is one, and a crew pane starts in the home too). A plain shell has
no registered session, so the captain's own Shell tab is never chosen. The
focused pane wins only when it is itself such a coordinator. When none, or
several, qualify, or Herdr or Firstmate cannot answer, print nothing: the
collector then keeps its last-registered Captain rather than a guess.
"""
import json
import os
import subprocess
import sys


def run_json(argv, env=None):
    result = subprocess.run(argv, capture_output=True, text=True, timeout=30, env=env,
                            stdin=subprocess.DEVNULL)
    if result.returncode:
        raise RuntimeError(f"{argv[0]} exited {result.returncode}")
    return json.loads(result.stdout)


def real(path):
    return os.path.realpath(path) if isinstance(path, str) and path else None


def endpoints(home, herdr):
    """Pane IDs Firstmate's tasks run in. Firstmate reads Herdr from PATH."""
    env = dict(os.environ, FM_HOME=home)
    if os.sep in herdr:
        env["PATH"] = os.pathsep.join(filter(None, (os.path.dirname(os.path.abspath(herdr)),
                                                    env.get("PATH"))))
    snapshot = run_json([os.path.join(home, "bin", "fm-fleet-snapshot.sh"), "--json"], env)
    if real(snapshot.get("fm_home")) != real(home) or not isinstance(snapshot.get("tasks"), list):
        raise ValueError("snapshot is for another Firstmate home")
    panes = set()
    for task in snapshot["tasks"]:
        target = (task.get("endpoint") or {}).get("target") or ""
        # A `session:pane` target, where a pane ID itself holds one colon.
        panes.add(target.partition(":")[2] if target.count(":") == 2 else target)
    return panes


def coordinator(panes, home, crew, focused=None):
    """The one coordinator pane ID among Herdr's `panes`, else None."""
    home = real(home)
    found = []
    for pane in panes:
        session = pane.get("agent_session") or {}
        if (session.get("agent") in ("claude", "codex") and session.get("kind") == "id"
                and isinstance(session.get("value"), str) and session["value"].strip()
                and real(pane.get("cwd")) == home and pane.get("pane_id")
                and pane["pane_id"] not in crew):
            found.append(pane["pane_id"])
    if focused in found:
        return focused
    return found[0] if len(found) == 1 else None


def main(argv):
    herdr, home = argv[1], argv[2]
    focused = argv[3] if len(argv) > 3 else None
    try:
        panes = run_json([herdr, "pane", "list"])["result"]["panes"]
        crew = endpoints(home, herdr)
    except (OSError, RuntimeError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        return
    pane = coordinator(panes, home, crew, focused)
    if pane:
        print(pane)


if __name__ == "__main__":
    main(sys.argv)
