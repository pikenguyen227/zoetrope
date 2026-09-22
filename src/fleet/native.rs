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

use super::journal::JournalTail;
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
    let journal = JournalTail::beside(path).poll()?;
    fleet.absorb_lifecycle(journal.reset, journal.events, journal.rejected);
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
            // The overview's transport moves the one fleet playhead, and every
            // member with it. Its timeline is an index it never folds, so these
            // must not reach the overview App's own single-session handlers.
            match key.code {
                KeyCode::Char(' ') => fleet.toggle_play_pause(),
                KeyCode::Char('[') => fleet.step(false),
                KeyCode::Char(']') => fleet.step(true),
                KeyCode::Char('g' | 'G') | KeyCode::End => fleet.go_live(),
                _ => return crate::handler::handle_event(event, fleet.active()),
            }
            return false;
        }
    }
    crate::handler::handle_event(event, fleet.active())
}

pub async fn run(path: PathBuf) -> Result<()> {
    let (initial, mut last_text) = load_with_text(&path)?;
    let mut fleet = Fleet::new(initial).map_err(|e| anyhow!(e))?;
    let mut journal = JournalTail::beside(&path);
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
    let mut telemetry = Telemetry::default();
    let mut last = tokio::time::Instant::now();
    let mut dirty = true;
    let result = loop {
        let now = tokio::time::Instant::now();
        let elapsed = now - last;
        last = now;
        let focused = fleet.focused.is_some();
        if !focused {
            // The fleet playhead moves (and drives the members) before the
            // projection is rebuilt from them.
            dirty |= fleet.tick_timeline(elapsed);
        }
        if dirty {
            fleet.sync();
            dirty = false;
        }
        // Chips age while the fleet plays, and freeze when it is paused or parked.
        let playing = fleet.at().is_none()
            || (fleet.overview.timeline.follow_head && !fleet.overview.is_paused);
        let app = fleet.active();
        let _ = app.flow.tick_auto_pan(elapsed);
        app.flow.tick_animation(elapsed);
        app.tick_camera(elapsed);
        app.tick_pulse(elapsed);
        if focused {
            app.tick_timeline(elapsed);
        } else {
            app.chips.reconcile(elapsed, playing, &app.session);
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
                telemetry.refresh(&mut fleet);
                for member in fleet.members.values_mut() { member.app.status_tick(); }
                dirty = true;
            },
            _ = refresh.tick() => {
                match journal.poll() {
                    Ok(poll) => {
                        dirty |= fleet.absorb_lifecycle(poll.reset, poll.events, poll.rejected);
                    }
                    Err(error) => {
                        fleet.manifest_error = Some(format!("lifecycle journal: {error}"));
                        dirty = true;
                    }
                }
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
    let limits = quota_lines(fleet, chrono::Utc::now());
    let [header, body, quota] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(limits.len().min(6) as u16),
    ])
    .areas(frame.area());
    // The overview's timeline is the fleet's merged index, so this draws the
    // upstream scrubber for the whole crew; lifecycle marks and coverage gaps
    // go over it.
    crate::ui::draw_in(frame, fleet.active(), body);
    if fleet.focused.is_none() {
        super::timeline::draw_overlay(frame, fleet);
    }
    frame.render_widget(
        Paragraph::new(limits.join("\n")).style(Style::default().fg(Color::Gray).bg(Color::Black)),
        quota,
    );
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
        // A moment nobody observed, past or present, outranks today's diagnostics.
        let gap = (!fleet.covered())
            .then(|| fleet.at().unwrap_or_else(chrono::Utc::now))
            .map(|t| {
                format!(
                    "no lifecycle coverage at {}: badges marked ? are last records, unverified",
                    t.with_timezone(&chrono::Local).format("%H:%M:%S")
                )
            });
        let note = gap
            .as_deref()
            .or(fleet.manifest_error.as_deref())
            .or_else(|| fleet.manifest.diagnostics.first().map(String::as_str));
        format!(
            " {} · {} sessions · {} unavailable · snapshot {}s ago\n {}",
            fleet.manifest.label,
            fleet.members.len(),
            unavailable,
            age,
            note.unwrap_or(
                "FLEET · space: play/pause · drag, [ ]: seek · g: live · Enter: session · x: children · q: quit"
            )
        )
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Indexed(178)).bg(Color::Black)),
        header,
    );
    if fleet.focused.is_none() && frame.area().height > 0 {
        let footer =
            ratatui::layout::Rect::new(body.x, body.bottom().saturating_sub(1), body.width, 1);
        // Clear the underlying session status, including its right-aligned
        // hints and modifiers, before replacing it with Fleet status.
        frame.render_widget(ratatui::widgets::Clear, footer);
        let agents: usize = fleet
            .members
            .values()
            .map(|m| m.app.session.agent_count())
            .sum();
        let when = match fleet.at() {
            None => "live".to_string(),
            Some(t) => t
                .with_timezone(&chrono::Local)
                .format("%b %d %H:%M:%S")
                .to_string(),
        };
        let mut text = format!(
            " zoe-fleet · {} sessions · {agents} native agents · {} tools · {when}",
            fleet.members.len(),
            fleet.overview.session.tool_count()
        );
        // Narrate the crew: the newest lifecycle event at the playhead.
        if let Some(event) = fleet.lifecycle.latest_at(fleet.at()) {
            let what = match &event.change {
                super::journal::Change::Status { status } => status.value.clone(),
                change => change.name().replace('_', " "),
            };
            let task = event.attempt.as_ref().map_or("", |a| a.task.as_str());
            let at = event.at.map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string()
            });
            text.push_str(&format!(" · ◆ {} {task} {what}", at.unwrap_or_default()));
        }
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Indexed(178)).bg(Color::Black)),
            footer,
        );
    }
}

