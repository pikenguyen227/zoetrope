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
