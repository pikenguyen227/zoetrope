//! Multi-session membership and display projection. Native facts stay in their
//! own App; only display IDs are namespaced. Firstmate is an external adapter.

pub mod actions;
pub mod journal;
pub mod pipeline;
pub mod timeline;
mod validation;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rataflow::{Edge, Reconnectable};
use serde::{Deserialize, Serialize};

use crate::state::session::{AgentInfo, AgentKind, AgentStatus, MAIN_ID, SessionModel};
use crate::state::{App, Camera, Mode, graph};
use crate::tailer::UiEvent;
use crate::ui::nodes::{CrewMark, CrewTone, ValidationBand};
pub use actions::{Action, Prompt, Request, Target};
use journal::{Attempt, AttemptState, Lifecycle};

#[cfg(feature = "native")]
pub mod native;

pub const SCHEMA: &str = "zoetrope.fleet.v1";
const FLEET_ROOT: &str = "@fleet";

/// Exact native identity, never a pane ID, guessed path or session prefix.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionKey {
    pub provider: String,
    pub session_id: String,
}

impl SessionKey {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.provider.as_str(), "claude" | "codex") {
            return Err(format!("unsupported provider {:?}", self.provider));
        }
        nonempty(&self.session_id, "session_id")
    }

    /// Tuple encoding is unambiguous even if an ID contains punctuation.
    pub fn node_id(&self, agent: &str) -> String {
        serde_json::to_string(&(&self.provider, &self.session_id, agent)).unwrap()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSpec {
    pub key: SessionKey,
    pub label: String,
    /// Optional exact fixture/export file, relative to the manifest directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// The adapter's runtime for a session no attempt names, such as a
    /// Captain: `not observed` once a later Captain took its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Observation>,
    /// Where a Captain or a secondmate sits in Herdr. Display only: the
    /// session key is the identity, and the label already carries the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<Place>,
}

/// A Herdr pane by Herdr's own IDs, which renaming a tab or a workspace
/// never changes, with the names they last had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub value: String,
    pub source: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub spawn_gen: String,
    pub label: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub session: Option<SessionKey>,
    pub state: Observation,
    #[serde(default)]
    pub runtime: Option<Observation>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Delegates,
    Continues,
    DependsOn,
}

impl Relation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delegates => "delegates",
            Self::Continues => "continues",
            Self::DependsOn => "depends on",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub id: String,
    pub from: SessionKey,
    pub to: SessionKey,
    pub kind: Relation,
    pub evidence: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub fleet_id: String,
    pub label: String,
    pub observed_at: DateTime<Utc>,
    pub sessions: Vec<SessionSpec>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub links: Vec<Link>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

fn nonempty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(format!(
            "{field} must be nonempty text without control characters"
        ))
    } else {
        Ok(())
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!("expected schema {SCHEMA}"));
        }
        nonempty(&self.fleet_id, "fleet_id")?;
        nonempty(&self.label, "label")?;
        if self.sessions.len() > 128 || self.tasks.len() > 4096 || self.links.len() > 4096 {
            return Err("fleet exceeds supported membership limits".into());
        }
        let mut keys = BTreeSet::new();
        for spec in &self.sessions {
            spec.key.validate()?;
            nonempty(&spec.label, "session label")?;
            if !keys.insert(&spec.key) {
                return Err(format!("duplicate session {:?}", spec.key));
            }
        }
        let mut attempts = BTreeSet::new();
        for task in &self.tasks {
            nonempty(&task.id, "task id")?;
            nonempty(&task.spawn_gen, "spawn_gen")?;
            if !attempts.insert((&task.id, &task.spawn_gen)) {
                return Err("duplicate task attempt".into());
            }
            if let Some(key) = &task.session
                && !keys.contains(key)
            {
                return Err(format!(
                    "task {} refers to an unregistered session",
                    task.id
                ));
            }
        }
        let mut links = BTreeSet::new();
        for link in &self.links {
            nonempty(&link.id, "link id")?;
            nonempty(&link.evidence, "link evidence")?;
            if !links.insert(&link.id) || link.from == link.to {
                return Err("duplicate link ID or self-link".into());
            }
            if !keys.contains(&link.from) || !keys.contains(&link.to) {
                return Err(format!(
                    "link {} refers to an unregistered session",
                    link.id
                ));
            }
        }
        Ok(())
    }
}

