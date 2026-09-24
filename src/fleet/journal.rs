//! The fleet lifecycle journal, `zoetrope.fleet.journal.v2`: timestamped facts
//! about Firstmate task attempts (spawned, bound to a native session, status,
//! torn down, ...) and the windows in which anyone was watching for them.
//!
//! The adapter writes these lines beside its manifest; the core reads only this
//! schema and never a Firstmate file. A line states one fact and how its time is
//! known ([`AtQuality`]). The store is a set keyed by event ID, so a re-read, an
//! adapter restart that re-emits, or lines in any order converge on one state.
//!
//! Two sources write these facts. Firstmate's own lifecycle feed records every
//! transition at its real time, whether or not anyone watches. Where no feed
//! speaks, the adapter bridges from successive snapshots, which only sees what
//! it polls while it runs. Where the feed records the same fact, it replaces
//! the bridge's (`Lifecycle::superseded`), so nothing is counted twice and
//! the feed's times win.
//!
//! Coverage is what keeps the timeline honest: a moment outside every recorded
//! [`Change::Coverage`] window is a gap, where the last record before it is
//! shown as unverified, never as a reconstruction.
//!
//! The adapter also journals the no-mistakes validation run Firstmate
//! attributes to an attempt ([`Change::Validation`]), read through the
//! no-mistakes CLI, with its own coverage ([`Change::ValidationCoverage`]):
//! lifecycle coverage says nothing about whether anyone was reading a run.
//! A viewer that predates these types skips them as lines it does not know.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{SessionKey, nonempty};

pub const JOURNAL_SCHEMA: &str = "zoetrope.fleet.journal.v2";
const MAX_ID: usize = 512;
const MAX_TEXT: usize = 1024;

/// Where a same-second lifecycle event sorts against transcript items: births
/// and joins before them, teardown after. Firstmate stamps whole seconds while
/// transcripts carry milliseconds, so a spawn and its first line often tie.
pub const TRANSCRIPT_RANK: u8 = 2;

/// How an event's `at` is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtQuality {
    /// The author's own emission stamp (a status line's `[at=]`).
    Stamp,
    /// Firstmate's clock at a transition it performed itself.
    Firstmate,
    /// Recovered from an identity that embeds a time (a `spawn_gen` epoch).
    Derived,
    /// When an observer noticed it: the fact happened at or before `at`.
    Observed,
    /// Replayed after the fact from records that survived.
    Backfill,
    /// No time is known; `at` is absent and the event never places on the timeline.
    Unknown,
}

impl AtQuality {
    /// A short tag for details and narration. Exact times carry none.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Stamp | Self::Firstmate => "",
            Self::Derived => "derived",
            Self::Observed => "observed",
            Self::Backfill => "backfill",
            Self::Unknown => "time unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// The adapter's diff of successive Firstmate snapshots.
    Bridge,
    /// A lifecycle feed Firstmate writes itself.
    Firstmate,
    /// The adapter's reads of a no-mistakes run through its CLI.
    NoMistakes,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The adapter run that observed it, for bridge events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

/// A launch attempt: a Firstmate task under one `spawn_gen`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub task: String,
    pub spawn_gen: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawned {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// One status line: its verb (`working`, `needs-decision`, `done`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A digest of the raw line, so a writer can recognise it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// The key of the feed event recording this line, as Firstmate's
    /// snapshot publishes it (`last_event.lifecycle_key`), on a bridged
    /// status whose identity Firstmate could establish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionChange {
    Opened,
    Replaced,
    Closed,
}

/// A keyed decision opening or closing, in Firstmate's own fold semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub key: String,
    pub change: DecisionChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
}

/// Steering metadata only: never a message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Steer {
    pub msg: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reclassified {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TornDown {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Busy {
    pub state: String,
}

/// What a [`Validation`] event records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The run was created: `at` is recovered from its ID (`derived`).
    Started,
    /// A read found the run changed.
    Seen,
    /// A read found a gate newly waiting on a decision.
    Parked,
    /// A read found the run newly completed, failed or cancelled.
    Ended,
    /// The daemon behind the run's record was down: a record still saying
    /// running is a dead instrument's, so the run is unverified.
    DaemonDown,
    /// The run's record could no longer be found.
    Gone,
}

impl Phase {
    /// Whether the event carries the state a read found.
    pub fn is_read(self) -> bool {
        matches!(self, Self::Seen | Self::Parked | Self::Ended)
    }
}

/// One pipeline step as a read found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub step: String,
    /// `pending`, `running`, `fixing`, `awaiting_approval`, `completed`,
    /// `skipped` or `failed`, as no-mistakes words it.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<u32>,
    /// How long a settled step ran, time parked at a gate excluded: a
    /// duration, never a place on the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// An active step's round: `round 1`, `fix 2`, `auto-fix 1/3`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<String>,
    /// When that round began, derived from the age the read gave, to the
    /// second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_since: Option<DateTime<Utc>>,
}

/// A gate waiting on a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub findings: u32,
    /// Findings only the operator may decide.
    #[serde(default)]
    pub ask_user: u32,
}

/// A no-mistakes run, as the adapter read it. `at` is when it read (or, for
/// [`Phase::Started`], the run's creation); what changed, changed after
/// `since`, the read before, and at or before `at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    pub run: String,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<DateTime<Utc>>,
    /// The run record's status word, on a read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    /// Why the run failed, as no-mistakes words it: often several lines.
    /// Each run of control characters folds to a space ([`fold`]), so a line
    /// break costs no more than a space, never the fact the error rides on.
    #[serde(
        default,
        deserialize_with = "folded",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<String>,
}

/// `value` with each run of control characters (line breaks, tabs, escapes)
/// folded to one space and none left at either end.
fn fold(value: &str) -> String {
    let mut folded = String::with_capacity(value.len());
    let mut gap = false;
    for c in value.chars() {
        if c.is_control() {
            gap = true;
            continue;
        }
        if gap && !folded.is_empty() {
            folded.push(' ');
        }
        gap = false;
        folded.push(c);
    }
    folded
}

fn folded<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(d)?;
    Ok(value.map(|v| fold(&v)).filter(|v| !v.trim().is_empty()))
}

impl Validation {
    fn validate(&self, at: Option<DateTime<Utc>>) -> Result<(), String> {
        text(&self.run, "run", 64)?;
        if self.phase.is_read() != self.status.is_some() {
            return Err("a status is required exactly on a read".into());
        }
        if self
            .since
            .is_some_and(|since| at.is_none_or(|at| since > at))
        {
            return Err("since must precede the read".into());
        }
        if self.steps.len() > 64 {
            return Err("too many steps".into());
        }
        for step in &self.steps {
            text(&step.step, "step", 64)?;
            text(&step.status, "step status", 64)?;
            if let Some(round) = &step.round {
                text(round, "round", 64)?;
            }
        }
        if let Some(gate) = &self.gate {
            text(&gate.step, "gate step", 64)?;
        }
        for value in [&self.status, &self.outcome].into_iter().flatten() {
            text(value, "run status", 64)?;
        }
        for value in [&self.branch, &self.pr, &self.error].into_iter().flatten() {
            text(value, "run field", MAX_TEXT)?;
        }
        Ok(())
    }

    /// The step the run is at: the first that is neither settled nor waiting
    /// to start, else the first that failed.
    pub fn current(&self) -> Option<&Step> {
        self.steps
            .iter()
            .find(|s| {
                !matches!(
                    s.status.as_str(),
                    "completed" | "skipped" | "pending" | "failed"
                )
            })
            .or_else(|| self.steps.iter().find(|s| s.status == "failed"))
    }
}

/// An interval in which the adapter was reading one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunWindow {
    pub run: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_gap: Option<u32>,
}

/// A closed interval of wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// The longest pause, in seconds, the adapter still counts as observing:
    /// its cadence, so the viewer knows how stale a running adapter's newest
    /// segment may be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_gap: Option<u32>,
}

/// What an event says. Each payload sits under a key named after its type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    Spawned {
        #[serde(default)]
        spawned: Spawned,
    },
    /// The attempt's native session was joined (Herdr-observed).
    Bound,
    Status {
        status: Status,
    },
    Decision {
        decision: Decision,
    },
    Steered {
        steered: Steer,
    },
    SteerAcked {
        steer_acked: Steer,
    },
    Reclassified {
        reclassified: Reclassified,
    },
    TornDown {
        #[serde(default)]
        torn_down: TornDown,
    },
    Busy {
        busy: Busy,
    },
    /// An interval someone was observing: the bridge while it polled, or a
    /// Firstmate feed from its start up to the adapter's latest read of it.
    Coverage {
        coverage: Window,
    },
    /// The no-mistakes run Firstmate attributed to the attempt, as read.
    Validation {
        validation: Box<Validation>,
    },
    /// An interval in which the adapter was reading that run. Its own type,
    /// so a viewer that predates it can never count it as lifecycle coverage.
    ValidationCoverage {
        validation_coverage: RunWindow,
    },
}

