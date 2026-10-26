# Fleet tower acceptance — Phase A

Input: local debug build and checked-out runtime; no uploaded release assets were used.

Firstmate pin: `3cebfb5e809f7e87cea296a59fe14e2f1e47b7b6`

Zoetrope binary: `zoe 0.2.0 (e790184dcfb4-dirty, built 2026-10-26 16:54:16 UTC)`
Isolation: `sandbox-exec -f scripts/tower-acceptance.sb`; fresh lab HOME, XDG dirs, TMPDIR and minimal PATH.

## Commands and results

1. Clone Firstmate at the pin; create empty `data/`, `state/`, `config/`, `projects/`.
2. `zoe-fleet-tower setup`, `doctor`, snapshot, `fm-home-seed.sh lab-mate --no-projects`, snapshot.
3. `zoe-fleet fleet assets/fleet/crew/fleet.json --inspect`.

Assertions passed: exact source pin; private config; fresh-home snapshot; lab-only registered mate; zero tasks; binary inspect; generated state has no `Desktop/Agentic`.

### Doctor

```text
Config: /private/tmp/zoe-tower-acceptance.faYMNW/home/.config/zoe-fleet/config
zoe 0.2.0 (e790184dcfb4-dirty, built 2026-10-26 16:54:16 UTC)
Python: compatible
Firstmate home: /private/tmp/zoe-tower-acceptance.faYMNW/firstmate
Herdr: found
Herdr plugin: not linked
no-mistakes: validation bands off
State writable: /private/tmp/zoe-tower-acceptance.faYMNW/home/.local/state/zoe-fleet/ff69e0b872b0
```

### Binary inspect

```text
{
  "build": {
    "built": "2026-10-26T16:54:16+00:00",
    "commit": "e790184dcfb4",
    "commit_unknown": null,
    "dirty": true,
    "version": "0.2.0"
  },
  "edges": 2,
  "fleet_id": "firstmate:/synthetic/firstmate",
  "links": [],
  "nodes": 3,
  "sessions": [
    {
      "agents": [
        {
          "activity": "idle",
          "id": "[\"codex\",\"c0c0c0c0-0000-4000-8000-000000000001\",\"main\"]",
          "native_id": "main",
          "output_tokens": 0,
          "parent": null,
          "tools": 6
        }
      ],
      "error": null,
      "key": {
        "provider": "codex",
        "session_id": "c0c0c0c0-0000-4000-8000-000000000001"
      },
      "label": "Captain · Codex",
      "loaded": true
    },
    {
      "agents": [
        {
          "activity": "idle",
          "id": "[\"codex\",\"c0c0c0c0-0000-4000-8000-000000000002\",\"main\"]",
          "native_id": "main",
          "output_tokens": 0,
          "parent": null,
          "tools": 8
        }
      ],
      "error": null,
      "key": {
        "provider": "codex",
        "session_id": "c0c0c0c0-0000-4000-8000-000000000002"
      },
      "label": "impl",
      "loaded": true
    },
    {
      "agents": [
        {
          "activity": "idle",
          "id": "[\"codex\",\"c0c0c0c0-0000-4000-8000-000000000003\",\"main\"]",
          "native_id": "main",
          "output_tokens": 0,
          "parent": null,
          "tools": 11
        }
      ],
      "error": null,
      "key": {
        "provider": "codex",
        "session_id": "c0c0c0c0-0000-4000-8000-000000000003"
      },
      "label": "tests",
      "loaded": true
    }
  ],
  "tasks": [
    {
      "depends_on": [],
      "id": "impl",
      "label": "impl",
      "project": "/synthetic/project",
      "runtime": {
        "observed_at": "2026-09-22T08:16:00Z",
        "source": "firstmate.snapshot",
        "value": "not observed"
      },
      "session": {
        "provider": "codex",
        "session_id": "c0c0c0c0-0000-4000-8000-000000000002"
      },
      "spawn_gen": "s1790064100.4101.1",
      "state": {
        "observed_at": "2026-09-22T08:16:00Z",
        "source": "firstmate.snapshot",
        "value": "unknown"
      }
    },
    {
      "depends_on": [],
      "id": "tests",
      "label": "tests",
      "project": "/synthetic/project",
      "runtime": {
        "observed_at": "2026-09-22T08:16:00Z",
        "source": "herdr.pane.get",
        "value": "working"
      },
      "session": {
        "provider": "codex",
        "session_id": "c0c0c0c0-0000-4000-8000-000000000003"
      },
      "spawn_gen": "s1790064400.4102.2",
      "state": {
        "observed_at": "2026-09-22T08:16:00Z",
        "source": "pane",
        "value": "working"
      }
    },
    {
      "depends_on": [],
      "id": "docs",
      "label": "docs",
      "project": "/synthetic/project",
      "runtime": {
        "observed_at": "2026-09-22T08:16:00Z",
        "source": "firstmate.snapshot",
        "value": "not observed"
      },
      "session": null,
      "spawn_gen": "s1790064660.4103.3",
      "state": {
        "observed_at": "2026-09-22T08:16:00Z",
        "source": "firstmate.snapshot",
        "value": "unknown"
      }
    }
  ]
}
```

