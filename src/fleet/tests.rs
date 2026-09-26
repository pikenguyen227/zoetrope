use super::*;
use crate::fact::{Fact, FactKind, Statement};

fn key(id: &str) -> SessionKey {
    SessionKey {
        provider: "codex".into(),
        session_id: id.into(),
    }
}

fn manifest() -> Manifest {
    Manifest::parse(
        r#"{
      "schema":"zoetrope.fleet.v1", "fleet_id":"test", "label":"Test fleet",
      "observed_at":"2026-09-22T00:00:00Z",
      "sessions":[
        {"key":{"provider":"codex","session_id":"captain"},"label":"Captain"},
        {"key":{"provider":"codex","session_id":"worker"},"label":"Worker"}
      ],
      "links":[{"id":"assignment", "from":{"provider":"codex","session_id":"captain"},
        "to":{"provider":"codex","session_id":"worker"}, "kind":"delegates",
        "evidence":"synthetic fixture assignment", "observed_at":"2026-09-22T00:00:00Z"}]
    }"#,
    )
    .unwrap()
}

fn activity(id: &str) -> UiEvent {
    let ts = Some("2026-09-22T00:00:01Z".parse().unwrap());
    UiEvent::Batch {
        session_id: id.into(),
        statements: vec![Statement {
            at: ts,
            facts: vec![
                Fact {
                    agent: Some("main".into()),
                    ts,
                    kind: FactKind::Tokens {
                        output: 100,
                        dedup: Some("same-token-key".into()),
                    },
                },
                Fact {
                    agent: Some("main".into()),
                    ts,
                    kind: FactKind::ToolStart {
                        call: "same-call".into(),
                        name: "Read".into(),
                        summary: None,
                    },
                },
                Fact {
                    agent: Some("child".into()),
                    ts,
                    kind: FactKind::Agent {
                        kind: AgentKind::Subagent,
                        parent: Some("main".into()),
                        agent_type: Some("Research".into()),
                        description: None,
                        spawned_by: None,
                        interactive: false,
                    },
                },
            ],
        }],
    }
}

#[test]
fn independent_roots_calls_tokens_and_children_do_not_collide() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    for id in ["captain", "worker"] {
        fleet.event(&key(id), activity(id));
    }
    fleet.sync();
    assert_eq!(fleet.overview.session.agent_count(), 5);
    assert_eq!(fleet.overview.session.tool_count(), 2);
    assert_eq!(fleet.overview.flow.edges().len(), 4);
    for id in ["captain", "worker"] {
        assert_eq!(
            fleet
                .overview
                .session
                .agent(&key(id).node_id("main"))
                .unwrap()
                .output_tokens,
            100
        );
        assert_eq!(
            fleet
                .overview
                .session
                .agent(&key(id).node_id("child"))
                .unwrap()
                .parent,
            Some(key(id).node_id("main"))
        );
    }
    // Repeated delivery doesn't create another native call or count tokens again.
    fleet.event(&key("worker"), activity("worker"));
    fleet.sync();
    assert_eq!(fleet.overview.session.tool_count(), 2);
    assert_eq!(
        fleet
            .overview
            .session
            .agent(&key("worker").node_id("main"))
            .unwrap()
            .output_tokens,
        100
    );
}

#[test]
fn source_reset_and_wrong_identity_never_clear_other_roots() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    for id in ["captain", "worker"] {
        fleet.event(&key(id), activity(id));
    }
    let worker = fleet.members[&key("worker")].app.session.clone();
    fleet.event(
        &key("captain"),
        UiEvent::SessionReset {
            session_id: "captain".into(),
        },
    );
    fleet.sync();
    assert!(fleet.members[&key("worker")].app.session == worker);
    assert!(
        fleet
            .overview
            .session
            .agent(&key("captain").node_id("child"))
            .is_none()
    );
    fleet.event(
        &key("worker"),
        UiEvent::SessionReset {
            session_id: "unrelated".into(),
        },
    );
    assert!(fleet.members[&key("worker")].app.session == worker);
    assert!(fleet.members[&key("worker")].error.is_some());
}