pub struct Member {
    pub spec: SessionSpec,
    pub app: App,
    pub loaded: bool,
    pub error: Option<String>,
    pub retained: bool,
}

impl Member {
    /// Whether its session has recorded anything by its playhead.
    fn active(&self) -> bool {
        self.app.session.last_activity.is_some()
    }

    /// Whether the adapter no longer registers it: gone from the manifest, or
    /// an earlier Captain the manifest reads as `not observed`.
    fn unregistered(&self) -> bool {
        self.retained
            || self
                .spec
                .runtime
                .as_ref()
                .is_some_and(|r| r.value == NOT_OBSERVED)
    }
}

/// An overview plus independent, fully functional session inspectors.
pub struct Fleet {
    pub manifest: Manifest,
    pub members: BTreeMap<SessionKey, Member>,
    pub overview: App,
    pub focused: Option<SessionKey>,
    pub manifest_error: Option<String>,
    /// Lifecycle facts from the adapter's journal, deduplicated.
    pub lifecycle: Lifecycle,
    /// The shared playhead's bookkeeping (see [`timeline`]).
    pub timeline: timeline::FleetTimeline,
    pub collapsed: BTreeSet<SessionKey>,
    pub(crate) nodes: BTreeMap<String, (SessionKey, String)>,
    /// Members whose loaded history the overview's chip tray has absorbed, so
    /// a backfill arriving does not replay as a burst of fresh chips.
    baselined: BTreeSet<SessionKey>,
    /// Where each node last stood, so one that leaves on a scrub back returns
    /// to the same place instead of a fresh grid slot.
    positions: BTreeMap<String, (f64, f64)>,
    /// Draw finished members and attempt cards (see `finished`); hidden by
    /// default, so the graph fits the crew still at work.
    pub show_finished: bool,
    /// How many finished members and cards the last sync found at its moment,
    /// drawn or not.
    pub finished: usize,
    /// What the live edge left out as finished when last synced there: a
    /// change re-arranges the crew that remains.
    live_hidden: Option<BTreeSet<String>>,
    /// Re-arrange on the next sync: the visible crew changed on request.
    rearrange: bool,
    /// Attempt cards without a session, by node ID, so actions can name them.
    pub(crate) cards: BTreeMap<String, Attempt>,
    /// A question or note the header shows until the next key.
    pub prompt: Option<Prompt>,
    /// Whether a collector carries out archive and delete requests; the
    /// viewer hands one over by exiting (see [`actions`]).
    pub collector: bool,
    /// The request to hand the collector on exit.
    pub request: Option<Request>,
    /// A validation run's pipeline agents, listed on demand (see [`pipeline`]).
    pub picker: Option<pipeline::Picker>,
    /// A pipeline agent's transcript, open read-only: never a member.
    pub inspection: Option<pipeline::Inspection>,
    /// Where pipeline transcripts are looked for, a directory of Claude
    /// project directories; `None`: Claude's own.
    pub pipeline_root: Option<PathBuf>,
    /// Why this viewer is not the installed build: one installed over it
    /// since it started, so it still runs the old code (see `native`).
    pub stale_build: Option<String>,
}

/// The adapter's runtime for a task that left the Firstmate snapshot.
const NOT_OBSERVED: &str = "not observed";

/// Whether the adapter, when it last observed, no longer saw this task.
fn left(task: &Task) -> bool {
    task.runtime
        .as_ref()
        .is_some_and(|r| r.value == NOT_OBSERVED)
}