impl Change {
    /// Same-second order: births and joins first, teardown last.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Spawned { .. } => 0,
            Self::Bound => 1,
            Self::TornDown { .. } => 4,
            _ => 3,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Spawned { .. } => "spawned",
            Self::Bound => "bound",
            Self::Status { .. } => "status",
            Self::Decision { .. } => "decision",
            Self::Steered { .. } => "steered",
            Self::SteerAcked { .. } => "steer_acked",
            Self::Reclassified { .. } => "reclassified",
            Self::TornDown { .. } => "torn_down",
            Self::Busy { .. } => "busy",
            Self::Coverage { .. } => "coverage",
            Self::Validation { .. } => "validation",
            Self::ValidationCoverage { .. } => "validation_coverage",
        }
    }

    /// Whether this says when someone was observing rather than what happened.
    pub fn is_coverage(&self) -> bool {
        matches!(
            self,
            Self::Coverage { .. } | Self::ValidationCoverage { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEvent {
    /// Derived from the fact, never from arrival: the same fact always has the
    /// same ID, which is what makes re-reading idempotent.
    pub id: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
    pub at_quality: AtQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionKey>,
    #[serde(flatten)]
    pub change: Change,
}

fn text(value: &str, field: &str, max: usize) -> Result<(), String> {
    nonempty(value, field)?;
    if value.chars().count() > max {
        return Err(format!("{field} exceeds {max} characters"));
    }
    Ok(())
}

impl LifecycleEvent {
    pub fn validate(&self) -> Result<(), String> {
        text(&self.id, "event id", MAX_ID)?;
        if self.at.is_none() != (self.at_quality == AtQuality::Unknown) {
            return Err("at is required exactly when its quality is known".into());
        }
        if let Some(session) = &self.session {
            session.validate()?;
        }
        match &self.change {
            Change::Coverage { coverage } => {
                if self.at.is_none() || coverage.from > coverage.to {
                    return Err("coverage needs a time and from <= to".into());
                }
                return Ok(());
            }
            Change::Bound if self.session.is_none() => {
                return Err("bound needs a session".into());
            }
            Change::Status { status } => {
                text(&status.value, "status value", 64)?;
                for value in [
                    &status.key,
                    &status.note,
                    &status.digest,
                    &status.lifecycle_key,
                ]
                .into_iter()
                .flatten()
                {
                    text(value, "status field", MAX_TEXT)?;
                }
            }
            Change::Decision { decision } => text(&decision.key, "decision key", MAX_TEXT)?,
            Change::Steered { steered: steer } | Change::SteerAcked { steer_acked: steer } => {
                text(&steer.msg, "steer message", MAX_TEXT)?
            }
            Change::Busy { busy } => text(&busy.state, "busy state", 64)?,
            Change::Validation { validation } => validation.validate(self.at)?,
            Change::ValidationCoverage {
                validation_coverage: window,
            } => {
                text(&window.run, "run", 64)?;
                if self.at.is_none() || window.from > window.to {
                    return Err("coverage needs a time and from <= to".into());
                }
            }
            _ => {}
        }
        let attempt = self
            .attempt
            .as_ref()
            .ok_or_else(|| format!("{} needs an attempt", self.change.name()))?;
        text(&attempt.task, "task", MAX_TEXT)?;
        text(&attempt.spawn_gen, "spawn_gen", MAX_TEXT)
    }

    /// The §8.2 timeline order: time, then same-second rank, then source order,
    /// with the ID as the final, total tiebreak.
    fn order(&self) -> impl Ord + '_ {
        (
            self.at,
            self.change.rank(),
            &self.source.home,
            self.source.seq,
            &self.id,
        )
    }
}

#[derive(Deserialize)]
struct Envelope {
    schema: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    event: Option<serde_json::Value>,
}

/// One journal line. `Ok(None)` for lines that are not v2 lifecycle facts: v1
/// and v2 manifest checkpoints, blank lines, other schemas.
pub fn parse_line(line: &[u8]) -> Result<Option<LifecycleEvent>, String> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let envelope: Envelope = serde_json::from_slice(line).map_err(|e| e.to_string())?;
    if envelope.schema != JOURNAL_SCHEMA || envelope.kind.as_deref() != Some("lifecycle") {
        return Ok(None);
    }
    let event: LifecycleEvent = serde_json::from_value(envelope.event.ok_or("missing event")?)
        .map_err(|e| e.to_string())?;
    event.validate()?;
    Ok(Some(event))
}

/// Complete lines at the front of `bytes`, and how many bytes they span. A
/// trailing line without its newline is still being written: it stays unread.
pub fn parse_chunk(bytes: &[u8]) -> (usize, Vec<LifecycleEvent>, usize) {
    let consumed = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let mut events = Vec::new();
    let mut rejected = 0;
    for line in bytes[..consumed].split(|&b| b == b'\n') {
        match parse_line(line) {
            Ok(Some(event)) => events.push(event),
            Ok(None) => {}
            Err(_) => rejected += 1,
        }
    }
    (consumed, events, rejected)
}

/// A status as of some moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusAt {
    pub value: String,
    pub note: Option<String>,
    pub at: DateTime<Utc>,
    pub quality: AtQuality,
}

/// What the journal says about one attempt, as of a moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttemptState {
    pub spawned: Option<(DateTime<Utc>, AtQuality)>,
    pub kind: Option<String>,
    pub project: Option<String>,
    pub status: Option<StatusAt>,
    pub open_decisions: BTreeSet<String>,
    pub torn_down: Option<(DateTime<Utc>, AtQuality)>,
    pub outcome: Option<String>,
}

impl AttemptState {
    /// Waiting on the operator: its last status asks for a decision or reports
    /// a block, or a keyed decision is open.
    pub fn needs_attention(&self) -> bool {
        !self.open_decisions.is_empty()
            || self
                .status
                .as_ref()
                .is_some_and(|s| matches!(s.value.as_str(), "needs-decision" | "blocked"))
    }
}

/// What the journal says about one attempt's validation run, as of a moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    pub run: String,
    /// When the run was created, and how that is known.
    pub started: Option<(DateTime<Utc>, AtQuality)>,
    /// The newest read: when, and what it found.
    pub read: Option<(DateTime<Utc>, Validation)>,
    /// When each step reached what the newest read shows it doing: the read
    /// that first showed it, and the read before that (the change came after).
    pub reached: BTreeMap<String, (DateTime<Utc>, Option<DateTime<Utc>>)>,
    /// A record after the newest read that the reading lost its footing: the
    /// daemon was down, or the record gone.
    pub lost: Option<(Phase, DateTime<Utc>)>,
    /// Its first record, for choosing an attempt's newest run.
    first: DateTime<Utc>,
}

impl RunView {
    /// Whether the newest read found the run ended: its state can no longer
    /// change, read or not.
    pub fn ended(&self) -> bool {
        self.read
            .as_ref()
            .is_some_and(|(_, v)| v.phase == Phase::Ended)
    }

    /// When the run began, as far as the journal knows.
    pub fn start(&self) -> DateTime<Utc> {
        self.started
            .map_or(self.first, |(at, _)| at.min(self.first))
    }
}

/// Merge windows that touch or overlap, ascending.
fn merge(mut windows: Vec<Window>) -> Vec<Window> {
    windows.sort();
    let mut merged: Vec<Window> = Vec::with_capacity(windows.len());
    for window in windows {
        match merged.last_mut() {
            // Segments chain end to start; touching ones merge, and the
            // feed's and the bridge's overlap.
            Some(last) if window.from <= last.to => {
                if window.to >= last.to {
                    last.to = window.to;
                    last.max_gap = window.max_gap;
                }
            }
            _ => merged.push(window),
        }
    }
    merged
}

fn within(windows: &[Window], t: DateTime<Utc>) -> bool {
    let i = windows.partition_point(|w| w.from <= t);
    i > 0 && t <= windows[i - 1].to
}

/// Whether the newest of `windows` still counts as observing at `now`: a
/// running adapter's newest segment trails the present by up to a checkpoint
/// (at most its `max_gap`) plus a poll (within `max_gap`), so twice its
/// `max_gap` of lag still counts, or two minutes for a segment that does not
/// say.
fn fresh(windows: &[Window], now: DateTime<Utc>) -> bool {
    windows.last().is_some_and(|w| {
        let lag = chrono::Duration::seconds(w.max_gap.map_or(60, i64::from) * 2);
        w.from <= now && now - w.to <= lag
    })
}

