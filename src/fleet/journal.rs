//! The fleet lifecycle journal, `zoetrope.fleet.journal.v2`: timestamped facts
//! about Firstmate task attempts (spawned, bound to a native session, status,
//! torn down, ...) and the windows in which anyone was watching for them.
//!
//! The adapter writes these lines beside its manifest; the core reads only this
//! schema and never a Firstmate file. A line states one fact and how its time is
//! known ([`AtQuality`]). The store is a set keyed by event ID, so a re-read, an
//! adapter restart that re-emits, or lines in any order converge on one state.
//!
//! Coverage is what keeps the timeline honest. The snapshot bridge only sees
//! what it polls while it runs, so a moment outside every recorded
//! [`Change::Coverage`] window is a gap: the last record before it is shown as
//! unverified, never as a reconstruction.

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

/// A closed interval of wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
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
    /// An interval the bridge was observing. Bridge only.
    Coverage {
        coverage: Window,
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
        }
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
                for value in [&status.key, &status.note, &status.digest]
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

/// The deduplicated set of lifecycle events, kept in timeline order.
#[derive(Debug, Default)]
pub struct Lifecycle {
    events: BTreeMap<String, LifecycleEvent>,
    /// Dated events in [`LifecycleEvent::order`]; undated ones never place.
    order: Vec<String>,
    /// Merged coverage windows, ascending and disjoint.
    coverage: Vec<Window>,
    bridge: bool,
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
        let mut dated: Vec<&LifecycleEvent> =
            self.events.values().filter(|e| e.at.is_some()).collect();
        dated.sort_by(|a, b| a.order().cmp(&b.order()));
        self.order = dated.iter().map(|e| e.id.clone()).collect();
        let mut windows: Vec<Window> = self
            .events
            .values()
            .filter_map(|e| match e.change {
                Change::Coverage { coverage } => Some(coverage),
                _ => None,
            })
            .collect();
        windows.sort();
        self.coverage.clear();
        for window in windows {
            match self.coverage.last_mut() {
                // Bridge segments chain end to start; touching ones merge.
                Some(last) if window.from <= last.to => last.to = last.to.max(window.to),
                _ => self.coverage.push(window),
            }
        }
        self.bridge = self
            .events
            .values()
            .any(|e| e.source.kind == SourceKind::Bridge);
        self.generation = self.generation.wrapping_add(1);
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

    /// Whether gaps apply at all: coverage describes the bridge, which only
    /// sees what it polls while it runs. A journal without bridge events has
    /// nothing a gap could mean.
    pub fn has_gaps(&self) -> bool {
        self.bridge
    }

    /// Whether `t` lies inside a window someone was observing.
    pub fn covered(&self, t: DateTime<Utc>) -> bool {
        if !self.bridge {
            return true;
        }
        let i = self.coverage.partition_point(|w| w.from <= t);
        i > 0 && t <= self.coverage[i - 1].to
    }

    /// Whether the bridge is still observing at `now`. Its newest window
    /// trails the present by up to a checkpoint while it runs, so that much
    /// lag still counts; past it, the adapter has stopped.
    pub fn observing(&self, now: DateTime<Utc>) -> bool {
        !self.bridge
            || self
                .coverage
                .last()
                .is_some_and(|w| w.from <= now && now - w.to <= chrono::Duration::minutes(2))
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

    /// The newest point event (not a coverage window) at or before `t`.
    pub fn latest_at(&self, t: Option<DateTime<Utc>>) -> Option<&LifecycleEvent> {
        self.ordered()
            .filter(|e| !matches!(e.change, Change::Coverage { .. }))
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