/// Whether a member has finished as of `at` (`None`: the live edge): every
/// attempt the journal or manifest joins to it was torn down by then, or, at
/// the live edge, the adapter no longer registers it: its session is gone
/// from the manifest, or every task joined to it is `not observed`. A member
/// that is only idle or quiet has not finished, and neither has one with no
/// attempt to its name, such as a Captain, while it is still registered.
fn finished(
    key: &SessionKey,
    unregistered: bool,
    at: Option<DateTime<Utc>>,
    crew: &BTreeMap<Attempt, AttemptState>,
    joins: &BTreeMap<Attempt, SessionKey>,
    tasks: &[Task],
) -> bool {
    let mut attempts = crew
        .iter()
        .filter(|(attempt, _)| joins.get(*attempt) == Some(key))
        .peekable();
    let torn_down = attempts.peek().is_some() && attempts.all(|(_, s)| s.torn_down.is_some());
    let mut joined = tasks
        .iter()
        .filter(|t| t.session.as_ref() == Some(key))
        .peekable();
    let unobserved = joined.peek().is_some() && joined.all(left);
    // Registration is today's fact; the past reads the journal alone.
    torn_down || (at.is_none() && (unregistered || unobserved))
}

/// Whether an attempt card has finished as of `at`: torn down by then, or, at
/// the live edge, its task is `not observed`.
fn card_finished(
    attempt: &Attempt,
    at: Option<DateTime<Utc>>,
    crew: &BTreeMap<Attempt, AttemptState>,
    tasks: &[Task],
) -> bool {
    crew.get(attempt).is_some_and(|s| s.torn_down.is_some())
        || (at.is_none()
            && tasks
                .iter()
                .any(|t| t.id == attempt.task && t.spawn_gen == attempt.spawn_gen && left(t)))
}

/// A clock time for badges and details, in the viewer's zone.
fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

/// A status and when it was recorded, with how that time is known.
fn describe(state: &AttemptState) -> String {
    let mut text = match &state.status {
        Some(s) => {
            let tag = s.quality.tag();
            let tag = if tag.is_empty() {
                String::new()
            } else {
                format!(" ({tag})")
            };
            format!("{} at {}{tag}", s.value, clock(s.at))
        }
        None => "spawned, no status yet".into(),
    };
    if let Some((at, _)) = state.torn_down {
        text.push_str(&format!(" · torn down {}", clock(at)));
    }
    text
}

/// The card badge for an attempt. Outside coverage it keeps the last record
/// but reads unknown: it may have changed unseen.
fn crew_mark(state: &AttemptState, covered: bool) -> CrewMark {
    let (label, mut tone) = match &state.status {
        Some(s) => (
            format!("{} {}", s.value, clock(s.at)),
            match s.value.as_str() {
                "needs-decision" | "blocked" => CrewTone::Attention,
                "failed" => CrewTone::Failed,
                "done" => CrewTone::Settled,
                "working" => CrewTone::Active,
                _ => CrewTone::Quiet,
            },
        ),
        None => ("spawned".into(), CrewTone::Quiet),
    };
    if state.needs_attention() {
        tone = CrewTone::Attention;
    }
    if !covered {
        tone = CrewTone::Unknown;
    }
    let dimmed = state.torn_down.is_some();
    CrewMark {
        label: if dimmed {
            format!("{label} · torn down")
        } else {
            label
        },
        tone,
        dimmed,
    }
}

impl Fleet {
    pub fn new(manifest: Manifest) -> Result<Self, String> {
        manifest.validate()?;
        let mut overview = App::new(manifest.fleet_id.clone(), Mode::Live);
        overview.wall_clock = true;
        let mut fleet = Self {
            overview,
            manifest: manifest.clone(),
            members: BTreeMap::new(),
            focused: None,
            manifest_error: None,
            lifecycle: Lifecycle::default(),
            timeline: timeline::FleetTimeline::default(),
            collapsed: BTreeSet::new(),
            nodes: BTreeMap::new(),
            baselined: BTreeSet::new(),
            positions: BTreeMap::new(),
            show_finished: false,
            finished: 0,
            live_hidden: None,
            rearrange: false,
            cards: BTreeMap::new(),
            prompt: None,
            collector: false,
            request: None,
            picker: None,
            inspection: None,
            pipeline_root: None,
            stale_build: None,
        };
        fleet.update(manifest)?;
        Ok(fleet)
    }