/// A run's life on the timeline: from its first record until it ended or its
/// record was gone, and the windows in which it was read.
#[derive(Debug)]
struct Reading {
    from: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
    windows: Vec<Window>,
}

/// The deduplicated set of lifecycle events, kept in timeline order.
#[derive(Debug, Default)]
pub struct Lifecycle {
    events: BTreeMap<String, LifecycleEvent>,
    /// Dated events in [`LifecycleEvent::order`]; undated ones never place.
    order: Vec<String>,
    /// Merged coverage windows, ascending and disjoint.
    coverage: Vec<Window>,
    /// Each attempt's validation runs: when they lived and were read.
    readings: BTreeMap<(Attempt, String), Reading>,
    /// Whether coverage applies: something wrote coverage, or the bridge,
    /// which only ever sees part of the time, wrote anything.
    gapped: bool,
    /// Bumped whenever the set changes, so derived indexes know to rebuild.
    pub generation: u64,
    /// Lines that did not validate. They are skipped, never guessed at.
    pub rejected: usize,
}

impl Lifecycle {
    /// Add events; returns whether the set changed. A duplicate ID keeps the
    /// canonically smaller event, so any delivery order converges.
    pub fn insert(&mut self, events: impl IntoIterator<Item = LifecycleEvent>) -> bool {
        let mut changed = false;
        for event in events {
            if event.validate().is_err() {
                self.rejected += 1;
                continue;
            }
            match self.events.get(&event.id) {
                Some(kept) if kept == &event => {}
                Some(kept) if canonical(kept) <= canonical(&event) => {}
                _ => {
                    self.events.insert(event.id.clone(), event);
                    changed = true;
                }
            }
        }
        if changed {
            self.reindex();
        }
        changed
    }

    /// Forget everything: the journal behind it was replaced.
    pub fn reset(&mut self) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            generation,
            ..Self::default()
        };
    }

    fn reindex(&mut self) {
        let superseded = self.superseded();
        let mut dated: Vec<&LifecycleEvent> = self
            .events
            .values()
            .filter(|e| e.at.is_some() && !superseded.contains(e.id.as_str()))
            .collect();
        dated.sort_by(|a, b| a.order().cmp(&b.order()));
        self.order = dated.iter().map(|e| e.id.clone()).collect();
        self.coverage = merge(
            self.events
                .values()
                .filter_map(|e| match e.change {
                    Change::Coverage { coverage } => Some(coverage),
                    _ => None,
                })
                .collect(),
        );
        let mut readings: BTreeMap<(Attempt, String), (Reading, Vec<Window>)> = BTreeMap::new();
        for event in self.ordered() {
            let (Some(attempt), Some(at)) = (&event.attempt, event.at) else {
                continue;
            };
            let (run, window, end) = match &event.change {
                Change::Validation { validation } => (
                    &validation.run,
                    None,
                    matches!(validation.phase, Phase::Ended | Phase::Gone),
                ),
                Change::ValidationCoverage {
                    validation_coverage: w,
                } => (
                    &w.run,
                    Some(Window {
                        from: w.from,
                        to: w.to,
                        max_gap: w.max_gap,
                    }),
                    false,
                ),
                _ => continue,
            };
            let (reading, windows) = readings
                .entry((attempt.clone(), run.clone()))
                .or_insert_with(|| {
                    (
                        Reading {
                            from: at,
                            until: None,
                            windows: Vec::new(),
                        },
                        Vec::new(),
                    )
                });
            reading.from = reading.from.min(window.map_or(at, |w| w.from));
            windows.extend(window);
            if end && reading.until.is_none() {
                reading.until = Some(at);
            }
        }
        self.readings = readings
            .into_iter()
            .map(|(key, (mut reading, windows))| {
                reading.windows = merge(windows);
                (key, reading)
            })
            .collect();
        self.gapped = !self.coverage.is_empty()
            || self
                .events
                .values()
                .any(|e| e.source.kind == SourceKind::Bridge);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Bridge facts a Firstmate feed has recorded too, by ID. They stay in
    /// the store (a removal still counts them) but never place on the
    /// timeline.
    ///
    /// A bridged spawn, teardown or status gives way only to the feed's own
    /// record of it: the attempt's spawn, its teardown, or, for a status, the
    /// dated feed event whose key is the line's `lifecycle_key` (an undated
    /// one never places, so it cannot stand in). A status without
    /// that key falls back to a feed status stamped at the same moment. So a
    /// status the feed lost still places from the bridge, and a session join,
    /// which no feed knows, always does. A status line with neither key nor
    /// stamp has no identity the two sides share, so its bridged record
    /// always places, even beside the feed's. `Reach` in
    /// `scripts/firstmate-fleet.py` keeps the adapter from writing what the
    /// feed already holds.
    fn superseded(&self) -> BTreeSet<&str> {
        #[derive(PartialEq, Eq, PartialOrd, Ord)]
        enum Mark<'a> {
            Fact(&'a Attempt, &'static str, Option<DateTime<Utc>>),
            /// A status line, by its feed key.
            Line(&'a str),
        }
        fn mark(e: &LifecycleEvent) -> Option<Mark<'_>> {
            let attempt = e.attempt.as_ref()?;
            match &e.change {
                Change::Spawned { .. } => Some(Mark::Fact(attempt, "spawned", None)),
                Change::TornDown { .. } => Some(Mark::Fact(attempt, "torn_down", None)),
                Change::Status { .. } if e.at_quality == AtQuality::Stamp => {
                    Some(Mark::Fact(attempt, "status", e.at))
                }
                _ => None,
            }
        }
        // A feed event's ID is `firstmate:<home>#<key>`.
        fn feed_key(e: &LifecycleEvent) -> Option<&str> {
            let home = e.source.home.as_deref()?;
            let key =
                e.id.strip_prefix("firstmate:")?
                    .strip_prefix(home)?
                    .strip_prefix('#')?;
            (!key.is_empty()).then_some(key)
        }
        let mut held = BTreeSet::new();
        for e in self
            .events
            .values()
            .filter(|e| e.source.kind == SourceKind::Firstmate)
        {
            held.extend(mark(e));
            if matches!(e.change, Change::Status { .. }) && e.at.is_some() {
                held.extend(feed_key(e).map(Mark::Line));
            }
        }
        self.events
            .values()
            .filter(|e| e.source.kind == SourceKind::Bridge)
            .filter(|e| {
                let line = match &e.change {
                    Change::Status { status } => status.lifecycle_key.as_deref(),
                    _ => None,
                };
                line.map(Mark::Line)
                    .or_else(|| mark(e))
                    .is_some_and(|m| held.contains(&m))
            })
            .map(|e| e.id.as_str())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Dated events in timeline order.
    pub fn ordered(&self) -> impl Iterator<Item = &LifecycleEvent> {
        self.order.iter().map(|id| &self.events[id])
    }

    pub fn coverage(&self) -> &[Window] {
        &self.coverage
    }

    /// Whether gaps apply at all: something recorded when it was observing,
    /// or the bridge, which only sees what it polls while it runs, wrote. A
    /// journal with neither has nothing a gap could mean.
    pub fn has_gaps(&self) -> bool {
        self.gapped
    }

    /// Whether `t` lies inside a window someone was observing.
    pub fn covered(&self, t: DateTime<Utc>) -> bool {
        !self.gapped || within(&self.coverage, t)
    }

    /// Whether the adapter is still observing at `now`: its newest segment
    /// (bridge or feed) is fresh by the rule of [`fresh`]; past it, the
    /// adapter has stopped.
    pub fn observing(&self, now: DateTime<Utc>) -> bool {
        !self.gapped || fresh(&self.coverage, now)
    }

    /// Whether `run`, attributed to `attempt`, was verified at the moment
    /// shown (`None`: the live edge): parked, whether a window in which the
    /// adapter read it holds `t`; at the live edge, whether it still reads it.
    /// Lifecycle coverage says nothing about this.
    pub fn run_verified(&self, attempt: &Attempt, run: &str, t: Option<DateTime<Utc>>) -> bool {
        self.readings
            .get(&(attempt.clone(), run.to_owned()))
            .is_some_and(|r| match t {
                Some(t) => within(&r.windows, t),
                None => fresh(&r.windows, Utc::now()),
            })
    }

    /// Whether any validation run was alive but unread at `t`: begun, not yet
    /// ended or gone, and outside every window in which it was read.
    pub fn unread(&self, t: DateTime<Utc>) -> bool {
        self.readings
            .values()
            .any(|r| r.from <= t && r.until.is_none_or(|until| t < until) && !within(&r.windows, t))
    }

    /// Whether the crew state at `t` is known as of `now`: someone observed
    /// lifecycle then, and every validation run alive then was read. The
    /// newest window of each still holds whatever follows it while its reader
    /// is fresh ([`fresh`]): a running adapter's windows trail the present by
    /// a checkpoint, and that tail is being watched, not missed.
    pub fn accounted(&self, t: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        let held = |windows: &[Window]| {
            within(windows, t) || (fresh(windows, now) && windows.last().is_some_and(|w| w.to < t))
        };
        (!self.gapped || held(&self.coverage))
            && !self
                .readings
                .values()
                .any(|r| r.from <= t && r.until.is_none_or(|until| t < until) && !held(&r.windows))
    }

    /// Whether the journal holds any validation run.
    pub fn has_runs(&self) -> bool {
        !self.readings.is_empty()
    }

    /// Each attempt's newest validation run, from the events at or before
    /// `t`; all of them for `None` (the live edge). An older run of the same
    /// attempt (one cancelled for a newer push, say) gives way to it.
    pub fn validation_at(&self, t: Option<DateTime<Utc>>) -> BTreeMap<Attempt, RunView> {
        let mut runs: BTreeMap<(Attempt, String), RunView> = BTreeMap::new();
        for event in self.ordered() {
            let at = event.at.expect("ordered events are dated");
            if t.is_some_and(|t| at > t) {
                break;
            }
            let (Some(attempt), Change::Validation { validation: v }) =
                (&event.attempt, &event.change)
            else {
                continue;
            };
            let view = runs
                .entry((attempt.clone(), v.run.clone()))
                .or_insert_with(|| RunView {
                    run: v.run.clone(),
                    started: None,
                    read: None,
                    reached: BTreeMap::new(),
                    lost: None,
                    first: at,
                });
            match v.phase {
                Phase::Started => view.started = Some((at, event.at_quality)),
                Phase::DaemonDown | Phase::Gone => view.lost = Some((v.phase, at)),
                Phase::Seen | Phase::Parked | Phase::Ended => {
                    let before = view.read.as_ref().map(|(_, r)| &r.steps);
                    for step in &v.steps {
                        let same = before
                            .and_then(|steps| steps.iter().find(|s| s.step == step.step))
                            .is_some_and(|s| s.status == step.status && s.round == step.round);
                        if !same {
                            view.reached.insert(step.step.clone(), (at, v.since));
                        }
                    }
                    view.read = Some((at, (**v).clone()));
                    view.lost = None;
                }
            }
        }
        let mut newest: BTreeMap<Attempt, RunView> = BTreeMap::new();
        for ((attempt, _), view) in runs {
            if newest
                .get(&attempt)
                .is_none_or(|kept| view.start() >= kept.start())
            {
                newest.insert(attempt, view);
            }
        }
        newest
    }

    /// Every attempt's state from the events at or before `t`; all of them for
    /// `None` (the live edge).
    pub fn state_at(&self, t: Option<DateTime<Utc>>) -> BTreeMap<Attempt, AttemptState> {
        let mut states: BTreeMap<Attempt, AttemptState> = BTreeMap::new();
        for event in self.ordered() {
            let at = event.at.expect("ordered events are dated");
            if t.is_some_and(|t| at > t) {
                break;
            }
            let Some(attempt) = &event.attempt else {
                continue;
            };
            if matches!(
                event.change,
                Change::Validation { .. } | Change::ValidationCoverage { .. }
            ) {
                // A run's reads are not the attempt's lifecycle (validation_at).
                continue;
            }
            let state = states.entry(attempt.clone()).or_default();
            match &event.change {
                Change::Spawned { spawned } => {
                    state.spawned = Some((at, event.at_quality));
                    state.kind = spawned.kind.clone().or(state.kind.take());
                    state.project = spawned.project.clone().or(state.project.take());
                }
                Change::Status { status } => {
                    state.status = Some(StatusAt {
                        value: status.value.clone(),
                        note: status.note.clone(),
                        at,
                        quality: event.at_quality,
                    });
                }
                Change::Decision { decision } => match decision.change {
                    DecisionChange::Opened | DecisionChange::Replaced => {
                        state.open_decisions.insert(decision.key.clone());
                    }
                    DecisionChange::Closed => {
                        state.open_decisions.remove(&decision.key);
                    }
                },
                Change::Reclassified { reclassified } => {
                    if let Some(to) = &reclassified.to {
                        state.kind = Some(to.clone());
                    }
                }
                Change::TornDown { torn_down } => {
                    state.torn_down = Some((at, event.at_quality));
                    state.outcome = torn_down.outcome.clone();
                }
                _ => {}
            }
        }
        states
    }

    /// Each attempt's native session. A join is identity, not state: it holds
    /// at every moment, including before the adapter observed it.
    pub fn sessions(&self) -> BTreeMap<Attempt, SessionKey> {
        self.events
            .values()
            .filter(|e| matches!(e.change, Change::Bound))
            .filter_map(|e| Some((e.attempt.clone()?, e.session.clone()?)))
            .collect()
    }

    /// How many events, dated or not, belong to these attempts.
    pub fn count_for(&self, attempts: &BTreeSet<Attempt>) -> usize {
        self.events
            .values()
            .filter(|e| e.attempt.as_ref().is_some_and(|a| attempts.contains(a)))
            .count()
    }

    /// The newest point event (not a coverage window) at or before `t`.
    pub fn latest_at(&self, t: Option<DateTime<Utc>>) -> Option<&LifecycleEvent> {
        self.ordered()
            .filter(|e| !e.change.is_coverage())
            .take_while(|e| t.is_none_or(|t| e.at.is_some_and(|at| at <= t)))
            .last()
    }
}