#[derive(Default)]
struct Telemetry {
    seen: BTreeMap<SessionKey, std::time::SystemTime>,
}
impl Telemetry {
    fn refresh(&mut self, fleet: &mut Fleet) {
        let Some(dir) = std::env::var_os("ZOE_TELEMETRY_DIR") else {
            return;
        };
        for (key, member) in fleet
            .members
            .iter_mut()
            .filter(|(key, m)| key.provider == "claude" && !m.retained)
        {
            if !key
                .session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                continue;
            }
            let path = Path::new(&dir).join(format!("claude-{}.json", key.session_id));
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.len() > 16_384 {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if self.seen.get(key) == Some(&modified) && !member.app.session_info.quotas.is_empty() {
                continue;
            }
            let Some(quota) = read_telemetry(&path, &key.session_id) else {
                continue;
            };
            member.app.session_info.apply(&crate::fact::Fact {
                agent: None,
                ts: None,
                kind: crate::fact::FactKind::Quota(quota),
            });
            self.seen.insert(key.clone(), modified);
        }
    }
}
fn read_telemetry(path: &Path, session_id: &str) -> Option<crate::usage::Quota> {
    #[derive(serde::Deserialize)]
    struct Snapshot {
        schema: String,
        session_id: String,
        quota: crate::usage::Quota,
    }
    let data: Snapshot = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (data.schema == "zoe.usage/v1" && data.session_id == session_id && data.quota.valid())
        .then_some(data.quota)
}