    /// A malformed refresh is rejected before touching any current state.
    pub fn update(&mut self, manifest: Manifest) -> Result<(), String> {
        manifest.validate()?;
        if manifest.fleet_id != self.manifest.fleet_id {
            return Err("refusing to switch fleet identity during refresh".into());
        }
        for member in self.members.values_mut() {
            member.retained = true;
        }
        for spec in &manifest.sessions {
            let member = self
                .members
                .entry(spec.key.clone())
                .or_insert_with(|| Member {
                    spec: spec.clone(),
                    app: App::new(spec.key.session_id.clone(), Mode::Live),
                    loaded: false,
                    error: None,
                    retained: false,
                });
            member.spec = spec.clone();
            member.retained = false;
        }
        self.manifest = manifest;
        self.manifest_error = None;
        self.sync();
        Ok(())
    }

    pub fn event(&mut self, key: &SessionKey, event: UiEvent) {
        let Some(member) = self.members.get_mut(key) else {
            return;
        };
        let id = match &event {
            UiEvent::Batch { session_id, .. }
            | UiEvent::ReplayLoaded { session_id, .. }
            | UiEvent::SessionReset { session_id } => Some(session_id),
            UiEvent::Error(_) => None,
        };
        if id.is_some_and(|id| id != &key.session_id) {
            member.error = Some("transcript identity changed; refusing unrelated session".into());
            return;
        }
        match &event {
            UiEvent::Error(error) => member.error = Some(error.clone()),
            UiEvent::SessionReset { .. } => {
                member.loaded = false;
                self.baselined.remove(key);
            }
            _ => {
                member.loaded = true;
                member.error = None;
                member.app.last_error = None;
            }
        }
        if matches!(event, UiEvent::Batch { .. }) {
            // The fleet reads live while any member is still being appended to.
            self.overview.last_batch_at = Some(web_time::Instant::now());
        }
        member.app.handle_ui_event(event);
    }

    /// Apply what the journal reader found; `reset` when the journal it read
    /// before was replaced.
    pub fn absorb_lifecycle(
        &mut self,
        reset: bool,
        events: Vec<journal::LifecycleEvent>,
        rejected: usize,
    ) -> bool {
        if reset {
            self.lifecycle.reset();
        }
        self.lifecycle.rejected += rejected;
        self.lifecycle.insert(events) || reset
    }

    /// Each attempt's native session: the manifest's joins and the journal's.
    pub(crate) fn joins(&self) -> BTreeMap<Attempt, SessionKey> {
        let mut joins = self.lifecycle.sessions();
        for task in &self.manifest.tasks {
            if let Some(key) = &task.session {
                joins.insert(
                    Attempt {
                        task: task.id.clone(),
                        spawn_gen: task.spawn_gen.clone(),
                    },
                    key.clone(),
                );
            }
        }
        joins
    }