#[test]
fn validation_is_atomic_and_ids_are_unambiguous() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    let mut bad = manifest();
    bad.sessions.push(bad.sessions[0].clone());
    assert!(fleet.update(bad).is_err());
    assert_eq!(fleet.members.len(), 2);
    let mut bad = manifest();
    bad.links[0].evidence.clear();
    assert!(bad.validate().is_err());
    assert_ne!(key("a:b").node_id("c"), key("a").node_id("b:c"));
}

#[test]
fn manifest_refresh_preserves_selection_and_removed_sessions() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    fleet.event(&key("worker"), activity("worker"));
    fleet.sync();
    let id = key("worker").node_id("main");
    fleet.overview.flow.select_node(&id);
    let position = fleet.overview.flow.node(&id).unwrap().position;
    fleet.update(manifest()).unwrap();
    assert_eq!(
        fleet.overview.selected_agent_id().as_deref(),
        Some(id.as_str())
    );
    assert_eq!(fleet.overview.flow.node(&id).unwrap().position, position);
    let mut next = manifest();
    next.sessions.pop();
    next.links.clear();
    fleet.update(next).unwrap();
    assert!(fleet.members[&key("worker")].retained);
    assert_eq!(fleet.members[&key("worker")].app.session.tool_count(), 1);
}

#[test]
fn resume_reuses_node_continuation_is_an_explicit_new_node() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    fleet.update(manifest()).unwrap();
    assert_eq!(fleet.members.len(), 2);
    let mut next = manifest();
    next.sessions.push(SessionSpec {
        key: key("worker-next"),
        label: "Worker attempt 2".into(),
        file: None,
        runtime: None,
        herdr: None,
    });
    next.links.push(Link {
        id: "handoff".into(),
        from: key("worker"),
        to: key("worker-next"),
        kind: Relation::Continues,
        evidence: "explicit handoff fixture".into(),
        observed_at: next.observed_at,
    });
    fleet.update(next).unwrap();
    assert_eq!(fleet.members.len(), 3);
    assert!(
        fleet
            .overview
            .flow
            .edges()
            .iter()
            .any(|e| e.label.as_deref() == Some("continues"))
    );
}

#[test]
fn source_delivery_order_converges() {
    let mut a = Fleet::new(manifest()).unwrap();
    let mut b = Fleet::new(manifest()).unwrap();
    for id in ["captain", "worker"] {
        a.event(&key(id), activity(id));
    }
    for id in ["worker", "captain"] {
        b.event(&key(id), activity(id));
    }
    a.sync();
    b.sync();
    assert!(a.overview.session == b.overview.session);
}

#[cfg(feature = "native")]
#[test]
fn real_fleet_ui_renders_both_sessions() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut fleet = Fleet::new(manifest()).unwrap();
    for id in ["captain", "worker"] {
        fleet.event(&key(id), activity(id));
    }
    fleet.sync();
    fleet.overview.relayout_now();
    let mut terminal = Terminal::new(TestBackend::new(180, 55)).unwrap();
    terminal
        .draw(|frame| native::draw(frame, &mut fleet))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("Captain"));
    assert!(text.contains("Worker"));
    assert!(text.contains("delegates"));
}

/// Facts stamped at `at`, so wall-clock liveness reads them as current.
fn live(id: &str, at: DateTime<Utc>, facts: Vec<FactKind>, agent: &str) -> UiEvent {
    UiEvent::Batch {
        session_id: id.into(),
        statements: vec![Statement {
            at: Some(at),
            facts: facts
                .into_iter()
                .map(|kind| Fact {
                    agent: Some(agent.into()),
                    ts: Some(at),
                    kind,
                })
                .collect(),
        }],
    }
}

fn child(parent: &str) -> FactKind {
    FactKind::Agent {
        kind: AgentKind::Subagent,
        parent: Some(parent.into()),
        agent_type: Some("Research".into()),
        description: None,
        spawned_by: None,
        interactive: false,
    }
}

fn call(id: &str) -> [FactKind; 2] {
    [
        FactKind::ToolStart {
            call: id.into(),
            name: "Read".into(),
            summary: None,
        },
        FactKind::ToolEnd {
            call: id.into(),
            outcome: crate::fact::Outcome::Ok,
        },
    ]
}

