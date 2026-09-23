//! A validation run's pipeline agents, on demand: from a worker's validation
//! band down to one agent's own transcript, read-only.
//!
//! no-mistakes publishes no identity for its agents, so the join is an
//! inference from what is already on disk, and it says so. Each pipeline agent
//! works in the run's worktree (`<no-mistakes home>/worktrees/<repo>/<run>`),
//! and Claude files its transcript under that directory's project key. A
//! transcript is taken as the run's only when that key names the run inside a
//! no-mistakes worktree and its own dated records fall within the run's life:
//! none before the run was created (decoded from its ID), none after a read
//! found it ended. Everything else is listed with its reason and cannot be
//! opened: a session filed under two runs (ambiguous), a file that cannot be
//! read, one with no dated record to check, or one whose records fall outside
//! the run. Nothing ties an agent to a step: no record here says which one.
//!
//! An opened transcript is an [`Inspection`]: its own watcher and inspector
//! while it is open, never a fleet member, never in the manifest, and never
//! counted in the crew.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::journal::{Change, Phase};
use super::{Fleet, Prompt, SessionKey, SessionSpec, clock};
use crate::provider::{FileRole, Provider};
use crate::state::{App, Mode};
use crate::tailer::UiEvent;

/// When a run was alive, as far as the viewer can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Life {
    /// Its creation, decoded from its ID.
    pub created: DateTime<Utc>,
    /// The first read that found it ended; `None` while none has.
    pub ended_by: Option<DateTime<Utc>>,
}

/// What one transcript's own records say, through its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Its first and last dated records; `None`: none is dated.
    pub span: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub tools: usize,
    pub subagents: usize,
    pub model: Option<String>,
}

/// Why a transcript is, or is not, taken as one of the run's agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Filed under the run and within its life: inferred, never published.
    Inferred,
    /// Filed under the run, and under another run, or twice under this one.
    Ambiguous(String),
    Unreadable(String),
    /// No dated record: its time cannot be checked against the run.
    Undated,
    /// Its records fall outside the run's life.
    Outside(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub session: String,
    pub path: PathBuf,
    pub reading: Option<Reading>,
    pub standing: Standing,
}

impl Agent {
    /// A row of the list: when it wrote, how much, and why it is not openable.
    pub fn line(&self) -> String {
        let short: String = self.session.chars().take(8).collect();
        let mut words = Vec::new();
        match self.reading.as_ref().and_then(|r| r.span) {
            Some((first, last)) => {
                words.push(format!("{}–{}", clock(first), clock(last)));
                words.push(span((last - first).num_seconds()));
            }
            None => words.push("no dated record".into()),
        }
        if let Some(reading) = &self.reading {
            words.push(count(reading.tools, "tool"));
            if reading.subagents > 0 {
                words.push(count(reading.subagents, "subagent"));
            }
            words.extend(reading.model.clone());
        }
        words.push(short);
        let why = match &self.standing {
            Standing::Inferred => return words.join(" · "),
            Standing::Ambiguous(why) => format!("ambiguous: {why}"),
            Standing::Unreadable(why) => format!("unreadable: {why}"),
            Standing::Undated => "cannot be placed in the run: no dated record".into(),
            Standing::Outside(why) => format!("not this run's: {why}"),
        };
        format!("{} · {why}", words.join(" · "))
    }

    pub fn openable(&self) -> bool {
        self.standing == Standing::Inferred
    }
}

/// A run's transcripts, as found on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agents {
    pub run: String,
    pub life: Life,
    /// Ordered by first record; undated last.
    pub agents: Vec<Agent>,
}

impl Agents {
    /// The list's lines above its rows: what the join is, and what it is not.
    pub fn heading(&self) -> Vec<String> {
        let ended = self.life.ended_by.map_or_else(
            || "not read ended".into(),
            |at| format!("read ended by {}", clock(at)),
        );
        let mut lines = vec![
            format!(
                "run {} · created {} · {ended}",
                self.run,
                clock(self.life.created)
            ),
            "Inferred, not published by no-mistakes: filed under this run's worktree, \
             with every record inside its life. No agent is tied to a step."
                .into(),
        ];
        if self.agents.is_empty() {
            lines.push(
                "No transcript is filed under this run's worktree: its agents have not \
                 started, were removed, or are not Claude's."
                    .into(),
            );
        }
        lines
    }
}