    /// Render-only union. Never fold into this model: its cloned native agents
    /// retain their private call indexes/dedup stores within their own scope.
    ///
    /// Members are already at the fleet playhead, so the union is as of that
    /// moment. Lifecycle is read as of it too: at the live edge from the whole
    /// journal beside the manifest's current observations; parked in the past
    /// from the journal alone, since the manifest describes the present.
    pub fn sync(&mut self) {
        self.refresh_timeline();
        let at = self.at();
        let crew = self.lifecycle.state_at(at);
        let covered = self.covered();
        let joins = self.joins();
        let mut marks: BTreeMap<String, CrewMark> = BTreeMap::new();
        // Validation runs as of the moment, and each card's band.
        let runs = self.lifecycle.validation_at(at);
        let now = at.unwrap_or_else(Utc::now);
        let mut bands: BTreeMap<String, ValidationBand> = BTreeMap::new();
        // The attempt that speaks for a session: the newest one standing.
        let attempt_for = |key: &SessionKey| {
            crew.iter()
                .filter(|(attempt, _)| joins.get(*attempt) == Some(key))
                .max_by_key(|(_, state)| (state.torn_down.is_none(), state.spawned))
        };
        let mut projection = SessionModel::new(self.manifest.fleet_id.clone());
        projection.agents.clear();
        projection.spawn_order.clear();
        self.nodes.clear();
        self.cards.clear();
        // Node IDs of finished members and cards at this moment; left out of
        // the projection unless shown, so they are neither laid out nor drawn.
        let mut done = BTreeSet::new();
        let mut home = AgentInfo::new(AgentKind::Group);
        home.agent_type = Some(self.manifest.label.clone());
        home.description =
            Some("Fleet membership · connections do not imply native subagents".into());
        home.status = AgentStatus::Idle;
        projection.agents.insert(FLEET_ROOT.into(), home);
        projection.spawn_order.push_back(FLEET_ROOT.into());
        for (key, member) in &self.members {
            // In the past, a session that has not said anything yet by then
            // is not there: its model holds only the root that names it.
            if at.is_some() && !member.active() {
                continue;
            }
            if finished(
                key,
                member.unregistered(),
                at,
                &crew,
                &joins,
                &self.manifest.tasks,
            ) {
                done.insert(key.node_id(MAIN_ID));
                if !self.show_finished {
                    continue;
                }
            }
            for local in member.app.session.spawn_order() {
                if local != MAIN_ID && self.collapsed.contains(key) {
                    continue;
                }
                let mut agent = member.app.session.agent(local).unwrap().clone();
                let id = key.node_id(local);
                agent.parent = agent.parent.as_deref().map(|p| key.node_id(p));
                if local == MAIN_ID {
                    agent.agent_type = Some(member.spec.label.clone());
                    if member.error.is_some() {
                        agent.agent_type = Some(format!("{} · unavailable", member.spec.label));
                    }
                    let mut detail = vec![format!("{} · {}", key.provider, key.session_id)];
                    if at.is_none() {
                        for task in self
                            .manifest
                            .tasks
                            .iter()
                            .filter(|t| t.session.as_ref() == Some(key))
                        {
                            detail.push(format!(
                                "task {} [{}]: {} ({}, {})",
                                task.id,
                                task.spawn_gen,
                                task.state.value,
                                task.state.source,
                                task.state.observed_at
                            ));
                            if let Some(runtime) = &task.runtime {
                                detail.push(format!(
                                    "runtime: {} ({})",
                                    runtime.value, runtime.source
                                ));
                            }
                        }
                    } else {
                        // The manifest is today's; the past reads from the journal.
                        for (attempt, state) in
                            crew.iter().filter(|(a, _)| joins.get(*a) == Some(key))
                        {
                            detail.push(format!(
                                "task {} [{}]: {}",
                                attempt.task,
                                attempt.spawn_gen,
                                describe(state)
                            ));
                        }
                        if !covered {
                            detail.push(
                                "no lifecycle coverage here: last records, unverified".into(),
                            );
                        }
                    }
                    if let Some(error) = &member.error {
                        detail.push(format!("UNAVAILABLE: {error}"));
                    } else if !member.loaded {
                        detail.push("waiting for transcript".into());
                    }
                    if member.retained && at.is_none() {
                        detail.push("retained history · no longer registered".into());
                    } else if member.unregistered() && at.is_none() {
                        detail.push("no longer registered".into());
                    }
                    agent.description = Some(detail.join("\n"));
                    if !member.loaded || member.error.is_some() {
                        // This is a display placeholder, not an Ended fact.
                        agent.status = AgentStatus::Idle;
                    }
                    let joined = joins.iter().filter(|(_, k)| *k == key).map(|(a, _)| a);
                    if let Some(band) =
                        validation::band_for(&self.lifecycle, &runs, joined, at, now)
                    {
                        bands.insert(id.clone(), band);
                    }
                    if let Some((_, state)) = attempt_for(key) {
                        marks.insert(id.clone(), crew_mark(state, covered));
                    } else if at.is_none() && member.unregistered() {
                        // An earlier Captain has no attempt to be torn down;
                        // shown, it reads as gone all the same.
                        marks.insert(
                            id.clone(),
                            CrewMark {
                                label: "no longer registered".into(),
                                tone: CrewTone::Quiet,
                                dimmed: true,
                            },
                        );
                    }
                }
                projection.last_activity = projection.last_activity.max(agent.last_ts);
                projection.agents.insert(id.clone(), agent);
                projection.spawn_order.push_back(id.clone());
                self.nodes.insert(id, (key.clone(), local.to_owned()));
            }
        }
        // Attempts with no transcript to show are cards of their own. Live,
        // the manifest lists them; in the past, the journal does, and an
        // attempt not yet spawned by then is not there at all.
        let unbound: Vec<(Attempt, String, String)> = if at.is_none() {
            self.manifest
                .tasks
                .iter()
                .filter(|t| t.session.is_none())
                .map(|task| {
                    let attempt = Attempt {
                        task: task.id.clone(),
                        spawn_gen: task.spawn_gen.clone(),
                    };
                    let detail = format!(
                        "waiting for session registration\ntask: {} ({})",
                        task.state.value, task.state.source
                    );
                    (attempt, task.label.clone(), detail)
                })
                .collect()
        } else {
            crew.iter()
                .filter(|(attempt, _)| {
                    joins
                        .get(*attempt)
                        .is_none_or(|key| self.members.get(key).is_none_or(|m| !m.active()))
                })
                .map(|(attempt, state)| {
                    let label = self
                        .manifest
                        .tasks
                        .iter()
                        .find(|t| t.id == attempt.task && t.spawn_gen == attempt.spawn_gen)
                        .map_or_else(|| attempt.task.clone(), |t| t.label.clone());
                    let mut detail =
                        format!("no transcript at this moment\ntask: {}", describe(state));
                    if !covered {
                        detail.push_str("\nno lifecycle coverage here: last record, unverified");
                    }
                    (attempt.clone(), label, detail)
                })
                .collect()
        };
        for (attempt, label, detail) in unbound {
            let id = serde_json::to_string(&("task", &attempt.task, &attempt.spawn_gen)).unwrap();
            if card_finished(&attempt, at, &crew, &self.manifest.tasks) {
                done.insert(id.clone());
                if !self.show_finished {
                    continue;
                }
            }
            self.cards.insert(id.clone(), attempt.clone());
            let mut agent = AgentInfo::new(AgentKind::Main);
            agent.agent_type = Some(label);
            agent.status = AgentStatus::Idle;
            agent.description = Some(detail);
            if let Some(state) = crew.get(&attempt) {
                marks.insert(id.clone(), crew_mark(state, covered));
            }
            if let Some(band) =
                validation::band_for(&self.lifecycle, &runs, [&attempt].into_iter(), at, now)
            {
                bands.insert(id.clone(), band);
            }
            projection.agents.insert(id.clone(), agent);
            projection.spawn_order.push_back(id);
        }
        self.finished = done.len();
        let hidden = if self.show_finished {
            BTreeSet::new()
        } else {
            done
        };
        // At the live edge, a worker finishing (or the toggle) re-arranges the
        // crew that remains, once. Scrubbing only moves the playhead: nodes
        // come and go in place, as sessions not yet started do.
        let arrange = std::mem::take(&mut self.rearrange)
            || (at.is_none()
                && self
                    .live_hidden
                    .replace(hidden.clone())
                    .is_some_and(|old| old != hidden));
        // The crew root reads busy while any member agent is working.
        let busy = projection
            .agents
            .values()
            .any(|a| a.status == AgentStatus::Running);
        if let Some(home) = projection.agents.get_mut(FLEET_ROOT) {
            home.status = if busy {
                AgentStatus::Running
            } else {
                AgentStatus::Idle
            };
        }
        // Absorb each member's history once it loads: the tray counts calls by
        // display ID, and only calls after this mark are fresh activity.
        for (key, member) in &self.members {
            if member.loaded && self.baselined.insert(key.clone()) {
                let model = &member.app.session;
                self.overview
                    .chips
                    .adopt_agents(model.spawn_order().map(|local| {
                        let calls = model.agent(local).map_or(0, |a| a.tool_calls.len());
                        (key.node_id(local), calls)
                    }));
            }
        }
        self.overview
            .flow
            .retain_nodes(|n| projection.agent(&n.id).is_some());
        // Native parent changes and link edits must remove stale edges too.
        let mut edges = BTreeSet::new();
        for id in projection.spawn_order() {
            if projection.agent(id).unwrap().parent.is_some() {
                edges.insert(format!("e-{id}"));
            }
        }
        edges.extend(
            self.manifest
                .links
                .iter()
                .map(|l| format!("fleet-link:{}", l.id)),
        );
        let member_edges: BTreeMap<_, _> = self
            .members
            .keys()
            .filter(|key| {
                !self
                    .manifest
                    .links
                    .iter()
                    .any(|l| &l.to == *key && l.kind != Relation::DependsOn)
            })
            .map(|key| {
                (
                    format!("member:{}", key.node_id(MAIN_ID)),
                    key.node_id(MAIN_ID),
                )
            })
            .collect();
        edges.extend(member_edges.keys().cloned());
        self.overview.flow.retain_edges(|e| {
            edges.contains(&e.id)
                && (!e.id.starts_with("e-")
                    || projection
                        .agent(&e.target)
                        .and_then(|a| a.parent.as_deref())
                        == Some(e.source.as_str()))
        });
        let before: BTreeSet<_> = self.overview.flow.nodes().map(|n| n.id.clone()).collect();
        let mut structural = graph::sync(&mut self.overview.flow, &projection, false);
        for (id, target) in member_edges {
            let running = projection
                .agent(&target)
                .is_some_and(|a| a.status == AgentStatus::Running);
            if !self.overview.flow.edges().iter().any(|e| e.id == id) {
                // A member with nothing to show yet has no node to hang from.
                if projection.agent(&target).is_none() {
                    continue;
                }
                structural |= self
                    .overview
                    .flow
                    .add_edge(
                        Edge::new(id.clone(), FLEET_ROOT, target)
                            .with_label("member")
                            .with_selectable(false)
                            .with_deletable(false)
                            .with_reconnectable(Reconnectable::None),
                    )
                    .is_ok();
            }
            // A working member reads on its edge, as a native child does.
            if let Some(content) = self.overview.flow.edge_content_mut(&id) {
                content.running = running;
            }
            self.overview.flow.set_edge_animated(&id, running);
        }
        // New independent roots otherwise all land at (0,0). A node seen
        // before returns to where it stood; a new one gets a stable local
        // slot, with an explicit `r` for a full graph layout.
        for (index, id) in projection.spawn_order().enumerate() {
            if before.contains(id) {
                continue;
            }
            if let Some(&position) = self.positions.get(id) {
                self.overview.flow.set_node_position(id, position);
            } else if projection.agent(id).unwrap().parent.is_none() {
                self.overview
                    .flow
                    .set_node_position(id, ((index % 3) as f64 * 68.0, (index / 3) as f64 * 25.0));
            }
        }
        for link in &self.manifest.links {
            let id = format!("fleet-link:{}", link.id);
            // Reconcile endpoints as well as label; unchanged edges stay put.
            let from = link.from.node_id(MAIN_ID);
            let to = link.to.node_id(MAIN_ID);
            let same = self.overview.flow.edges().iter().any(|e| {
                e.id == id
                    && e.source == from
                    && e.target == to
                    && e.label.as_deref() == Some(link.kind.label())
            });
            if !same {
                self.overview.flow.remove_edge(&id);
                let edge = Edge::new(id.clone(), from, to.clone())
                    .with_label(link.kind.label())
                    .with_selectable(false)
                    .with_deletable(false)
                    .with_reconnectable(Reconnectable::None);
                structural |= self.overview.flow.add_edge(edge).is_ok();
            }
            let running = projection
                .agent(&to)
                .is_some_and(|a| a.status == AgentStatus::Running);
            if let Some(content) = self.overview.flow.edge_content_mut(&id) {
                content.running = running;
            }
            self.overview.flow.set_edge_animated(&id, running);
        }
        let after: BTreeSet<_> = self.overview.flow.nodes().map(|n| n.id.clone()).collect();
        structural |= after != before;
        for id in &after {
            if let Some(content) = self.overview.flow.node_content_mut(id) {
                content.crew = marks.remove(id);
                content.validation = bands.remove(id);
            }
            if let Some(node) = self.overview.flow.node(id) {
                self.positions
                    .insert(id.clone(), (node.position.x, node.position.y));
            }
        }
        self.overview.session = projection;
        self.overview.session_info.title = Some(self.manifest.label.clone());
        self.overview.layout_dirty |= structural;
        let jumped = std::mem::take(&mut self.timeline.jumped);
        if (before.is_empty() && structural) || arrange {
            self.overview.relayout_now();
        } else {
            // The overview never folds, so the camera follow-up a fold runs
            // for a single session (`App::commit_fold`) happens here. A scrub
            // is time navigation, not growth: it never reframes, as upstream.
            match self.overview.camera {
                Camera::Overview if structural && !jumped => self.overview.flow.request_fit_view(),
                Camera::Follow => self.overview.track_activity(),
                _ => {}
            }
        }
        if jumped {
            // What already happened at the new moment is history, not a burst.
            self.overview.chips.adopt_baseline(&self.overview.session);
        }
        self.overview.last_error = self.manifest_error.clone();
    }

