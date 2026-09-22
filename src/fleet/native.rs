//! Native manifest loader, isolated session watchers and Fleet terminal frontend.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc;

use super::{Fleet, Manifest, SessionKey, SessionSpec};
use crate::provider::{Provider, Session, Target};
use crate::tailer::{TailRequest, UiEvent};

const RETRY: Duration = Duration::from_secs(3);
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

pub fn load(path: &Path) -> Result<Manifest> {
    load_with_text(path).map(|(manifest, _)| manifest)
}

fn load_with_text(path: &Path) -> Result<(Manifest, String)> {
    if std::fs::metadata(path)?.len() > MAX_MANIFEST_BYTES {
        bail!("fleet manifest exceeds 4 MiB");
    }
    let text = std::fs::read_to_string(path)?;
    let mut manifest = Manifest::parse(&text).map_err(|e| anyhow!(e))?;
    let base = path.canonicalize()?.parent().unwrap().to_path_buf();
    for spec in &mut manifest.sessions {
        if let Some(file) = &mut spec.file
            && file.is_relative()
        {
            *file = base.join(&*file);
        }
    }
    Ok((manifest, text))
}

/// Resolve through the provider and then compare the complete identity. An ID
/// prefix, mismatched fixture or recycled pane must never attach another root.
pub fn resolve(spec: &SessionSpec) -> Result<Session> {
    let provider =
        Provider::parse(&spec.key.provider).ok_or_else(|| anyhow!("unknown provider"))?;
    let target = spec.file.as_ref().map_or_else(
        || Target::Id(spec.key.session_id.clone()),
        |p| Target::Path(p.clone()),
    );
    let session = crate::provider::open(&target, Some(provider))?;
    if session.id != spec.key.session_id || session.provider != provider {
        bail!(
            "resolved transcript identity differs from registered {:?}",
            spec.key
        );
    }
    std::fs::File::open(&session.root.path).context("opening root transcript")?;
    Ok(session)
}

/// Headless fixture/integration diagnostic. This never opens Herdr or launches
/// an agent. It reports errors per member and exits nonzero if any cannot load.
pub fn inspect(path: &Path) -> Result<()> {
    let manifest = load(path)?;
    let mut fleet = Fleet::new(manifest).map_err(|e| anyhow!(e))?;
    let specs = fleet.manifest.sessions.clone();
    let mut failed = false;
    for spec in specs {
        match resolve(&spec) {
            Ok(session) => {
                let (items, info, _) = crate::tailer::replay::build_replay(&session);
                fleet.event(
                    &spec.key,
                    UiEvent::ReplayLoaded {
                        session_id: session.id,
                        items,
                        info,
                        speed: 1.0,
                    },
                );
            }
            Err(error) => {
                failed = true;
                fleet.event(&spec.key, UiEvent::Error(error.to_string()));
            }
        }
    }
    fleet.sync();
    let members: Vec<_> = fleet
        .members
        .iter()
        .map(|(key, member)| {
            let agents: Vec<_> = member
                .app
                .session
                .spawn_order()
                .map(|id| {
                    let agent = member.app.session.agent(id).unwrap();
                    serde_json::json!({"id":key.node_id(id), "native_id":id,
                "parent":agent.parent.as_deref().map(|p| key.node_id(p)),
                "tools":agent.tool_calls().len(), "output_tokens":agent.output_tokens,
                "activity":agent.status_word()})
                })
                .collect();
            serde_json::json!({"key":key, "label":member.spec.label, "error":member.error,
            "loaded":member.loaded, "agents":agents})
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "fleet_id":fleet.manifest.fleet_id, "sessions":members,
            "tasks":fleet.manifest.tasks, "links":fleet.manifest.links,
            "nodes":fleet.overview.flow.nodes().count(), "edges":fleet.overview.flow.edges().len()
        }))?
    );
    if failed {
        bail!("one or more fleet transcripts are unavailable");
    }
    Ok(())
}

/// Cancelling a watcher also cancels its nested tailer, including on panic/quit.
struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Message {
    key: SessionKey,
    generation: u64,
    event: UiEvent,
}
struct Watcher {
    spec: SessionSpec,
    generation: u64,
    _task: AbortTask,
}