### Snapshot assertions

```json
{
  "schema": "fm-fleet-snapshot.v1",
  "fm_home": "/private/tmp/zoe-tower-acceptance.faYMNW/firstmate",
  "tasks": [],
  "secondmate_current": {
    "registry": {
      "present": true,
      "available": true,
      "reason": null,
      "provenance": "registered-table",
      "path": "/private/tmp/zoe-tower-acceptance.faYMNW/firstmate/data/secondmates.md",
      "freshness": {
        "status": "fresh",
        "observed_at": "2026-10-26T16:55:33Z"
      },
      "records": [
        {
          "id": "lab-mate",
          "home": "/private/tmp/zoe-tower-acceptance.faYMNW/mate",
          "host": null,
          "root": null,
          "remote": false,
          "registered": true,
          "registry_error": null
        }
      ],
      "input_truncated": false,
      "records_truncated": false,
      "complete": true,
      "reasons": [],
      "lines_in_window": 1,
      "records_in_window": 1
    },
    "records": [
      {
        "id": "lab-mate",
        "home": "/private/tmp/zoe-tower-acceptance.faYMNW/mate",
        "host": null,
        "remote": false,
        "registered": true,
        "spawn_gen": null,
        "current": {
          "state": "unknown",
          "reason": "structured home ledger is missing, unreadable, or invalid"
        },
        "invalidity": null,
        "reconcile_inventory": null,
        "provenance": {
          "selected": "unknown",
          "structured_home": "/private/tmp/zoe-tower-acceptance.faYMNW/mate",
          "parent_event_role": "fallback-only-not-current"
        },
        "freshness": {
          "status": "unknown",
          "observed_at": "2026-10-26T16:55:33Z",
          "age_seconds": null
        },
        "active_children": [],
        "decisions_open": [],
        "holds": [],
        "queued": [],
        "landed": [],
        "endpoints": [],
        "counts": {
          "active_children": 0,
          "decisions_open": 0,
          "holds": 0,
          "queued": 0,
          "landed": 0,
          "endpoints": 0
        },
        "omitted": [],
        "parent_event": {
          "raw": "",
          "note": "",
          "age_seconds": null,
          "open_activities": [],
          "open_decisions": [],
          "activity_scan": {
            "records": [],
            "available": true,
            "input_truncated": false,
            "retained_truncated": false,
            "reasons": [],
            "lines_in_window": 0,
            "records_in_window": 0
          }
        },
        "terminal_evidence": {
          "provenance": "parent-direct-report-terminal",
          "trust": "untrusted-supplement",
          "captured": false,
          "observed_at": "2026-10-26T16:55:33Z",
          "freshness": "not-collected",
          "reason": "no parent event to compare",
          "lines": 0,
          "bytes": 0,
          "event_note_seen": false,
          "contradiction": false
        },
        "contradiction": false
      }
    ],
    "total_registered": 1,
    "total": 1,
    "shown": 1,
    "truncated": 0
  }
}
```

Phase B: not run; requires one person to launch a real Herdr and agent in the lab.