    pub fn inspect_selected(&mut self) {
        if let Some(id) = self.overview.selected_agent_id()
            && let Some((key, local)) = self.nodes.get(&id)
            && let Some(member) = self.members.get_mut(key)
        {
            member.app.flow.select_node(local);
            member.app.pending_center = Some(local.clone());
            // Its tray was not reconciled while unfocused: absorb what already
            // happened instead of replaying it as a burst.
            member.app.chips.adopt_baseline(&member.app.session);
            self.focused = Some(key.clone());
        }
    }

    pub fn toggle_children(&mut self) {
        if let Some(id) = self.overview.selected_agent_id()
            && let Some((key, _)) = self.nodes.get(&id)
        {
            let key = key.clone();
            let expanded = self.collapsed.remove(&key);
            if !expanded {
                self.collapsed.insert(key.clone());
            }
            self.sync();
            // Collapsed subagents were outside the projection, so their marks
            // froze: what they did meanwhile is history, not a burst.
            if expanded && let Some(member) = self.members.get(&key) {
                let model = &member.app.session;
                self.overview.chips.adopt_agents(
                    model
                        .spawn_order()
                        .filter(|local| *local != MAIN_ID)
                        .map(|local| {
                            let calls = model.agent(local).map_or(0, |a| a.tool_calls.len());
                            (key.node_id(local), calls)
                        }),
                );
            }
        }
    }

    pub fn back(&mut self) {
        let left = self.focused.take();
        let at = self.at();
        if let Some(key) = &left
            && let Some(member) = self.members.get_mut(key)
        {
            // Rejoin the fleet's moment, wherever the session's own DVR went.
            match at {
                None => member.app.go_live(),
                Some(t) => {
                    if !member.app.park_at(t) {
                        member.app.status_tick();
                    }
                }
            }
        }
        self.sync();
        // The overview tray was not reconciled while a member was focused.
        if left.is_some() {
            self.overview.chips.adopt_baseline(&self.overview.session);
        }
    }

    pub fn active(&mut self) -> &mut App {
        if let Some(open) = &mut self.inspection {
            &mut open.app
        } else if let Some(key) = &self.focused {
            &mut self.members.get_mut(key).unwrap().app
        } else {
            &mut self.overview
        }
    }
}

#[cfg(test)]
mod tests;