async fn watch(spec: SessionSpec, generation: u64, tx: mpsc::Sender<Message>) {
    loop {
        let owned = spec.clone();
        let resolved = tokio::task::spawn_blocking(move || resolve(&owned)).await;
        let session = match resolved {
            Ok(Ok(session)) => session,
            error => {
                let error = match error {
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                    _ => unreachable!(),
                };
                if tx
                    .send(Message {
                        key: spec.key.clone(),
                        generation,
                        event: UiEvent::Error(error),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        let (request_tx, request_rx) = mpsc::channel(1);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let provider = session.provider;
        let root_path = session.root.path.clone();
        let tailer = AbortTask(tokio::spawn(async move {
            let _ = crate::tailer::run(request_rx, event_tx, true, 1.0, Some(provider)).await;
        }));
        if request_tx
            .send(TailRequest::Watch(Target::Path(session.root.path)))
            .await
            .is_err()
        {
            return;
        }
        let mut health = tokio::time::interval(RETRY);
        loop {
            let incoming = tokio::select! {
                event = event_rx.recv() => event,
                _ = health.tick() => {
                    if valid_root(provider, &root_path, &spec.key.session_id) { continue; }
                    Some(UiEvent::Error("root transcript unavailable or identity changed".into()))
                },
            };
            let Some(mut event) = incoming else { break };
            if !matches!(event, UiEvent::Error(_))
                && !valid_root(provider, &root_path, &spec.key.session_id)
            {
                event = UiEvent::Error("root transcript unavailable or identity changed".into());
            }
            let id = match &event {
                UiEvent::Batch { session_id, .. }
                | UiEvent::ReplayLoaded { session_id, .. }
                | UiEvent::SessionReset { session_id } => Some(session_id),
                UiEvent::Error(_) => None,
            };
            if id.is_some_and(|id| id != &spec.key.session_id) {
                event = UiEvent::Error(
                    "transcript identity changed; waiting for registered session".into(),
                );
            }
            let retry = matches!(event, UiEvent::Error(_));
            if matches!(event, UiEvent::ReplayLoaded { .. }) {
                // A reattachment replaces this source's complete seed. Without
                // resetting first, calls removed by rotation would survive.
                if tx
                    .send(Message {
                        key: spec.key.clone(),
                        generation,
                        event: UiEvent::SessionReset {
                            session_id: spec.key.session_id.clone(),
                        },
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if tx
                .send(Message {
                    key: spec.key.clone(),
                    generation,
                    event,
                })
                .await
                .is_err()
            {
                return;
            }
            if retry {
                break;
            }
        }
        drop(tailer);
        drop(request_tx);
        tokio::time::sleep(RETRY).await;
    }
}

fn valid_root(provider: Provider, path: &Path, id: &str) -> bool {
    provider.session_file(path).is_some_and(|file| {
        file.session == id && matches!(file.role, crate::provider::FileRole::Root)
    })
}

fn reconcile_watchers(
    fleet: &Fleet,
    jobs: &mut BTreeMap<SessionKey, Watcher>,
    next: &mut u64,
    tx: &mpsc::Sender<Message>,
) {
    jobs.retain(|key, _| fleet.manifest.sessions.iter().any(|s| &s.key == key));
    for spec in &fleet.manifest.sessions {
        if jobs
            .get(&spec.key)
            .is_some_and(|j| j.spec.file == spec.file)
        {
            continue;
        }
        *next += 1;
        let task = tokio::spawn(watch(spec.clone(), *next, tx.clone()));
        jobs.insert(
            spec.key.clone(),
            Watcher {
                spec: spec.clone(),
                generation: *next,
                _task: AbortTask(task),
            },
        );
    }
}

fn route(fleet: &mut Fleet, event: &Event) -> bool {
    if let Event::Key(key) = event {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if fleet.focused.is_some() && key.code == KeyCode::Esc {
            fleet.back();
            return false;
        }
        if fleet.focused.is_none() {
            if key.code == KeyCode::Enter {
                fleet.inspect_selected();
                return false;
            }
            if key.code == KeyCode::Char('x') {
                fleet.toggle_children();
                return false;
            }
            // The fleet overview has no historical clock. DVR controls belong
            // to the selected session, where they cannot affect another root.
            if matches!(
                key.code,
                KeyCode::Char(' ' | '[' | ']' | 's' | 'S' | 'g' | 'G') | KeyCode::End
            ) {
                return false;
            }
        }
    }
    crate::handler::handle_event(event, fleet.active())
}

pub async fn run(path: PathBuf) -> Result<()> {
    let (initial, mut last_text) = load_with_text(&path)?;
    let mut fleet = Fleet::new(initial).map_err(|e| anyhow!(e))?;
    let (tx, mut rx) = mpsc::channel(64);
    let mut jobs = BTreeMap::new();
    let mut generation = 0;
    reconcile_watchers(&fleet, &mut jobs, &mut generation, &tx);
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let mut terminal = ratatui::init();
    if let Err(error) = crossterm::execute!(std::io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(error.into());
    }
    crate::tui::install_panic_hook();
    let _input = AbortTask(tokio::spawn(async move {
        let mut events = crossterm::event::EventStream::new();
        while let Some(Ok(event)) = events.next().await {
            if input_tx.send(event).is_err() {
                break;
            }
        }
    }));
    let mut tick = tokio::time::interval(Duration::from_millis(16));
    let mut refresh = tokio::time::interval(Duration::from_secs(2));
    let mut status = tokio::time::interval(Duration::from_secs(1));
    let mut last = tokio::time::Instant::now();
    let mut dirty = true;
    let result = loop {
        let now = tokio::time::Instant::now();
        let elapsed = now - last;
        last = now;
        if dirty {
            fleet.sync();
            dirty = false;
        }
        let focused = fleet.focused.is_some();
        let app = fleet.active();
        let _ = app.flow.tick_auto_pan(elapsed);
        app.flow.tick_animation(elapsed);
        app.tick_camera(elapsed);
        if focused {
            app.tick_timeline(elapsed);
        } else {
            app.chips.reconcile(elapsed, true, &app.session);
        }
        if let Err(error) = terminal.draw(|frame| draw(frame, &mut fleet)) {
            break Err(error.into());
        }
        tokio::select! {
            _ = tick.tick() => {},
            Some(message) = rx.recv() => {
                if jobs.get(&message.key).is_some_and(|j| j.generation == message.generation) {
                    fleet.event(&message.key, message.event); dirty = true;
                }
            },
            Some(event) = input_rx.recv() => { if route(&mut fleet, &event) { break Ok(()); } },
            _ = status.tick() => {
                for member in fleet.members.values_mut() { member.app.status_tick(); }
                dirty = true;
            },
            _ = refresh.tick() => {
                match load_with_text(&path) {
                    Ok((manifest, text)) if text != last_text => {
                        match fleet.update(manifest).map_err(|e| anyhow!(e)) {
                            Ok(()) => {
                                last_text = text;
                                reconcile_watchers(&fleet, &mut jobs, &mut generation, &tx);
                            },
                            Err(error) => fleet.manifest_error = Some(error.to_string()),
                        }
                        dirty = true;
                    },
                    Err(error) => { fleet.manifest_error = Some(error.to_string()); dirty = true; },
                    _ => { if fleet.manifest_error.take().is_some() { dirty = true; } },
                }
            },
        }
        // Bounded drain keeps both high-volume fleets and pointer input responsive.
        for _ in 0..64 {
            let Ok(message) = rx.try_recv() else { break };
            if jobs
                .get(&message.key)
                .is_some_and(|j| j.generation == message.generation)
            {
                fleet.event(&message.key, message.event);
                dirty = true;
            }
        }
    };
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Exposed to buffer tests so the real Fleet UI can be checked without a TTY.
pub fn draw(frame: &mut ratatui::Frame, fleet: &mut Fleet) {
    // Reuse the original graph, tool cards, inspector and single-session DVR.
    let [header, body] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(frame.area());
    crate::ui::draw_in(frame, fleet.active(), body);
    let age = (chrono::Utc::now() - fleet.manifest.observed_at)
        .num_seconds()
        .max(0);
    let unavailable = fleet
        .members
        .values()
        .filter(|m| m.error.is_some() || !m.loaded)
        .count();
    let text = if let Some(key) = &fleet.focused {
        format!(
            " {} · {}\n Session timeline · Esc: fleet · q: quit",
            fleet.manifest.label, key.session_id
        )
    } else {
        let note = fleet
            .manifest_error
            .as_deref()
            .or_else(|| fleet.manifest.diagnostics.first().map(String::as_str));
        format!(
            " {} · {} sessions · {} unavailable · snapshot {}s ago\n {}",
            fleet.manifest.label,
            fleet.members.len(),
            unavailable,
            age,
            note.unwrap_or("LIVE FLEET · Enter: session · x: children · r: arrange · q: quit")
        )
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Indexed(178)).bg(Color::Black)),
        header,
    );
    if fleet.focused.is_none() && frame.area().height > 0 {
        let area = frame.area();
        let footer = ratatui::layout::Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let agents: usize = fleet
            .members
            .values()
            .map(|m| m.app.session.agent_count())
            .sum();
        frame.render_widget(
            Paragraph::new(format!(
                " zoe-fleet · {} sessions · {agents} native agents · {} tools · live overview",
                fleet.members.len(),
                fleet.overview.session.tool_count()
            ))
            .style(Style::default().fg(Color::Indexed(178)).bg(Color::Black)),
            footer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zoe-fleet-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fn copy(from: &Path, to: &Path) {
                std::fs::create_dir_all(to).unwrap();
                for entry in std::fs::read_dir(from).unwrap() {
                    let entry = entry.unwrap();
                    if entry.path().is_dir() {
                        copy(&entry.path(), &to.join(entry.file_name()));
                    } else {
                        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
                    }
                }
            }
            copy(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fleet/demo"),
                &dir,
            );
            Self(dir)
        }
        fn manifest(&self) -> Manifest {
            load(&self.0.join("fleet.json")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fixture_resolves_three_exact_sessions_and_native_child() {
        let fixture = Fixture::new();
        let manifest = fixture.manifest();
        let mut fleet = Fleet::new(manifest.clone()).unwrap();
        for spec in &manifest.sessions {
            let session = resolve(spec).unwrap();
            let (items, info, _) = crate::tailer::replay::build_replay(&session);
            fleet.event(
                &spec.key,
                UiEvent::ReplayLoaded {
                    session_id: session.id,
                    items,
                    info,
                    speed: 1.0,
                },
            );
        }
        fleet.sync();
        assert_eq!(fleet.members.len(), 3);
        assert_eq!(fleet.overview.session.agent_count(), 5); // fleet + 3 roots + native child
        assert_eq!(fleet.overview.session.tool_count(), 4);
        assert_eq!(fleet.overview.flow.edges().len(), 4);
        let mut wrong = manifest.sessions[0].clone();
        wrong.key.session_id = "1111".into();
        assert!(resolve(&wrong).is_err()); // prefixes never pass identity validation
        wrong.key = manifest.sessions[1].key.clone();
        assert!(resolve(&wrong).is_err()); // named file belongs to another session
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_buffers_partial_lines_and_rejects_replaced_root() {
        let fixture = Fixture::new();
        let spec = fixture.manifest().sessions[0].clone();
        let path = spec.file.clone().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let _watch = AbortTask(tokio::spawn(watch(spec.clone(), 7, tx)));
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(message.generation, 7);
            if matches!(message.event, UiEvent::ReplayLoaded { .. }) {
                break;
            }
        }
        let line = r#"{"timestamp":"2026-09-22T00:00:03Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"partial","arguments":"{}"}}"#;
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writer.write_all(line.as_bytes()).unwrap();
        writer.flush().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(450), rx.recv())
                .await
                .is_err()
        );
        writer.write_all(b"\n").unwrap();
        writer.flush().unwrap();
        let message = tokio::time::timeout(Duration::from_secs(4), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let UiEvent::Batch { statements, .. } = message.event else {
            panic!("expected append")
        };
        assert!(statements.iter().flat_map(|s| &s.facts).any(|f|
            matches!(&f.kind, crate::fact::FactKind::ToolStart { call, .. } if call == "partial")));
        drop(writer);
        std::fs::write(
            &path,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"unrelated\",\"source\":\"exec\"}}\n",
        )
        .unwrap();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(message.event, UiEvent::Error(_)) {
                break;
            }
            assert!(
                !matches!(message.event, UiEvent::Batch { .. }),
                "replacement activity must not be forwarded"
            );
        }
    }
}