/// The list the operator picks from, over the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub agents: Agents,
    pub selected: usize,
    /// Why the last Enter opened nothing.
    pub note: Option<String>,
}

/// One pipeline agent's transcript, open read-only in the session inspector.
pub struct Inspection {
    /// What its watcher resolves: the exact file, checked against its session.
    pub spec: SessionSpec,
    pub run: String,
    pub app: App,
    pub loaded: bool,
    pub error: Option<String>,
    /// The fleet's moment when it was opened, to park at once loaded.
    park: Option<DateTime<Utc>>,
}

const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Whether `id` has a no-mistakes run ID's shape: a ULID.
fn is_ulid(id: &str) -> bool {
    id.len() == 26 && id.bytes().all(|b| CROCKFORD.contains(&b)) && id.as_bytes()[0] <= b'7'
}

/// A run's creation, from the millisecond time its ULID starts with.
pub fn created(run: &str) -> Option<DateTime<Utc>> {
    if !is_ulid(run) {
        return None;
    }
    let ms = run.bytes().take(10).fold(0i64, |ms, b| {
        ms * 32 + CROCKFORD.iter().position(|&c| c == b).unwrap() as i64
    });
    DateTime::from_timestamp_millis(ms)
}

/// The run a Claude project directory belongs to: one in a no-mistakes
/// worktree, named for the run it ends with.
fn run_of(project_key: &str) -> Option<&str> {
    let anchor = Provider::Claude.project_key(Path::new("/.no-mistakes/worktrees/"));
    let (_, run) = project_key.rsplit_once('-')?;
    (project_key.contains(&anchor) && is_ulid(run)).then_some(run)
}

/// Join `run`'s transcripts among `roots` (root transcript paths): those
/// filed under the run's worktree, each read through `read` and placed
/// against the run's `life`. Only those files are read.
pub fn join(
    run: &str,
    life: Life,
    roots: impl IntoIterator<Item = PathBuf>,
    read: impl Fn(&Path) -> Result<Reading, String>,
) -> Agents {
    let mut ours: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut runs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in roots {
        let Some(file) = Provider::Claude.session_file(&path) else {
            continue;
        };
        if file.role != FileRole::Root {
            continue;
        }
        let Some(of) = run_of(&file.project_key) else {
            continue;
        };
        runs.entry(file.session.clone())
            .or_default()
            .insert(of.to_owned());
        if of == run {
            ours.entry(file.session).or_default().push(path);
        }
    }
    let mut agents: Vec<Agent> = Vec::new();
    for (session, paths) in ours {
        let others: Vec<_> = runs[&session]
            .iter()
            .filter(|r| *r != run)
            .cloned()
            .collect();
        let twice = paths.len() > 1;
        for path in paths {
            let reading = read(&path);
            let standing = match &reading {
                Err(why) => Standing::Unreadable(why.clone()),
                Ok(_) if !others.is_empty() => {
                    Standing::Ambiguous(format!("also filed under run {}", others.join(", ")))
                }
                Ok(_) if twice => {
                    Standing::Ambiguous("filed twice under this run's worktrees".into())
                }
                Ok(Reading { span: None, .. }) => Standing::Undated,
                Ok(Reading {
                    span: Some((first, last)),
                    ..
                }) => place(*first, *last, life),
            };
            agents.push(Agent {
                session: session.clone(),
                path,
                reading: reading.ok(),
                standing,
            });
        }
    }
    // By first record, undated last.
    agents.sort_by_key(|a| {
        let first = a
            .reading
            .as_ref()
            .and_then(|r| r.span)
            .map(|(first, _)| first);
        (first.is_none(), first, a.session.clone())
    });
    Agents {
        run: run.to_owned(),
        life,
        agents,
    }
}