fn draw(fleet: &mut Fleet) -> ratatui::Terminal<ratatui::backend::TestBackend> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 48)).unwrap();
    terminal.draw(|frame| native::draw(frame, fleet)).unwrap();
    terminal
}

#[test]
fn overview_camera_reframes_as_the_crew_grows() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    draw(&mut fleet);
    let now = Utc::now();
    for id in ["captain", "worker"] {
        fleet.event(&key(id), live(id, now, vec![FactKind::Activity], "main"));
    }
    fleet.sync();
    draw(&mut fleet);
    // A fan of subagents lands below one worker, wider than the first frame.
    for n in 0..8 {
        fleet.event(
            &key("worker"),
            live("worker", now, vec![child("main")], &format!("sub{n}")),
        );
    }
    fleet.sync();
    let terminal = draw(&mut fleet);
    let area = terminal.backend().buffer().area;
    for node in fleet.overview.flow.nodes() {
        let (l, t, r, b) = fleet.overview.flow.node_terminal_rect(&node.id).unwrap();
        assert!(
            l >= 0 && t >= 0 && r <= area.width as i32 && b <= area.height as i32,
            "{} is off screen at ({l},{t})-({r},{b})",
            node.id
        );
    }
}

#[test]
fn overview_follow_retargets_to_the_latest_activity() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    let now = Utc::now();
    fleet.event(
        &key("captain"),
        live("captain", now, vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    draw(&mut fleet);
    fleet.overview.camera = Camera::Follow;
    fleet.overview.track_activity();
    assert_eq!(
        fleet.overview.selected_agent_id(),
        Some(key("captain").node_id("main"))
    );
    let later = now + chrono::Duration::seconds(1);
    fleet.event(
        &key("worker"),
        live("worker", later, vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    assert_eq!(
        fleet.overview.selected_agent_id(),
        Some(key("worker").node_id("main"))
    );
}

#[test]
fn member_edges_and_crew_root_follow_member_liveness() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    let edge = |fleet: &Fleet| {
        // The captain is a member root; the worker hangs off its link instead.
        let id = format!("member:{}", key("captain").node_id(MAIN_ID));
        fleet
            .overview
            .flow
            .edges()
            .iter()
            .find(|e| e.id == id)
            .map(|e| (e.animated, e.content.running))
            .unwrap()
    };
    let root = |fleet: &Fleet| fleet.overview.session.agent(FLEET_ROOT).unwrap().status;
    // Hours-old activity reads idle.
    let old = Utc::now() - chrono::Duration::hours(3);
    fleet.event(
        &key("captain"),
        live("captain", old, vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    assert_eq!(edge(&fleet), (false, false));
    assert_eq!(root(&fleet), AgentStatus::Idle);
    fleet.event(
        &key("captain"),
        live("captain", Utc::now(), vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    assert_eq!(edge(&fleet), (true, true));
    assert_eq!(root(&fleet), AgentStatus::Running);
}

#[test]
fn backfill_is_history_but_later_calls_chip() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    let now = Utc::now();
    fleet.event(
        &key("worker"),
        live("worker", now, [call("a"), call("b")].concat(), "main"),
    );
    fleet.sync();
    let tick = std::time::Duration::from_millis(16);
    fleet
        .overview
        .chips
        .reconcile(tick, true, &fleet.overview.session);
    assert_eq!(
        fleet.overview.chips.len(),
        0,
        "attach backfill must not chip"
    );
    fleet.event(
        &key("worker"),
        live("worker", now, call("c").into(), "main"),
    );
    fleet.sync();
    fleet
        .overview
        .chips
        .reconcile(tick, true, &fleet.overview.session);
    assert_eq!(fleet.overview.chips.len(), 1, "fresh activity chips");
    // At the live edge, pending durations tick against the wall clock.
    assert!(fleet.overview.wall_clock);
    assert!(fleet.overview.chrome_now().is_some());

    // What the worker did while unfocused is history once you open it.
    fleet.event(
        &key("worker"),
        live("worker", now, call("d").into(), "main"),
    );
    fleet.sync();
    fleet
        .overview
        .flow
        .select_node(&key("worker").node_id(MAIN_ID));
    fleet.inspect_selected();
    let app = fleet.active();
    app.chips.reconcile(tick, true, &app.session);
    assert_eq!(app.chips.len(), 0, "focus must not replay a burst");
}

#[test]
fn returning_from_focus_does_not_replay_a_burst() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    let now = Utc::now();
    let tick = std::time::Duration::from_millis(16);
    fleet.event(
        &key("worker"),
        live("worker", now, call("a").into(), "main"),
    );
    fleet.sync();
    fleet
        .overview
        .chips
        .reconcile(tick, true, &fleet.overview.session);
    fleet
        .overview
        .flow
        .select_node(&key("worker").node_id(MAIN_ID));
    fleet.inspect_selected();

    // Activity while focused: the overview tray is not reconciled.
    fleet.event(
        &key("worker"),
        live("worker", now, [call("b"), call("c")].concat(), "main"),
    );
    fleet.sync();
    fleet.back();
    fleet
        .overview
        .chips
        .reconcile(tick, true, &fleet.overview.session);
    assert_eq!(
        fleet.overview.chips.len(),
        0,
        "back must not replay a burst"
    );

    // Activity after returning still chips.
    fleet.event(
        &key("worker"),
        live("worker", now, call("d").into(), "main"),
    );
    fleet.sync();
    fleet
        .overview
        .chips
        .reconcile(tick, true, &fleet.overview.session);
    assert_eq!(fleet.overview.chips.len(), 1, "fresh activity chips");
}

#[test]
fn running_cards_pulse_on_the_app_clock() {
    let mut fleet = Fleet::new(manifest()).unwrap();
    fleet.event(
        &key("worker"),
        live("worker", Utc::now(), vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    let worker = key("worker").node_id(MAIN_ID);
    let glyphs = |fleet: &mut Fleet| {
        let terminal = draw(fleet);
        let buf = terminal.backend().buffer();
        (
            buf.content.iter().filter(|c| c.symbol() == "○").count(),
            buf.content.iter().filter(|c| c.symbol() == "●").count(),
        )
    };
    fleet.overview.relayout_now();
    let (off, _) = glyphs(&mut fleet);
    assert_eq!(off, 0);
    let half = std::time::Duration::from_millis(480);
    fleet.overview.tick_pulse(half);
    assert!(fleet.overview.flow.node_content_mut(&worker).unwrap().pulse);
    let (off, _) = glyphs(&mut fleet);
    assert!(off >= 1, "a running card shows its off beat");
    // A content rebuild keeps the beat instead of snapping it back.
    fleet.event(
        &key("worker"),
        live("worker", Utc::now(), call("x").into(), "main"),
    );
    fleet.sync();
    assert!(fleet.overview.flow.node_content_mut(&worker).unwrap().pulse);
    fleet.overview.tick_pulse(half);
    let (off, _) = glyphs(&mut fleet);
    assert_eq!(off, 0);
}

#[test]
fn relaunched_member_keeps_its_root_edge() {
    // A secondmate relaunched once: its first launch generation is no longer
    // observed, and the adapter records the new session continuing it.
    let manifest = Manifest::parse(
        r#"{
      "schema":"zoetrope.fleet.v1", "fleet_id":"test", "label":"Test fleet",
      "observed_at":"2026-09-22T00:00:00Z",
      "sessions":[
        {"key":{"provider":"codex","session_id":"mate-1"},"label":"Mate"},
        {"key":{"provider":"codex","session_id":"mate-2"},"label":"Mate"}
      ],
      "tasks":[
        {"id":"mate","spawn_gen":"1","label":"Mate",
          "session":{"provider":"codex","session_id":"mate-1"},
          "state":{"value":"working","source":"fixture","observed_at":"2026-09-22T00:00:00Z"},
          "runtime":{"value":"not observed","source":"fixture","observed_at":"2026-09-22T00:00:00Z"}},
        {"id":"mate","spawn_gen":"2","label":"Mate",
          "session":{"provider":"codex","session_id":"mate-2"},
          "state":{"value":"working","source":"fixture","observed_at":"2026-09-22T00:00:00Z"},
          "runtime":{"value":"working","source":"fixture","observed_at":"2026-09-22T00:00:00Z"}}
      ],
      "links":[{"id":"relaunch", "from":{"provider":"codex","session_id":"mate-1"},
        "to":{"provider":"codex","session_id":"mate-2"}, "kind":"continues",
        "evidence":"launch generation 1 -> 2", "observed_at":"2026-09-22T00:00:00Z"}]
    }"#,
    )
    .unwrap();
    let mut fleet = Fleet::new(manifest).unwrap();
    for id in ["mate-1", "mate-2"] {
        fleet.event(&key(id), activity(id));
    }
    let current = key("mate-2").node_id(MAIN_ID);
    let member = |fleet: &Fleet| {
        fleet.overview.flow.edges().iter().any(|e| {
            e.id == format!("member:{current}") && e.source == FLEET_ROOT && e.target == current
        })
    };
    fleet.sync();
    assert!(!fleet.show_finished);
    assert_eq!(fleet.finished, 1, "the first launch has finished");
    assert!(
        member(&fleet),
        "hidden predecessor: current session is on the root"
    );
    fleet.toggle_finished();
    assert!(fleet.show_finished);
    assert!(
        member(&fleet),
        "shown predecessor: current session is on the root"
    );
    // The succession itself is still drawn, as history.
    assert!(
        fleet
            .overview
            .flow
            .edges()
            .iter()
            .any(|e| e.label.as_deref() == Some("continues") && e.target == current)
    );
}

#[test]
fn a_secondmates_crew_hangs_under_it_while_it_is_shown() {
    // A secondmate's own worker, delegated by it in its Firstmate home. The
    // secondmate's launch is no longer observed, so it is finished.
    let manifest = Manifest::parse(
        r#"{
      "schema":"zoetrope.fleet.v1", "fleet_id":"test", "label":"Test fleet",
      "observed_at":"2026-09-22T00:00:00Z",
      "sessions":[
        {"key":{"provider":"codex","session_id":"mate"},"label":"uiux"},
        {"key":{"provider":"codex","session_id":"crew"},"label":"acceptance-docs-typo"}
      ],
      "tasks":[
        {"id":"uiux","spawn_gen":"1","label":"uiux",
          "session":{"provider":"codex","session_id":"mate"},
          "state":{"value":"working","source":"fixture","observed_at":"2026-09-22T00:00:00Z"},
          "runtime":{"value":"not observed","source":"fixture","observed_at":"2026-09-22T00:00:00Z"}},
        {"id":"acceptance-docs-typo","spawn_gen":"1","label":"acceptance-docs-typo",
          "session":{"provider":"codex","session_id":"crew"},
          "state":{"value":"working","source":"fixture","observed_at":"2026-09-22T00:00:00Z"},
          "runtime":{"value":"working","source":"fixture","observed_at":"2026-09-22T00:00:00Z"}}
      ],
      "links":[{"id":"crew", "from":{"provider":"codex","session_id":"mate"},
        "to":{"provider":"codex","session_id":"crew"}, "kind":"delegates",
        "evidence":"secondmate uiux's own task", "observed_at":"2026-09-22T00:00:00Z"}]
    }"#,
    )
    .unwrap();
    let mut fleet = Fleet::new(manifest).unwrap();
    for id in ["mate", "crew"] {
        fleet.event(&key(id), activity(id));
    }
    let (mate, crew) = (key("mate").node_id(MAIN_ID), key("crew").node_id(MAIN_ID));
    let edge =
        |fleet: &Fleet, source: &str, label: &str| {
            fleet.overview.flow.edges().iter().any(|e| {
                e.source == source && e.target == crew && e.label.as_deref() == Some(label)
            })
        };
    fleet.sync();
    assert_eq!(fleet.finished, 1, "the secondmate has finished");
    assert!(
        edge(&fleet, FLEET_ROOT, "member"),
        "hidden secondmate: its crew is on the root"
    );
    fleet.toggle_finished();
    assert!(edge(&fleet, &mate, "delegates"), "crew hangs under it");
    assert!(
        !edge(&fleet, FLEET_ROOT, "member"),
        "shown secondmate: its crew is off the root"
    );
}

#[test]
fn a_member_herdr_saw_stop_reads_idle_at_once() {
    let now = Utc::now();
    let recent = now - chrono::Duration::seconds(5);
    // The worker's pane as Herdr last read it, or no runtime at all.
    let herdr = |value: Option<&str>, at: DateTime<Utc>| {
        let mut manifest = manifest();
        manifest.tasks.push(Task {
            id: "worker".into(),
            spawn_gen: "1".into(),
            label: "Worker".into(),
            project: None,
            session: Some(key("worker")),
            state: Observation {
                value: "working".into(),
                source: "fixture".into(),
                observed_at: at,
            },
            runtime: value.map(|value| Observation {
                value: value.into(),
                source: HERDR_PANE.into(),
                observed_at: at,
            }),
            depends_on: vec![],
        });
        manifest
    };
    let worker = |fleet: &Fleet| {
        let id = key("worker").node_id(MAIN_ID);
        let status = fleet.overview.session.agent(&id).unwrap().status;
        let edge = fleet.overview.flow.edges().iter().find(|e| e.target == id);
        (status, edge.unwrap().animated)
    };
    let mut fleet = Fleet::new(herdr(None, now)).unwrap();
    fleet.event(
        &key("worker"),
        live("worker", recent, vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    assert_eq!(
        worker(&fleet),
        (AgentStatus::Running, true),
        "no runtime: a recent entry still reads active"
    );
    fleet.update(herdr(Some("working"), now)).unwrap();
    assert_eq!(worker(&fleet), (AgentStatus::Running, true));
    for stopped in ["done", "idle"] {
        fleet.update(herdr(Some(stopped), now)).unwrap();
        assert_eq!(
            worker(&fleet),
            (AgentStatus::Idle, false),
            "Herdr read {stopped}: idle within the recency window"
        );
    }
    // Anything it records after Herdr saw it stop is recency's again.
    let later = now + chrono::Duration::seconds(1);
    fleet.event(
        &key("worker"),
        live("worker", later, vec![FactKind::Activity], "main"),
    );
    fleet.sync();
    assert_eq!(worker(&fleet), (AgentStatus::Running, true));
    fleet.update(herdr(None, now)).unwrap();
    assert_eq!(worker(&fleet), (AgentStatus::Running, true));
}

#[test]
fn a_member_herdr_sees_working_reads_active_however_quiet_its_transcript() {
    // Reproduction: a worker relaunched in its Herdr pane runs in a new
    // transcript while Herdr still registers the pane's first session, so the
    // one it is joined to has gone quiet past the recency window. Herdr reads
    // the pane working, and so must its card, at every level of the crew: a
    // primary worker, a secondmate and the secondmate's worker.
    let now = Utc::now();
    let quiet = now - chrono::Duration::seconds(600);
    let task = |id: &str, spawn: &str, runtime: Option<Observation>| Task {
        id: id.into(),
        spawn_gen: spawn.into(),
        label: id.into(),
        project: None,
        session: Some(key(id)),
        state: Observation {
            value: "working".into(),
            source: "status-log".into(),
            observed_at: quiet,
        },
        runtime,
        depends_on: vec![],
    };
    let herdr = |value: &str, at: DateTime<Utc>| {
        Some(Observation {
            value: value.into(),
            source: HERDR_PANE.into(),
            observed_at: at,
        })
    };
    let members = ["worker", "mate", "crew"];
    // Every member's pane as Herdr last read it, or no runtime at all.
    let crew = |value: Option<&str>, at: DateTime<Utc>| {
        let mut manifest = manifest();
        for id in ["mate", "crew"] {
            manifest.sessions.push(SessionSpec {
                key: key(id),
                label: id.into(),
                file: None,
                runtime: None,
                herdr: None,
            });
        }
        manifest.links.push(Link {
            id: "own".into(),
            from: key("mate"),
            to: key("crew"),
            kind: Relation::Delegates,
            evidence: "secondmate mate's own task".into(),
            observed_at: quiet,
        });
        // The worker's earlier launch, joined to the same session and left
        // behind by the relaunch: not Herdr's reading, so it never counts.
        let mut earlier = task(
            "crew",
            "0",
            Some(Observation {
                value: "not observed".into(),
                source: "firstmate.snapshot".into(),
                observed_at: at,
            }),
        );
        earlier.state.observed_at = quiet - chrono::Duration::seconds(600);
        manifest.tasks.push(earlier);
        for id in members {
            manifest
                .tasks
                .push(task(id, "1", value.and_then(|v| herdr(v, at))));
        }
        manifest
    };
    let status = |fleet: &Fleet, id: &str| {
        let node = key(id).node_id(MAIN_ID);
        fleet.overview.session.agent(&node).unwrap().status
    };
    let mut fleet = Fleet::new(crew(None, now)).unwrap();
    for id in members {
        fleet.event(&key(id), live(id, quiet, vec![FactKind::Activity], "main"));
    }
    fleet.sync();
    for id in members {
        assert_eq!(
            status(&fleet, id),
            AgentStatus::Idle,
            "{id}: no runtime, quiet past the window: idle"
        );
    }
    fleet.update(crew(Some("working"), now)).unwrap();
    for id in members {
        assert_eq!(
            status(&fleet, id),
            AgentStatus::Running,
            "{id}: Herdr reads its pane working: active however quiet"
        );
    }
    let crew_edge = fleet
        .overview
        .flow
        .edges()
        .iter()
        .find(|e| e.target == key("crew").node_id(MAIN_ID))
        .unwrap();
    assert!(
        crew_edge.animated,
        "the secondmate's working worker animates"
    );
    // PR 23's settle still holds: Herdr seeing the pane stop idles it at once.
    fleet.update(crew(Some("done"), now)).unwrap();
    for id in members {
        assert_eq!(
            status(&fleet, id),
            AgentStatus::Idle,
            "{id}: Herdr read done"
        );
    }
    // A working reading is evidence for the window after it, as a transcript
    // line is: a collector that stopped observing does not pin a card active.
    fleet.update(crew(Some("working"), quiet)).unwrap();
    for id in members {
        assert_eq!(
            status(&fleet, id),
            AgentStatus::Idle,
            "{id}: Herdr last read working past the window: recency decides"
        );
    }
}

#[test]
fn a_member_herdr_sees_working_through_a_long_tool_call_in_an_unjoined_transcript() {
    // The reported case: a secondmate's worker 18m40s into a turn, inside a
    // shell command already running past 30s. The command's tool call is in
    // the pane's relaunched transcript, which the collector has not joined, so
    // it never reaches the viewer; the joined one ended its last call long
    // before the recency window. Only Herdr's runtime shows the member working.
    let now = Utc::now();
    let turn = now - chrono::Duration::seconds(18 * 60 + 40);
    let crew = |runtime: Option<&str>| {
        let mut manifest = manifest();
        for id in ["mate", "crew"] {
            manifest.sessions.push(SessionSpec {
                key: key(id),
                label: id.into(),
                file: None,
                runtime: None,
                herdr: None,
            });
        }
        manifest.links.push(Link {
            id: "own".into(),
            from: key("mate"),
            to: key("crew"),
            kind: Relation::Delegates,
            evidence: "secondmate mate's own task".into(),
            observed_at: turn,
        });
        manifest.tasks.push(Task {
            id: "crew".into(),
            spawn_gen: "1".into(),
            label: "crew".into(),
            project: None,
            session: Some(key("crew")),
            state: Observation {
                value: "working".into(),
                source: "status-log".into(),
                observed_at: turn,
            },
            runtime: runtime.map(|value| Observation {
                value: value.into(),
                source: HERDR_PANE.into(),
                observed_at: now,
            }),
            depends_on: vec![],
        });
        manifest
    };
    let crew_main = key("crew").node_id(MAIN_ID);
    let crew_card = |fleet: &Fleet| {
        let status = fleet.overview.session.agent(&crew_main).unwrap().status;
        let edge = fleet
            .overview
            .flow
            .edges()
            .iter()
            .find(|e| e.target == crew_main);
        (status, edge.unwrap().animated)
    };
    let mut fleet = Fleet::new(crew(None)).unwrap();
    fleet.event(
        &key("crew"),
        live("crew", turn, call("read").to_vec(), "main"),
    );
    fleet.sync();
    assert_eq!(
        crew_card(&fleet),
        (AgentStatus::Idle, false),
        "no runtime: the joined transcript is quiet and holds no pending call"
    );
    fleet.update(crew(Some("working"))).unwrap();
    assert_eq!(
        crew_card(&fleet),
        (AgentStatus::Running, true),
        "Herdr reads the pane working mid-command: active"
    );
}
