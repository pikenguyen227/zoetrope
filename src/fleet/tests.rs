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