fn canonical(event: &LifecycleEvent) -> String {
    serde_json::to_string(event).unwrap_or_default()
}

/// Incremental reader of a journal file: remembers how far it has read, reads
/// only complete lines, and starts over if the file shrank or was replaced.
#[cfg(feature = "native")]
#[derive(Debug)]
pub struct JournalTail {
    path: std::path::PathBuf,
    offset: u64,
}

/// What one [`JournalTail::poll`] found.
#[cfg(feature = "native")]
#[derive(Debug, Default)]
pub struct Poll {
    /// The file shrank, vanished or no longer ends where it did: drop what was
    /// read from it before applying `events`.
    pub reset: bool,
    pub events: Vec<LifecycleEvent>,
    pub rejected: usize,
}

#[cfg(feature = "native")]
impl JournalTail {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
        }
    }

    /// The journal the adapter writes beside a manifest: `fleet.json` →
    /// `fleet.events.jsonl`.
    pub fn beside(manifest: &std::path::Path) -> Self {
        Self::new(manifest.with_extension("events.jsonl"))
    }

    pub fn poll(&mut self) -> std::io::Result<Poll> {
        use std::io::{Read, Seek, SeekFrom};
        let mut poll = Poll::default();
        let mut file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                poll.reset = std::mem::take(&mut self.offset) > 0;
                return Ok(poll);
            }
            Err(error) => return Err(error),
        };
        let len = file.metadata()?.len();
        if self.offset > 0 {
            // Everything read so far ended on a newline; if that byte is gone
            // or changed, this is a different file.
            let mut last = [0u8];
            let same = len >= self.offset
                && file.seek(SeekFrom::Start(self.offset - 1)).is_ok()
                && file.read_exact(&mut last).is_ok()
                && last[0] == b'\n';
            if !same {
                poll.reset = true;
                self.offset = 0;
            }
        }
        if len <= self.offset {
            return Ok(poll);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::with_capacity((len - self.offset) as usize);
        file.take(len - self.offset).read_to_end(&mut bytes)?;
        let (consumed, events, rejected) = parse_chunk(&bytes);
        self.offset += consumed as u64;
        poll.events = events;
        poll.rejected = rejected;
        Ok(poll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        format!("2026-09-22T{s}Z").parse().unwrap()
    }

    fn bridge() -> Source {
        Source {
            kind: SourceKind::Bridge,
            home: None,
            seq: None,
            run: Some("run-1".into()),
        }
    }

    fn attempt(task: &str) -> Option<Attempt> {
        Some(Attempt {
            task: task.into(),
            spawn_gen: format!("s-{task}"),
        })
    }

    fn event(id: &str, t: &str, task: &str, change: Change) -> LifecycleEvent {
        LifecycleEvent {
            id: id.into(),
            source: bridge(),
            at: Some(at(t)),
            at_quality: AtQuality::Stamp,
            attempt: attempt(task),
            session: None,
            change,
        }
    }

    fn status(value: &str) -> Change {
        Change::Status {
            status: Status {
                value: value.into(),
                key: None,
                note: None,
                digest: None,
                lifecycle_key: None,
            },
        }
    }

    fn coverage(id: &str, from: &str, to: &str) -> LifecycleEvent {
        LifecycleEvent {
            id: id.into(),
            source: bridge(),
            at: Some(at(to)),
            at_quality: AtQuality::Observed,
            attempt: None,
            session: None,
            change: Change::Coverage {
                coverage: Window {
                    from: at(from),
                    to: at(to),
                    max_gap: None,
                },
            },
        }
    }

    fn sample() -> Vec<LifecycleEvent> {
        vec![
            event(
                "spawn",
                "08:00:00",
                "impl",
                Change::Spawned {
                    spawned: Spawned {
                        kind: Some("ship".into()),
                        ..Spawned::default()
                    },
                },
            ),
            event("s1", "08:01:00", "impl", status("working")),
            event("s2", "08:05:00", "impl", status("needs-decision")),
            event("s3", "08:06:00", "impl", status("working")),
            event(
                "down",
                "08:09:00",
                "impl",
                Change::TornDown {
                    torn_down: TornDown::default(),
                },
            ),
            coverage("c1", "07:59:00", "08:03:00"),
            coverage("c2", "08:03:00", "08:07:00"),
            coverage("c3", "08:08:30", "08:10:00"),
        ]
    }

    fn line(event: &LifecycleEvent) -> String {
        serde_json::json!({"schema": JOURNAL_SCHEMA, "kind": "lifecycle", "event": event})
            .to_string()
    }

    #[test]
    fn lines_round_trip_through_the_wire_shape() {
        let text = r#"{"schema":"zoetrope.fleet.journal.v2","kind":"lifecycle","event":{
          "id":"bridge:f#status/zoe-parity/s1/1790064647/abc",
          "source":{"kind":"bridge","run":"r"},
          "at":"2026-09-22T08:10:47Z","at_quality":"stamp",
          "attempt":{"task":"zoe-parity","spawn_gen":"s1"},
          "session":{"provider":"claude","session_id":"5405a46b"},
          "type":"status","status":{"value":"done","key":"default","note":"report"},
          "future_field":true}}"#
            .replace('\n', "");
        let parsed = parse_line(text.as_bytes()).unwrap().unwrap();
        assert_eq!(parsed.at_quality, AtQuality::Stamp);
        assert!(matches!(&parsed.change, Change::Status { status } if status.value == "done"));
        let again = parse_line(line(&parsed).as_bytes()).unwrap().unwrap();
        assert_eq!(again, parsed);
        // Every type survives the trip.
        for change in [
            Change::Spawned {
                spawned: Spawned::default(),
            },
            Change::Decision {
                decision: Decision {
                    key: "k".into(),
                    change: DecisionChange::Opened,
                    verb: None,
                    closed_by: None,
                },
            },
            Change::Steered {
                steered: Steer { msg: "007".into() },
            },
            Change::SteerAcked {
                steer_acked: Steer { msg: "007".into() },
            },
            Change::Reclassified {
                reclassified: Reclassified {
                    from: Some("scout".into()),
                    to: Some("ship".into()),
                },
            },
            Change::TornDown {
                torn_down: TornDown {
                    outcome: Some("done".into()),
                },
            },
            Change::Busy {
                busy: Busy {
                    state: "busy".into(),
                },
            },
        ] {
            let each = event("x", "08:00:00", "t", change);
            assert_eq!(parse_line(line(&each).as_bytes()).unwrap(), Some(each));
        }
    }

    #[test]
    fn manifests_and_other_schemas_are_not_lifecycle() {
        for text in [
            r#"{"schema":"zoetrope.fleet.journal.v1","manifest":{"sessions":[]}}"#,
            r#"{"schema":"zoetrope.fleet.journal.v2","kind":"manifest","manifest":{}}"#,
            r#"{"schema":"zoetrope.fleet.journal.v9","kind":"lifecycle","event":{}}"#,
            "   ",
        ] {
            assert_eq!(parse_line(text.as_bytes()), Ok(None), "{text}");
        }
    }

    #[test]
    fn invalid_events_are_rejected_not_guessed() {
        let valid = event("ok", "08:00:00", "t", status("working"));
        let mut cases = Vec::new();
        let mut e = valid.clone();
        e.attempt = None;
        cases.push(e); // a status about no attempt
        let mut e = valid.clone();
        e.at = None;
        cases.push(e); // a stamp without a time
        let mut e = valid.clone();
        e.at_quality = AtQuality::Unknown;
        cases.push(e); // a time claimed unknown
        let mut e = valid.clone();
        e.id = "bad\nid".into();
        cases.push(e);
        let mut e = valid.clone();
        e.change = Change::Bound;
        cases.push(e); // a join without a session
        let mut e = coverage("c", "08:02:00", "08:01:00");
        cases.push(e.clone()); // backwards window
        e.id.clear();
        cases.push(e);
        for case in cases {
            assert!(case.validate().is_err(), "{case:?}");
            assert!(parse_line(line(&case).as_bytes()).is_err());
        }
        let unknown_type = line(&valid).replace("\"type\":\"status\"", "\"type\":\"teleported\"");
        assert!(parse_line(unknown_type.as_bytes()).is_err());
        assert!(parse_line(b"{\"schema\":").is_err());
        let mut undated = valid;
        undated.at = None;
        undated.at_quality = AtQuality::Unknown;
        assert!(undated.validate().is_ok());
    }

    #[test]
    fn a_partial_last_line_waits_for_its_newline() {
        let events = sample();
        let whole = format!("{}\n{}\n", line(&events[0]), line(&events[1]));
        let partial = &line(&events[2])[..20];
        let bytes = format!("{whole}{partial}");
        let (consumed, parsed, rejected) = parse_chunk(bytes.as_bytes());
        assert_eq!(consumed, whole.len());
        assert_eq!(parsed.len(), 2);
        assert_eq!(rejected, 0);
        let (consumed, parsed, rejected) = parse_chunk(b"{broken\n");
        assert_eq!((consumed, parsed.len(), rejected), (8, 0, 1));
    }

    /// Deterministic permutations: every rotation and its reverse.
    fn orders(n: usize) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        for k in 0..n {
            let rotated: Vec<usize> = (0..n).map(|i| (i + k) % n).collect();
            out.push(rotated.iter().rev().copied().collect());
            out.push(rotated);
        }
        // An interleaving that separates neighbours.
        out.push((0..n).step_by(2).chain((1..n).step_by(2)).collect());
        out
    }

    #[test]
    fn any_delivery_order_and_any_repeat_converge() {
        let events = sample();
        let mut expected = Lifecycle::default();
        expected.insert(events.clone());
        for order in orders(events.len()) {
            let mut store = Lifecycle::default();
            for i in &order {
                store.insert([events[*i].clone()]);
                // Re-delivery (an adapter restart re-emitting) changes nothing.
                assert!(!store.insert([events[*i].clone()]));
            }
            assert_eq!(
                store.ordered().collect::<Vec<_>>(),
                expected.ordered().collect::<Vec<_>>()
            );
            assert_eq!(store.coverage(), expected.coverage());
            for t in ["08:00:30", "08:05:30", "08:08:00", "08:20:00"] {
                assert_eq!(store.state_at(Some(at(t))), expected.state_at(Some(at(t))));
            }
        }
        // Conflicting duplicates settle on one winner whatever arrives first.
        let a = event("dup", "08:00:00", "t", status("working"));
        let b = event("dup", "08:00:00", "t", status("done"));
        let (mut x, mut y) = (Lifecycle::default(), Lifecycle::default());
        x.insert([a.clone(), b.clone()]);
        y.insert([b, a]);
        assert_eq!(
            x.ordered().collect::<Vec<_>>(),
            y.ordered().collect::<Vec<_>>()
        );
    }

    #[test]
    fn same_second_ties_put_births_first_and_teardown_last() {
        let mut store = Lifecycle::default();
        store.insert([
            event(
                "z-down",
                "08:00:00",
                "t",
                Change::TornDown {
                    torn_down: TornDown::default(),
                },
            ),
            event("a-status", "08:00:00", "t", status("done")),
            event(
                "m-spawn",
                "08:00:00",
                "t",
                Change::Spawned {
                    spawned: Spawned::default(),
                },
            ),
        ]);
        let names: Vec<_> = store.ordered().map(|e| e.change.name()).collect();
        assert_eq!(names, ["spawned", "status", "torn_down"]);
    }

    #[test]
    fn state_as_of_a_moment_and_coverage_gaps() {
        let mut store = Lifecycle::default();
        store.insert(sample());
        let impl_ = attempt("impl").unwrap();
        assert!(store.state_at(Some(at("07:59:59"))).is_empty());
        let early = &store.state_at(Some(at("08:00:30")))[&impl_];
        assert!(early.spawned.is_some() && early.status.is_none());
        let asking = &store.state_at(Some(at("08:05:00")))[&impl_];
        assert!(asking.needs_attention());
        let back = &store.state_at(Some(at("08:06:00")))[&impl_];
        assert!(!back.needs_attention());
        assert!(back.torn_down.is_none());
        let live = &store.state_at(None)[&impl_];
        assert!(live.torn_down.is_some());
        // Touching segments merge; the 08:07-08:08:30 stretch is a gap.
        assert_eq!(store.coverage().len(), 2);
        assert!(store.covered(at("08:03:00")) && store.covered(at("08:07:00")));
        assert!(!store.covered(at("08:07:01")) && !store.covered(at("07:58:59")));
        assert!(store.covered(at("08:09:00")));
        // At the live edge: a running bridge's newest window trails a little;
        // once it stops reporting, the present is a gap too.
        assert!(store.observing(at("08:11:00")));
        assert!(!store.observing(at("08:12:01")));
        // A bridge polling every 150 s says so: its newest segment may trail
        // by minutes and it is still observing.
        let mut slow = coverage("c4", "08:10:00", "08:20:00");
        if let Change::Coverage { coverage } = &mut slow.change {
            coverage.max_gap = Some(600);
        }
        store.insert([slow]);
        assert!(store.observing(at("08:23:40")));
        assert!(store.observing(at("08:39:59")));
        assert!(!store.observing(at("08:40:01")));
        assert_eq!(store.latest_at(Some(at("08:05:30"))).unwrap().id, "s2");
        assert_eq!(store.latest_at(None).unwrap().id, "down");
    }

    #[test]
    fn a_feed_without_bridge_events_has_no_gaps() {
        let mut store = Lifecycle::default();
        let mut e = event("f", "08:00:00", "t", status("working"));
        e.source = Source {
            kind: SourceKind::Firstmate,
            home: Some("fmh".into()),
            seq: Some(1),
            run: None,
        };
        store.insert([e]);
        assert!(!store.has_gaps());
        assert!(store.covered(at("12:00:00")));
        assert!(store.observing(at("12:00:00")));
    }

    fn feed(mut event: LifecycleEvent, seq: u64) -> LifecycleEvent {
        event.source = Source {
            kind: SourceKind::Firstmate,
            home: Some("fmh".into()),
            seq: Some(seq),
            run: None,
        };
        event.id = format!("firstmate:fmh#{}", event.id);
        event
    }

    fn ids(store: &Lifecycle) -> Vec<&str> {
        store.ordered().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn a_feed_replaces_only_the_bridge_facts_it_records_too() {
        let mut store = Lifecycle::default();
        let mut bound = event("bound", "08:00:30", "impl", Change::Bound);
        bound.session = Some(SessionKey {
            provider: "codex".into(),
            session_id: "native".into(),
        });
        // Status lines without a stamp, which the bridge only noticed.
        store.insert(sample().into_iter().map(|mut e| {
            if matches!(e.change, Change::Status { .. }) {
                e.at_quality = AtQuality::Observed;
            }
            e
        }));
        store.insert([
            bound,
            event("other", "08:05:00", "other", status("working")),
        ]);
        // The feed records two of those lines, each noticed a little after
        // the bridge did; it lost the repeated 08:06 one.
        let mut since = feed(coverage("cov", "08:04:00", "08:10:00"), 0);
        since.attempt = None;
        let [first, noticed] = [
            ("w", "08:01:10", "working"),
            ("s", "08:05:10", "needs-decision"),
        ]
        .map(|(id, t, value)| {
            let mut e = feed(event(id, t, "impl", status(value)), 1);
            e.at_quality = AtQuality::Observed;
            e
        });
        store.insert([since, first, noticed]);
        // Unstamped lines share no identity with the feed's, so every bridged
        // one stands beside the feed's records (s1, s2 duplicate them), and the
        // repeated s3 the feed lost places. A bridged spawn and teardown stand
        // until the feed records its own.
        assert_eq!(
            ids(&store),
            [
                "spawn",
                "bound",
                "s1",
                "firstmate:fmh#w",
                "c1",
                "other",
                "s2",
                "firstmate:fmh#s",
                "s3",
                "c2",
                "down",
                "c3",
                "firstmate:fmh#cov"
            ]
        );
        let impl_ = attempt("impl").unwrap();
        assert_eq!(
            store.state_at(Some(at("08:06:30")))[&impl_]
                .status
                .as_ref()
                .unwrap()
                .at,
            at("08:06:00")
        );
        let spawned = Change::Spawned {
            spawned: Spawned::default(),
        };
        let mut spawn = feed(event("born", "07:59:30", "impl", spawned), 2);
        spawn.at_quality = AtQuality::Firstmate;
        let gone = Change::TornDown {
            torn_down: TornDown::default(),
        };
        store.insert([spawn, feed(event("gone", "08:08:50", "impl", gone), 3)]);
        assert_eq!(
            ids(&store),
            [
                "firstmate:fmh#born",
                "bound",
                "s1",
                "firstmate:fmh#w",
                "c1",
                "other",
                "s2",
                "firstmate:fmh#s",
                "s3",
                "c2",
                "firstmate:fmh#gone",
                "c3",
                "firstmate:fmh#cov"
            ]
        );
        let live = &store.state_at(None)[&impl_];
        assert_eq!(live.spawned, Some((at("07:59:30"), AtQuality::Firstmate)));
        assert_eq!(live.torn_down.unwrap().0, at("08:08:50"));
        // The join, which no feed knows, and every other attempt stay.
        assert!(store.sessions().contains_key(&impl_));
        assert!(
            store
                .state_at(None)
                .contains_key(&attempt("other").unwrap())
        );
        // What was superseded is still held, for a removal to count.
        assert_eq!(store.count_for(&[impl_].into()), 10);
        // The feed covered the bridge's 08:07-08:08:30 gap.
        assert!(store.covered(at("08:07:30")));
        assert!(!store.covered(at("07:58:30")));
    }

    #[test]
    fn a_stamped_status_gives_way_only_to_the_feeds_own_record_of_it() {
        let mut store = Lifecycle::default();
        let mut since = feed(coverage("cov", "08:00:00", "08:10:00"), 0);
        since.attempt = None;
        let mut spawn = feed(
            event(
                "born",
                "07:59:30",
                "impl",
                Change::Spawned {
                    spawned: Spawned::default(),
                },
            ),
            1,
        );
        spawn.at_quality = AtQuality::Firstmate;
        // The feed holds impl's whole life, but lost the 08:06 status.
        store.insert([
            since,
            spawn,
            feed(event("s", "08:05:00", "impl", status("needs-decision")), 2),
        ]);
        store.insert([
            event("held", "08:05:00", "impl", status("needs-decision")),
            event("lost", "08:06:00", "impl", status("working")),
        ]);
        assert_eq!(
            ids(&store),
            [
                "firstmate:fmh#born",
                "firstmate:fmh#s",
                "lost",
                "firstmate:fmh#cov"
            ]
        );
        let status = store.state_at(None)[&attempt("impl").unwrap()]
            .status
            .clone();
        assert_eq!(status.unwrap().at, at("08:06:00"));
    }

    #[test]
    fn a_keyed_status_gives_way_only_to_the_feed_event_of_its_key() {
        let keyed = |mut e: LifecycleEvent, key: &str| {
            if let Change::Status { status } = &mut e.change {
                status.lifecycle_key = Some(format!("status/impl/g/@{key}"));
            }
            e
        };
        let noticed = |mut e: LifecycleEvent| {
            e.at_quality = AtQuality::Observed;
            e
        };
        let mut store = Lifecycle::default();
        store.insert([
            noticed(feed(
                event("status/impl/g/@0", "08:01:05", "impl", status("working")),
                1,
            )),
            feed(
                event("status/impl/g/@40", "08:05:00", "impl", status("done")),
                2,
            ),
        ]);
        store.insert([
            // The feed's @0, noticed without a stamp: its key is enough.
            keyed(
                noticed(event("same", "08:01:00", "impl", status("working"))),
                "0",
            ),
            // Stamped at the feed's @40 moment but another line: the key wins.
            keyed(event("other", "08:05:00", "impl", status("done")), "80"),
            // The same words again at a later offset, which the feed lost.
            keyed(
                noticed(event("again", "08:06:00", "impl", status("working"))),
                "120",
            ),
            // Without a key, as before it existed: a stamp still matches,
            // and a line with neither always places.
            event("stamped", "08:05:00", "impl", status("done")),
            noticed(event("bare", "08:07:00", "impl", status("working"))),
        ]);
        assert_eq!(
            ids(&store),
            [
                "firstmate:fmh#status/impl/g/@0",
                "other",
                "firstmate:fmh#status/impl/g/@40",
                "again",
                "bare"
            ]
        );
    }

    #[test]
    fn an_undated_feed_status_does_not_hide_the_bridged_copy_of_its_key() {
        let mut undated = feed(
            event("status/impl/g/@40", "08:05:00", "impl", status("done")),
            1,
        );
        undated.at = None;
        undated.at_quality = AtQuality::Unknown;
        let mut bridged = event("bridged", "08:05:00", "impl", status("done"));
        if let Change::Status { status } = &mut bridged.change {
            status.lifecycle_key = Some("status/impl/g/@40".into());
        }
        let mut store = Lifecycle::default();
        store.insert([undated]);
        store.insert([bridged]);
        assert_eq!(ids(&store), ["bridged"]);
    }

    #[test]
    fn a_feed_with_coverage_has_gaps_outside_it() {
        let mut store = Lifecycle::default();
        let mut since = feed(coverage("cov", "08:00:00", "08:10:00"), 0);
        since.attempt = None;
        store.insert([
            since,
            feed(event("f", "08:05:00", "t", status("working")), 1),
        ]);
        assert!(store.has_gaps());
        assert!(store.covered(at("08:05:00")));
        assert!(!store.covered(at("07:59:59")));
        assert!(store.observing(at("08:11:00")));
        assert!(!store.observing(at("08:12:01")));
    }

    /// `assets/fleet/feed`: what the adapter writes while it tails a synthetic
    /// Firstmate feed and bridges the same home (`feed_journal` in
    /// `scripts/test_firstmate_fleet.py`).
    #[test]
    fn the_feed_fixture_reads_as_the_feed_says() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/fleet/feed/fleet.events.jsonl"),
        )
        .unwrap();
        let (consumed, events, rejected) = parse_chunk(&bytes);
        assert_eq!((consumed, rejected), (bytes.len(), 0));
        let mut store = Lifecycle::default();
        store.insert(events);
        assert_eq!(store.rejected, 0);
        let attempt = |task: &str, spawn_gen: &str| Attempt {
            task: task.into(),
            spawn_gen: spawn_gen.into(),
        };
        let impl_ = attempt("impl", "s1790064100.4101.1");
        let tests = attempt("tests", "s1790064400.4102.2");
        let relaunched = attempt("tests", "s1790064600.4104.4");
        // The bridge saw impl before the feed began, but the feed backfilled
        // its spawn: beside the session join, only the feed's facts place.
        assert!(
            store
                .ordered()
                .filter(|e| e.attempt.as_ref() == Some(&impl_))
                .all(|e| e.source.kind == SourceKind::Firstmate || e.change == Change::Bound)
        );
        let early = &store.state_at(Some(at("08:02:00")))[&impl_];
        assert_eq!(early.spawned, Some((at("08:01:40"), AtQuality::Backfill)));
        assert_eq!(early.status.as_ref().unwrap().quality, AtQuality::Stamp);
        assert!(store.state_at(Some(at("08:05:30")))[&impl_].needs_attention());
        assert!(!store.state_at(Some(at("08:06:30")))[&impl_].needs_attention());
        // Session joins still come from the bridge.
        let joins = store.sessions();
        assert!(joins.contains_key(&impl_) && joins.contains_key(&relaunched));
        // The relaunch happened while no adapter ran: the feed records only
        // the new spawn, so the first attempt ends when the bridge noticed.
        let live = store.state_at(None);
        assert_eq!(
            live[&tests].torn_down,
            Some((at("08:11:00"), AtQuality::Observed))
        );
        assert_eq!(live[&tests].outcome, None);
        // Every status the bridge wrote ahead of the feed's record of it gives
        // way to that record, at the same stamp.
        assert!(
            store
                .ordered()
                .all(|e| e.source.kind == SourceKind::Firstmate
                    || !matches!(e.change, Change::Status { .. }))
        );
        assert_eq!(
            live[&attempt("survey", "s1790064420.4103.3")]
                .kind
                .as_deref(),
            Some("ship")
        );
        assert!(store.state_at(Some(at("08:12:00")))[&relaunched].needs_attention());
        assert!(!live[&relaunched].needs_attention());
        // The feed covers the adapter's downtime; before anyone watched is a gap.
        assert!(store.covered(at("08:09:30")));
        assert!(store.covered(at("08:01:30")));
        assert!(!store.covered(at("08:00:30")));
    }

    fn fixture(name: &str) -> Vec<LifecycleEvent> {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/fleet")
                .join(name)
                .join("fleet.events.jsonl"),
        )
        .unwrap();
        let (consumed, events, rejected) = parse_chunk(&bytes);
        assert_eq!((consumed, rejected), (bytes.len(), 0), "{name}");
        events
    }

    fn crew_attempt(task: &str) -> Attempt {
        Attempt {
            task: task.into(),
            spawn_gen: match task {
                "impl" => "s1790064100.4101.1",
                _ => "s1790064400.4102.2",
            }
            .into(),
        }
    }

    const IMPL_RUN: &str = "01M342G5B2S1MP13RVNVA11DAT";
    const TESTS_RUN: &str = "01M342X98JT3STSRVNVA11DAT1";

    /// `assets/fleet/validation`: what the adapter journals while it reads the
    /// runs attributed to the crew's impl and tests (`validation_journal` in
    /// `scripts/test_firstmate_fleet.py`).
    #[test]
    fn validation_lines_round_trip_and_invalid_ones_are_rejected() {
        let events = fixture("validation");
        assert!(
            events
                .iter()
                .all(|e| e.source.kind == SourceKind::NoMistakes)
        );
        for event in &events {
            let again = parse_line(line(event).as_bytes()).unwrap().unwrap();
            assert_eq!(&again, event);
        }
        let read = events
            .iter()
            .find(|e| matches!(&e.change, Change::Validation { validation } if validation.phase == Phase::Seen))
            .unwrap()
            .clone();
        let mut cases = Vec::new();
        let mut e = read.clone();
        e.attempt = None;
        cases.push(e); // a run of no attempt
        let mut e = read.clone();
        if let Change::Validation { validation } = &mut e.change {
            validation.status = None; // a read that found nothing
        }
        cases.push(e);
        let mut e = read.clone();
        if let Change::Validation { validation } = &mut e.change {
            validation.since = Some(at("23:59:59")); // bounded by a later read
        }
        cases.push(e);
        let mut e = read.clone();
        if let Change::Validation { validation } = &mut e.change {
            validation.run.clear();
        }
        cases.push(e);
        let mut e = coverage("c", "08:00:00", "08:01:00");
        e.change = Change::ValidationCoverage {
            validation_coverage: RunWindow {
                run: IMPL_RUN.into(),
                from: at("08:01:00"),
                to: at("08:00:00"),
                max_gap: None,
            },
        };
        e.attempt = attempt("impl");
        cases.push(e); // backwards
        for case in cases {
            assert!(case.validate().is_err(), "{case:?}");
            assert!(parse_line(line(&case).as_bytes()).is_err());
        }
        // An unknown phase is a line this viewer does not understand.
        let future = line(&read).replace("\"phase\":\"seen\"", "\"phase\":\"teleported\"");
        assert!(parse_line(future.as_bytes()).is_err());
    }

    /// no-mistakes words a failed push over several lines. The run's end
    /// must still land: dropping it left the run alive and unread forever,
    /// so every moment after it showed as unknown crew state.
    #[test]
    fn a_multiline_error_folds_and_the_run_still_ends() {
        let events = fixture("validation");
        let end = events
            .iter()
            .find(|e| matches!(&e.change, Change::Validation { validation } if validation.phase == Phase::Ended && validation.run == IMPL_RUN))
            .unwrap();
        let mut value = serde_json::to_value(end).unwrap();
        value["validation"]["error"] =
            "step push failed: exit status 128: remote: denied.\r\nfatal: 403\n\u{1b}".into();
        let written = serde_json::json!({
            "schema": JOURNAL_SCHEMA, "kind": "lifecycle", "event": value,
        })
        .to_string();
        let read = parse_line(written.as_bytes()).unwrap().unwrap();
        let Change::Validation { validation } = &read.change else {
            unreachable!()
        };
        assert_eq!(
            validation.error.as_deref(),
            Some("step push failed: exit status 128: remote: denied. fatal: 403")
        );
        assert_eq!(parse_line(line(&read).as_bytes()).unwrap().unwrap(), read);
        // Only control characters is no error at all.
        value["validation"]["error"] = "\n\t".into();
        let written = serde_json::json!({
            "schema": JOURNAL_SCHEMA, "kind": "lifecycle", "event": value,
        })
        .to_string();
        let blank = parse_line(written.as_bytes()).unwrap().unwrap();
        assert!(
            matches!(&blank.change, Change::Validation { validation } if validation.error.is_none())
        );
        // The folded end still closes the run: after it is no unread time.
        let ran = |e: &&LifecycleEvent| match &e.change {
            Change::Validation { validation } => validation.run == IMPL_RUN,
            Change::ValidationCoverage {
                validation_coverage,
            } => validation_coverage.run == IMPL_RUN,
            _ => false,
        };
        let mut store = Lifecycle::default();
        store.insert(
            events
                .iter()
                .filter(ran)
                .filter(|e| e.id != end.id)
                .cloned(),
        );
        let after = end.at.unwrap() + chrono::Duration::hours(12);
        assert!(store.unread(after), "without its end the run never closes");
        store.insert([read]);
        assert!(!store.unread(after));
    }

    /// At the live edge the newest windows trail the present by a checkpoint.
    /// While their readers are fresh, that tail is watched, not unknown; once
    /// they stop, it is.
    #[test]
    fn the_tail_a_fresh_reader_trails_is_accounted_for() {
        let mut store = Lifecycle::default();
        store.insert(fixture("validation"));
        store.insert([coverage("c", "08:00:00", "08:17:00")]);
        let tests = store
            .readings
            .get(&(crew_attempt("tests"), TESTS_RUN.to_owned()))
            .unwrap();
        assert!(tests.until.is_none(), "the tests run is still alive");
        let last = tests.windows.last().unwrap().to;
        let t = last.max(at("08:17:00")) + chrono::Duration::seconds(20);
        assert!(store.unread(t) && !store.covered(t));
        // Read and observed a moment ago: the tail is accounted for.
        assert!(store.accounted(t, t + chrono::Duration::seconds(10)));
        // Both readers stopped long ago: the same moment is unknown.
        assert!(!store.accounted(t, t + chrono::Duration::minutes(10)));
        // A gap inside the windows stays a gap however fresh the reader.
        let impl_ = &store.readings[&(crew_attempt("impl"), IMPL_RUN.to_owned())];
        let hole = impl_.windows[0].to + chrono::Duration::seconds(1);
        assert!(
            hole < impl_.windows[1].from,
            "the fixture's impl run has a gap"
        );
        assert!(!store.accounted(hole, impl_.windows[1].from));
        // Within every window, it is accounted for regardless of now.
        assert!(store.accounted(impl_.windows[0].to, t + chrono::Duration::hours(1)));
    }

    #[test]
    fn a_viewer_that_predates_validation_cannot_mistake_it_for_lifecycle() {
        // An older viewer's types, and nothing else: coverage, as it read it.
        #[derive(Debug, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        #[allow(dead_code)]
        enum Older {
            Coverage { coverage: Window },
            Status { status: Status },
        }
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct OlderEvent {
            id: String,
            #[serde(flatten)]
            change: Older,
        }
        for event in fixture("validation") {
            let value = serde_json::to_value(&event).unwrap();
            assert!(
                serde_json::from_value::<OlderEvent>(value).is_err(),
                "{} would read as a type it knows",
                event.id
            );
        }
    }

    #[test]
    fn validation_is_neither_lifecycle_state_nor_lifecycle_coverage() {
        let mut store = Lifecycle::default();
        store.insert(fixture("validation"));
        // No attempt appears from its runs alone, nor any lifecycle coverage.
        assert!(store.state_at(None).is_empty());
        assert!(!store.has_gaps() && store.coverage().is_empty());
        assert!(store.covered(at("08:09:00")));
        assert!(store.has_runs());
        // The footer never narrates a coverage window.
        assert!(!store.latest_at(None).unwrap().change.is_coverage());
    }

    #[test]
    fn each_attempt_folds_its_run_as_of_a_moment() {
        let mut store = Lifecycle::default();
        store.insert(fixture("crew"));
        store.insert(fixture("validation"));
        let (impl_, tests) = (crew_attempt("impl"), crew_attempt("tests"));
        assert!(store.validation_at(Some(at("08:07:44"))).is_empty());
        // Created from its ID before anyone read it.
        let run = &store.validation_at(Some(at("08:07:50")))[&impl_];
        assert_eq!(run.run, IMPL_RUN);
        assert_eq!(run.started, Some((at("08:07:45"), AtQuality::Derived)));
        assert!(run.read.is_none());
        assert!(!store.run_verified(&impl_, IMPL_RUN, Some(at("08:07:50"))));
        // The first read: every step reached by then, no earlier bound.
        let run = &store.validation_at(Some(at("08:08:00")))[&impl_];
        let (read_at, read) = run.read.as_ref().unwrap();
        assert_eq!(
            (*read_at, read.current().unwrap().step.as_str()),
            (at("08:08:00"), "review")
        );
        assert_eq!(run.reached["intent"], (at("08:08:00"), None));
        assert!(store.run_verified(&impl_, IMPL_RUN, Some(at("08:08:00"))));
        // The adapter stopped: the last read stands, unverified.
        let run = &store.validation_at(Some(at("08:09:00")))[&impl_];
        assert_eq!(run.read.as_ref().unwrap().0, at("08:08:00"));
        assert!(!store.run_verified(&impl_, IMPL_RUN, Some(at("08:09:00"))));
        assert!(store.unread(at("08:09:00")));
        // The next read, across the gap: review ended somewhere in it.
        let run = &store.validation_at(Some(at("08:10:00")))[&impl_];
        assert_eq!(
            run.reached["review"],
            (at("08:10:00"), Some(at("08:08:00")))
        );
        assert_eq!(run.reached["intent"], (at("08:08:00"), None), "unchanged");
        assert_eq!(run.read.as_ref().unwrap().1.current().unwrap().step, "test");
        // Ended: it cannot change, read or not.
        let live = store.validation_at(None);
        assert!(live[&impl_].ended());
        // An ended run is no gap; a live one between reads is.
        assert!(!store.unread(at("08:14:30")));
        assert!(store.unread(at("08:15:10")));
        // tests: read, then the daemon went down, then its gate parked.
        let run = &store.validation_at(Some(at("08:15:40")))[&tests];
        assert_eq!(run.lost, Some((Phase::DaemonDown, at("08:15:30"))));
        let run = &live[&tests];
        assert_eq!(run.lost, None, "a read after it clears the doubt");
        assert_eq!(
            run.read.as_ref().unwrap().1.gate.as_ref().unwrap().ask_user,
            1
        );
        assert!(store.run_verified(&tests, TESTS_RUN, Some(at("08:16:00"))));
        assert!(!store.run_verified(&tests, TESTS_RUN, Some(at("08:15:30"))));
        // At the live edge, long after the adapter stopped, it is not read.
        assert!(!store.run_verified(&tests, TESTS_RUN, None));
    }

    #[test]
    fn an_attempts_newer_run_replaces_its_older_one() {
        let mut store = Lifecycle::default();
        let events = fixture("validation");
        // Attribute tests' run to impl too: impl now has two runs.
        let moved: Vec<_> = events
            .iter()
            .filter(|e| e.attempt == Some(crew_attempt("tests")))
            .map(|e| {
                let mut e = e.clone();
                e.id.push_str("#moved");
                e.attempt = Some(crew_attempt("impl"));
                e
            })
            .collect();
        store.insert(events);
        store.insert(moved);
        let impl_ = crew_attempt("impl");
        assert_eq!(
            store.validation_at(Some(at("08:14:50")))[&impl_].run,
            IMPL_RUN
        );
        assert_eq!(store.validation_at(None)[&impl_].run, TESTS_RUN);
    }

    #[cfg(feature = "native")]
    #[test]
    fn tail_reads_appends_waits_on_partials_and_restarts_on_replacement() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!(
            "zoe-journal-{}-{}",
            std::process::id(),
            line(&sample()[0]).len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("fleet.json");
        let path = dir.join("fleet.events.jsonl");
        let _ = std::fs::remove_file(&path);
        let mut tail = JournalTail::beside(&manifest);
        assert!(tail.poll().unwrap().events.is_empty());
        let events = sample();
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "{}", line(&events[0])).unwrap();
        writeln!(
            file,
            r#"{{"schema":"zoetrope.fleet.journal.v1","manifest":{{}}}}"#
        )
        .unwrap();
        let second = line(&events[1]);
        write!(file, "{}", &second[..10]).unwrap();
        file.flush().unwrap();
        let first = tail.poll().unwrap();
        assert_eq!((first.reset, first.events.len()), (false, 1));
        writeln!(file, "{}", &second[10..]).unwrap();
        file.flush().unwrap();
        let next = tail.poll().unwrap();
        assert_eq!(next.events, vec![events[1].clone()]);
        assert!(tail.poll().unwrap().events.is_empty());
        // Replaced by a shorter file: read it from the start.
        std::fs::write(&path, format!("{}\n", line(&events[2]))).unwrap();
        let replaced = tail.poll().unwrap();
        assert!(replaced.reset);
        assert_eq!(replaced.events, vec![events[2].clone()]);
        std::fs::remove_file(&path).unwrap();
        assert!(tail.poll().unwrap().reset);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