/// Whether records from `first` to `last` fit inside the run's life.
fn place(first: DateTime<Utc>, last: DateTime<Utc>, life: Life) -> Standing {
    if first < life.created {
        return Standing::Outside(format!(
            "it began {}, before the run was created {}",
            clock(first),
            clock(life.created)
        ));
    }
    match life.ended_by {
        Some(end) if last > end => Standing::Outside(format!(
            "it wrote until {}, after a read found the run ended {}",
            clock(last),
            clock(end)
        )),
        _ => Standing::Inferred,
    }
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn span(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{}s", secs / 60, secs % 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

impl Fleet {
    /// The validation run the selected card shows, as of the playhead: the
    /// newest run among the attempts joined to it, as its band reads.
    pub fn selected_run(&self) -> Option<String> {
        let id = self.overview.selected_agent_id()?;
        let attempts: Vec<_> = if let Some((key, _)) = self.nodes.get(&id) {
            self.joins()
                .into_iter()
                .filter(|(_, k)| k == key)
                .map(|(a, _)| a)
                .collect()
        } else {
            self.cards.get(&id).cloned().into_iter().collect()
        };
        let runs = self.lifecycle.validation_at(self.at());
        attempts
            .iter()
            .filter_map(|a| runs.get(a))
            .max_by_key(|view| view.start())
            .map(|view| view.run.clone())
    }

    /// `run`'s life: created per its ID, ended per the first read that said
    /// so, whenever that was.
    pub fn run_life(&self, run: &str) -> Option<Life> {
        let ended_by = self
            .lifecycle
            .ordered()
            .filter_map(|e| match &e.change {
                Change::Validation { validation }
                    if validation.run == run && validation.phase == Phase::Ended =>
                {
                    e.at
                }
                _ => None,
            })
            .min();
        Some(Life {
            created: created(run)?,
            ended_by,
        })
    }

    /// List the selected card's run agents, found by `find`; or say why not.
    pub fn list_agents(&mut self, find: impl FnOnce(&str, Life) -> Agents) {
        let Some(run) = self.selected_run() else {
            let when = self
                .at()
                .map_or_else(|| "now".into(), |t| format!("at {}", clock(t)));
            self.prompt = Some(Prompt::Notice(format!(
                "No validation run on the selected card {when}: select a worker whose card \
                 shows one, then p lists the run's pipeline agents"
            )));
            return;
        };
        let Some(life) = self.run_life(&run) else {
            self.prompt = Some(Prompt::Notice(format!(
                "Run {run} has no ID time to place its agents against, so none is listed"
            )));
            return;
        };
        self.picker = Some(Picker {
            agents: find(&run, life),
            selected: 0,
            note: None,
        });
    }

    pub fn pick_move(&mut self, down: bool) {
        if let Some(picker) = &mut self.picker {
            let last = picker.agents.agents.len().saturating_sub(1);
            picker.selected = if down {
                (picker.selected + 1).min(last)
            } else {
                picker.selected.saturating_sub(1)
            };
            picker.note = None;
        }
    }

    /// Open the chosen transcript read-only, if the join stands for it.
    pub fn pick_open(&mut self) {
        let at = self.at();
        let Some(picker) = &mut self.picker else {
            return;
        };
        let Some(agent) = picker.agents.agents.get(picker.selected) else {
            picker.note = Some("Nothing to open".into());
            return;
        };
        if !agent.openable() {
            picker.note = Some(format!(
                "Not opened, the join does not hold: {}",
                agent.line()
            ));
            return;
        }
        let key = SessionKey {
            provider: Provider::Claude.name().into(),
            session_id: agent.session.clone(),
        };
        let short: String = agent.session.chars().take(8).collect();
        self.inspection = Some(Inspection {
            app: App::new(key.session_id.clone(), Mode::Live),
            spec: SessionSpec {
                key,
                label: format!("pipeline agent {short}"),
                file: Some(agent.path.clone()),
                runtime: None,
            },
            run: picker.agents.run.clone(),
            loaded: false,
            error: None,
            park: at,
        });
    }

    /// Back to the list, dropping the inspection and its watcher.
    pub fn close_inspection(&mut self) {
        if self.inspection.take().is_some() {
            self.sync();
            self.overview.chips.adopt_baseline(&self.overview.session);
        }
    }

    /// An event from the inspection's watcher, checked as a member's are.
    pub fn inspection_event(&mut self, event: UiEvent) {
        let Some(open) = &mut self.inspection else {
            return;
        };
        let id = match &event {
            UiEvent::Batch { session_id, .. }
            | UiEvent::ReplayLoaded { session_id, .. }
            | UiEvent::SessionReset { session_id } => Some(session_id),
            UiEvent::Error(_) => None,
        };
        if id.is_some_and(|id| id != &open.spec.key.session_id) {
            open.error = Some("transcript identity changed; refusing unrelated session".into());
            return;
        }
        let replayed = matches!(event, UiEvent::ReplayLoaded { .. });
        match &event {
            UiEvent::Error(error) => open.error = Some(error.clone()),
            UiEvent::SessionReset { .. } => open.loaded = false,
            _ => {
                open.loaded = true;
                open.error = None;
                open.app.last_error = None;
            }
        }
        open.app.handle_ui_event(event);
        // Like Enter on a member: open at the fleet's moment.
        if replayed && let Some(t) = open.park.take() {
            open.app.park_at(t);
        }
    }

    /// Whether a session inspector has the screen: a member's or an
    /// inspection's.
    pub fn in_session(&self) -> bool {
        self.focused.is_some() || self.inspection.is_some()
    }
}

/// Every root transcript to join from: Claude's own, or those in `dir`, a
/// directory of Claude project directories (fixtures).
#[cfg(feature = "native")]
pub fn roots(dir: Option<&Path>) -> Vec<PathBuf> {
    let Some(dir) = dir else {
        return Provider::Claude.all_paths(&crate::provider::Scope::ALL);
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|project| std::fs::read_dir(project.path()).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    out.sort();
    out
}

/// What a transcript's records say, read through its provider, subagents
/// included. A file that cannot be opened is unreadable.
#[cfg(feature = "native")]
pub fn read(path: &Path) -> Result<Reading, String> {
    use crate::fact::FactKind;
    use crate::state::session::MAIN_ID;
    std::fs::File::open(path).map_err(|e| e.to_string())?;
    let session = crate::provider::open(
        &crate::provider::Target::Path(path.to_path_buf()),
        Some(Provider::Claude),
    )
    .map_err(|e| e.to_string())?;
    let (items, _, _) = crate::tailer::replay::build_replay(&session);
    let mut span: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    let (mut tools, mut subagents, mut model) = (0, BTreeSet::new(), None);
    for item in &items {
        if let Some(t) = item.ts() {
            span = Some(span.map_or((t, t), |(a, b)| (a.min(t), b.max(t))));
        }
        for fact in &item.facts {
            match &fact.kind {
                FactKind::ToolStart { .. } => tools += 1,
                FactKind::Agent { .. } | FactKind::Model(_) => match fact.agent.as_deref() {
                    Some(MAIN_ID) => {
                        if let FactKind::Model(m) = &fact.kind {
                            model = Some(m.clone());
                        }
                    }
                    Some(agent) if matches!(fact.kind, FactKind::Agent { .. }) => {
                        subagents.insert(agent.to_owned());
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(Reading {
        span,
        tools,
        subagents: subagents.len(),
        model,
    })
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    const RUN: &str = "01M342G5B2S1MP13RVNVA11DAT";
    const OTHER: &str = "01M342X98JT3STSRVNVA11DAT1";

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fleet/pipeline/projects")
    }

    fn at(hms: &str) -> DateTime<Utc> {
        format!("2026-09-22T{hms}Z").parse().unwrap()
    }

    fn life() -> Life {
        Life {
            created: created(RUN).unwrap(),
            ended_by: Some(at("08:14:00")),
        }
    }

    fn standing(agents: &Agents, session: &str) -> Standing {
        agents
            .agents
            .iter()
            .find(|a| a.session.starts_with(session))
            .unwrap_or_else(|| panic!("{session} not listed"))
            .standing
            .clone()
    }

    #[test]
    fn a_run_id_is_its_creation_time() {
        assert_eq!(
            created(RUN),
            Some("2026-09-22T08:07:45.250Z".parse().unwrap())
        );
        assert_eq!(created("not-a-run"), None);
        assert_eq!(
            created("01M342G5B2S1MP13RVNVA11DAI"),
            None,
            "I is not Crockford"
        );
        assert_eq!(
            run_of(&format!("-Users-me--no-mistakes-worktrees-abc-{RUN}")),
            Some(RUN)
        );
        // Named for the run, but not a no-mistakes worktree.
        assert_eq!(run_of(&format!("-Users-me-elsewhere-{RUN}")), None);
        assert_eq!(run_of("-Users-me--no-mistakes-worktrees-abc-main"), None);
    }

    #[test]
    fn the_join_is_the_run_worktree_and_the_transcripts_own_time() {
        let agents = join(RUN, life(), roots(Some(&fixture())), read);
        let sessions: Vec<_> = agents.agents.iter().map(|a| &a.session[..8]).collect();
        // Only the run's directory: never another run's agent, a project
        // directory that merely ends in the run ID, or an unrelated one.
        assert_eq!(
            sessions,
            [
                "a4444444", "a1111111", "a2222222", "a3333333", "a6666666", "a5555555"
            ]
        );
        assert_eq!(standing(&agents, "a1111111"), Standing::Inferred);
        let first = &agents.agents[1];
        assert_eq!(
            first.reading,
            Some(Reading {
                span: Some((at("08:08:02.100"), at("08:09:30"))),
                tools: 2,
                subagents: 0,
                model: Some("claude-opus-5-5".into()),
            })
        );
        assert!(first.openable());
        assert_eq!(
            first.line(),
            format!(
                "{}–{} · 1m27s · 2 tools · claude-opus-5-5 · a1111111",
                clock(at("08:08:02")),
                clock(at("08:09:30"))
            )
        );
        assert_eq!(standing(&agents, "a2222222"), Standing::Inferred);
        let heading = agents.heading();
        assert!(heading[1].starts_with("Inferred, not published by no-mistakes"));
        assert!(heading[1].ends_with("No agent is tied to a step."));
    }

    #[test]
    fn what_the_join_cannot_prove_says_why_and_is_not_openable() {
        let agents = join(RUN, life(), roots(Some(&fixture())), read);
        assert_eq!(
            standing(&agents, "a3333333"),
            Standing::Ambiguous(format!("also filed under run {OTHER}"))
        );
        assert!(matches!(
            standing(&agents, "a4444444"),
            Standing::Outside(why) if why.contains("before the run was created")
        ));
        assert!(matches!(
            standing(&agents, "a6666666"),
            Standing::Outside(why) if why.contains("after a read found the run ended")
        ));
        assert_eq!(standing(&agents, "a5555555"), Standing::Undated);
        for agent in &agents.agents {
            assert_eq!(agent.openable(), agent.standing == Standing::Inferred);
            assert_eq!(
                agent.line().contains(" · ambiguous: "),
                agent.session.starts_with("a3333333")
            );
        }
        // The other run's view of the same session is just as ambiguous.
        let other = Life {
            created: created(OTHER).unwrap(),
            ended_by: None,
        };
        let agents = join(OTHER, other, roots(Some(&fixture())), read);
        assert_eq!(
            standing(&agents, "a3333333"),
            Standing::Ambiguous(format!("also filed under run {RUN}"))
        );
        assert_eq!(standing(&agents, "b1111111"), Standing::Inferred);
        // Still running: nothing ends its life, so a late record is its own.
        assert_eq!(agents.life.ended_by, None);
    }

    #[test]
    fn a_run_without_transcripts_says_so() {
        let run = "01M342ZZZZZZZZZZZZZZZZZZZZ";
        let life = Life {
            created: created(run).unwrap(),
            ended_by: None,
        };
        let agents = join(run, life, roots(Some(&fixture())), read);
        assert!(agents.agents.is_empty());
        assert!(
            agents
                .heading()
                .iter()
                .any(|l| l.starts_with("No transcript"))
        );
        // A directory that does not exist is no transcripts, not an error.
        assert!(roots(Some(&fixture().join("missing"))).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_transcript_is_listed_as_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zoe-pipeline-{}", std::process::id()));
        let project = dir.join(format!("-Users-x--no-mistakes-worktrees-r-{RUN}"));
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("a7777777-0000-4000-8000-000000000007.jsonl");
        std::fs::copy(
            fixture()
                .join(format!(
                    "-Users-synthetic--no-mistakes-worktrees-5e1f0000aaaa-{RUN}"
                ))
                .join("a1111111-0000-4000-8000-000000000001.jsonl"),
            &file,
        )
        .unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = std::fs::File::open(&file).is_err(); // not when run as root
        let agents = join(RUN, life(), roots(Some(&dir)), read);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        if denied {
            assert!(matches!(
                standing(&agents, "a7777777"),
                Standing::Unreadable(_)
            ));
            assert!(!agents.agents[0].openable());
            assert!(agents.agents[0].line().contains("unreadable: "));
        }
    }
}