/// Quotas are account snapshots, never summed across sessions. Each reported
/// provider bucket remains separate; the newest observation wins.
fn quota_lines(fleet: &Fleet, now: chrono::DateTime<chrono::Utc>) -> Vec<String> {
    let mut buckets: BTreeMap<(String, String), crate::usage::Quota> = BTreeMap::new();
    let mut providers = std::collections::BTreeSet::new();
    for (key, m) in fleet.members.iter().filter(|(_, m)| !m.retained) {
        let provider = key.provider.to_string();
        providers.insert(provider.clone());
        for (bucket, q) in &m.app.session_info.quotas {
            let k = (provider.clone(), bucket.clone());
            if buckets
                .get(&k)
                .is_none_or(|old| old.observed_at < q.observed_at)
            {
                buckets.insert(k, q.clone());
            }
        }
    }
    let mut lines = Vec::new();
    for provider in providers {
        let matching: Vec<_> = buckets
            .iter()
            .filter(|((p, _), _)| p == &provider)
            .collect();
        if matching.is_empty() {
            lines.push(format!(
                " {provider} account · 5h — · week — · limits not reported"
            ));
        } else {
            for ((_, bucket), q) in matching {
                let label = if bucket == &provider {
                    provider.clone()
                } else {
                    format!("{provider}/{bucket}")
                };
                lines.push(format!(" {label} shared · {}", q.label(now)));
            }
        }
    }
    lines
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
            Self::of("demo")
        }
        fn of(name: &str) -> Self {
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
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/fleet")
                    .join(name),
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
        for (w, h) in [(140, 44), (80, 24), (40, 12)] {
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &mut fleet)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            if h >= 18 {
                // The crew's merged timeline, not a sparkline: a seekable
                // scrubber at a quiet live edge.
                assert!(fleet.overview.scrubber_area.is_some());
                assert!(text.contains("■ end"));
                assert!(text.contains("· live"));
            }
            assert!(text.contains("5h"));
        }
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

    #[test]
    fn limits_are_shared_not_summed_and_snapshots_require_exact_session() {
        let fixture = Fixture::new();
        let mut fleet = Fleet::new(fixture.manifest()).unwrap();
        let now = chrono::Utc::now();
        for (index, member) in fleet.members.values_mut().enumerate() {
            let q = crate::usage::Quota {
                bucket: "codex".into(),
                observed_at: now + chrono::Duration::seconds(index as i64),
                windows: vec![crate::usage::Window {
                    minutes: 300,
                    used_percent: 20.0 + index as f64,
                    resets_at: None,
                }],
            };
            member.app.session_info.quotas.insert("codex".into(), q);
        }
        let lines = quota_lines(&fleet, now);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("78% left"));
        let p = fixture.0.join("quota.json");
        let q = fleet
            .members
            .values()
            .next()
            .unwrap()
            .app
            .session_info
            .quotas["codex"]
            .clone();
        std::fs::write(
            &p,
            serde_json::json!({"schema":"zoe.usage/v1","session_id":"exact","quota":q}).to_string(),
        )
        .unwrap();
        assert!(read_telemetry(&p, "exact").is_some());
        assert!(read_telemetry(&p, "other").is_none());
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

    /// The crew fixture, loaded the way `run` does: its journal, then every
    /// transcript, at the live edge.
    fn crew() -> (Fixture, Fleet) {
        let fixture = Fixture::of("crew");
        let path = fixture.0.join("fleet.json");
        let manifest = load(&path).unwrap();
        let mut fleet = Fleet::new(manifest.clone()).unwrap();
        let poll = JournalTail::beside(&path).poll().unwrap();
        assert!(fleet.absorb_lifecycle(poll.reset, poll.events, poll.rejected));
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
        (fixture, fleet)
    }

    fn at(hms: &str) -> chrono::DateTime<chrono::Utc> {
        format!("2026-09-22T{hms}Z").parse().unwrap()
    }

    fn root(name: &str) -> String {
        let n = match name {
            "captain" => 1,
            "impl" => 2,
            _ => 3,
        };
        SessionKey {
            provider: "codex".into(),
            session_id: format!("c0c0c0c0-0000-4000-8000-00000000000{n}"),
        }
        .node_id(crate::state::session::MAIN_ID)
    }

    const DOCS: &str = r#"["task","docs","s1790064660.4103.3"]"#;

    fn mark(fleet: &mut Fleet, id: &str) -> Option<crate::ui::nodes::CrewMark> {
        fleet.overview.flow.node_content_mut(id)?.crew.clone()
    }

    fn seek(fleet: &mut Fleet, hms: &str) {
        fleet.seek(at(hms));
        fleet.sync();
    }

    fn screen(fleet: &mut Fleet) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 48)).unwrap();
        terminal.draw(|frame| draw(frame, fleet)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn crew_timeline_shows_the_crew_as_it_was() {
        use crate::state::session::AgentStatus;
        use crate::ui::nodes::CrewTone;
        let (_fixture, mut fleet) = crew();
        // Live: everyone ever registered, lifecycle read in full.
        assert_eq!(fleet.at(), None);
        let docs = mark(&mut fleet, DOCS).unwrap();
        assert!(docs.dimmed, "docs was torn down: {docs:?}");
        assert!(mark(&mut fleet, &root("impl")).unwrap().dimmed);
        // The finished run's adapter stopped long ago: today's badges are its
        // last records, unverified.
        assert_eq!(
            mark(&mut fleet, &root("tests")).unwrap().tone,
            CrewTone::Unknown
        );
        assert!(screen(&mut fleet).contains("no lifecycle coverage"));

        // 08:03 — only the captain and impl exist; tests and docs are not
        // spawned yet, so they are not there at all.
        seek(&mut fleet, "08:03:00");
        assert_eq!(fleet.at(), Some(at("08:03:00")));
        assert!(fleet.overview.session.agent(&root("tests")).is_none());
        assert!(fleet.overview.session.agent(DOCS).is_none());
        let impl_ = fleet.overview.session.agent(&root("impl")).unwrap();
        assert_eq!(impl_.tool_calls().len(), 2, "calls folded as of 08:03");
        // Liveness reads the playhead, not today's wall clock hours later.
        assert_eq!(impl_.status, AgentStatus::Running);
        let working = mark(&mut fleet, &root("impl")).unwrap();
        assert_eq!(working.tone, CrewTone::Active);
        assert!(working.label.starts_with("working"));
        assert!(!working.dimmed);

        // 08:05:10 — impl is waiting on the operator.
        seek(&mut fleet, "08:05:10");
        let asking = mark(&mut fleet, &root("impl")).unwrap();
        assert_eq!(asking.tone, CrewTone::Attention);
        assert!(asking.label.starts_with("needs-decision"));
        assert!(!screen(&mut fleet).contains("no lifecycle coverage"));

        // 08:09 — nobody was observing: the last records are shown, unverified.
        seek(&mut fleet, "08:09:00");
        let unverified = mark(&mut fleet, &root("impl")).unwrap();
        assert_eq!(unverified.tone, CrewTone::Unknown);
        assert!(unverified.label.starts_with("done"));
        assert!(!unverified.dimmed, "its teardown was only seen at 08:10");
        assert!(screen(&mut fleet).contains("no lifecycle coverage"));

        // 08:11:30 — docs spawned without a session; impl is gone, dimmed.
        seek(&mut fleet, "08:11:30");
        let docs = mark(&mut fleet, DOCS).unwrap();
        assert_eq!((docs.tone, docs.dimmed), (CrewTone::Active, false));
        assert!(mark(&mut fleet, &root("impl")).unwrap().dimmed);
        let tests = mark(&mut fleet, &root("tests")).unwrap();
        assert_eq!(
            tests.tone,
            CrewTone::Attention,
            "stamped in the gap, seen at 08:10"
        );

        seek(&mut fleet, "08:12:30");
        let blocked = mark(&mut fleet, &root("tests")).unwrap();
        assert!(blocked.label.starts_with("blocked") && blocked.tone == CrewTone::Attention);
        seek(&mut fleet, "08:14:00");
        assert!(mark(&mut fleet, DOCS).unwrap().dimmed);

        // Back to the edge: members ride their own live edges again.
        fleet.go_live();
        fleet.sync();
        assert_eq!(fleet.at(), None);
        assert!(fleet.members.values().all(|m| m.app.timeline.follow_head));
    }

    #[test]
    fn crew_keys_move_the_one_playhead() {
        let (_fixture, mut fleet) = crew();
        let key = |code| Event::Key(crossterm::event::KeyEvent::from(code));
        let head = fleet.overview.timeline.head_ts().unwrap();
        // Space parks the whole crew where it stands.
        assert!(!route(&mut fleet, &key(KeyCode::Char(' '))));
        assert!(fleet.overview.is_paused);
        assert_eq!(fleet.at(), Some(head));
        assert!(
            fleet.tick_timeline(Duration::from_millis(16)),
            "leaving the edge re-reads the crew as of that moment"
        );
        assert!(fleet.members.values().all(|m| !m.app.timeline.follow_head));
        // `[` steps to the previous chapter: the last lifecycle transition.
        route(&mut fleet, &key(KeyCode::Char('[')));
        assert_eq!(fleet.at(), Some(at("08:14:30")));
        route(&mut fleet, &key(KeyCode::Char('[')));
        assert_eq!(fleet.at(), Some(at("08:13:00")));
        // `]` past the last chapter is the live edge.
        route(&mut fleet, &key(KeyCode::Char(']')));
        route(&mut fleet, &key(KeyCode::Char(']')));
        assert_eq!(fleet.at(), None);
        // A scrubber click lands through the frame tick, like a drag.
        fleet.overview.pending_seek = Some(0.0);
        assert!(fleet.tick_timeline(Duration::from_millis(16)));
        assert_eq!(
            fleet.at(),
            Some(fleet.overview.timeline.start_ts().unwrap())
        );
        // `g` goes live from anywhere.
        route(&mut fleet, &key(KeyCode::Char('g')));
        assert_eq!(fleet.at(), None);
        // The overview App's own DVR never ran: its model is still a projection.
        assert_eq!(
            fleet.overview.timeline.folded,
            fleet.overview.timeline.items.len()
        );
    }

    #[test]
    fn crew_playback_carries_every_member_forward() {
        let (_fixture, mut fleet) = crew();
        seek(&mut fleet, "08:02:10");
        fleet.toggle_play_pause();
        assert!(fleet.overview.timeline.follow_head && !fleet.overview.is_paused);
        let calls = |fleet: &Fleet| {
            fleet.members[&SessionKey {
                provider: "codex".into(),
                session_id: "c0c0c0c0-0000-4000-8000-000000000002".into(),
            }]
                .app
                .session
                .tool_count()
        };
        let before = calls(&fleet);
        let mut last = fleet.at().unwrap();
        for _ in 0..600 {
            fleet.tick_timeline(Duration::from_millis(16));
            let now = fleet.at().unwrap_or(last);
            assert!(now >= last, "the playhead never runs backwards");
            last = now;
        }
        fleet.sync();
        assert!(last > at("08:02:40"), "played to {last}");
        assert!(calls(&fleet) > before);
    }

    #[test]
    fn leaving_a_session_rejoins_the_fleet_moment() {
        let (_fixture, mut fleet) = crew();
        seek(&mut fleet, "08:05:10");
        fleet.overview.flow.select_node(&root("impl"));
        fleet.inspect_selected();
        // The session's own DVR wanders off to its start.
        fleet.active().seek_to_fraction(0.0);
        fleet.back();
        let member = fleet
            .members
            .values()
            .find(|m| m.spec.label == "impl")
            .unwrap();
        assert_eq!(member.app.timeline.cursor, Some(at("08:05:10")));
        assert!(!member.app.timeline.follow_head);
    }

    #[test]
    fn crew_scrubber_draws_lifecycle_marks_and_coverage_gaps() {
        let (_fixture, mut fleet) = crew();
        seek(&mut fleet, "08:15:00");
        screen(&mut fleet);
        let bar = fleet
            .overview
            .scrubber_area
            .expect("the fleet has a scrubber");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 48)).unwrap();
        terminal.draw(|frame| draw(frame, &mut fleet)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String {
            (bar.x..bar.x + bar.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        };
        let markers = row(bar.y);
        for glyph in ["+", "▲", "✓", "⊘"] {
            assert!(markers.contains(glyph), "{glyph} missing from {markers:?}");
        }
        // The 08:08-08:10 stretch and the time before coverage are hatched.
        assert!(
            (bar.y..bar.y + bar.height).any(|y| row(y).contains('░')),
            "no gap hatching"
        );
        let text = screen(&mut fleet);
        assert!(text.contains("◆"), "the footer narrates the crew");
        assert!(text.contains("tests working"));
    }

    #[test]
    fn without_a_journal_the_past_claims_no_lifecycle() {
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
        fleet.seek("2026-09-22T00:00:01Z".parse().unwrap());
        fleet.sync();
        assert!(fleet.at().is_some());
        let ids: Vec<String> = fleet.overview.flow.nodes().map(|n| n.id.clone()).collect();
        assert!(!ids.is_empty());
        for id in ids {
            assert!(
                fleet
                    .overview
                    .flow
                    .node_content_mut(&id)
                    .unwrap()
                    .crew
                    .is_none()
            );
        }
        let text = screen(&mut fleet);
        assert!(!text.contains("no lifecycle coverage") && !text.contains('░'));
    }
}
