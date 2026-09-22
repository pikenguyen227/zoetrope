//! The fleet timeline: one wall-clock playhead across every session.
//!
//! The overview's own upstream [`Timeline`](crate::state::Timeline) holds the
//! merged index: one mark per dated member transcript item and one per dated
//! lifecycle event, in time order. That index is never folded — the overview
//! renders a projection of its members — but it gives the fleet the upstream
//! pacing, scrubber, markers and transport state unchanged. Its cursor is the
//! fleet playhead, and the playhead drives each member through the member's own
//! seek path ([`App::park_at`](crate::state::App::park_at)), so a member's model,
//! liveness and chips are as of that moment. Following the edge, members ride
//! their own live edges instead.
//!
//! Gap compression applies to the union: dead air is skipped only while every
//! session is quiet.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};

use super::journal::{Change, LifecycleEvent, TRANSCRIPT_RANK};
use super::{Fleet, SessionKey};
use crate::fact::{AgentKind, Fact, FactKind, Outcome};
use crate::state::session::MAIN_ID;
use crate::tailer::ReplayItem;

/// A lifecycle event's glyph on the scrubber's marker strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Spawned,
    Attention,
    Done,
    Failed,
    Status,
    TornDown,
    Other,
}

impl MarkKind {
    fn of(event: &LifecycleEvent) -> Self {
        match &event.change {
            Change::Spawned { .. } => Self::Spawned,
            Change::TornDown { .. } => Self::TornDown,
            Change::Status { status } => match status.value.as_str() {
                "needs-decision" | "blocked" => Self::Attention,
                "done" => Self::Done,
                "failed" => Self::Failed,
                _ => Self::Status,
            },
            Change::Decision { .. } => Self::Attention,
            _ => Self::Other,
        }
    }

    fn glyph(self, palette: &rataflow::Palette) -> Option<(&'static str, ratatui::style::Color)> {
        Some(match self {
            Self::Spawned => ("+", palette.success),
            Self::Attention => ("▲", crate::ui::nodes::ATTENTION),
            Self::Done => ("✓", palette.accent),
            Self::Failed => ("✗", palette.error),
            Self::Status => ("•", palette.text),
            Self::TornDown => ("⊘", palette.subtle),
            Self::Other => return None,
        })
    }
}

/// The fleet's playhead bookkeeping. The playhead itself is the overview
/// timeline's cursor.
#[derive(Debug)]
pub struct FleetTimeline {
    signature: Vec<(SessionKey, usize, u64)>,
    lifecycle_generation: Option<u64>,
    /// Lifecycle events on the merged index, for the marker strip.
    pub(crate) marks: Vec<(usize, MarkKind)>,
    /// Chapter boundaries `[` / `]` step between: member prompts and
    /// lifecycle transitions.
    chapters: Vec<DateTime<Utc>>,
    /// Members ride their own live edges: the fleet is following its edge.
    pub(crate) live: bool,
    /// A discontinuous jump happened: the next sync absorbs the chip history
    /// at the new moment and leaves the camera alone.
    pub(crate) jumped: bool,
}

impl Default for FleetTimeline {
    fn default() -> Self {
        Self {
            signature: Vec::new(),
            lifecycle_generation: None,
            marks: Vec::new(),
            chapters: Vec::new(),
            live: true,
            jumped: false,
        }
    }
}

/// One mark on the merged index before sorting.
struct Entry {
    at: DateTime<Utc>,
    /// Same-second order against transcript items.
    rank: u8,
    item: ReplayItem,
    mark: Option<MarkKind>,
    /// A boundary `[` / `]` can step to.
    chapter: bool,
}

/// Only what the scrubber draws survives into a mark: tool starts, spawns,
/// failures and main-thread prompts, with their text dropped.
fn reduce(item: &ReplayItem) -> Vec<Fact> {
    item.facts
        .iter()
        .filter_map(|f| {
            let (agent, kind) = match &f.kind {
                FactKind::ToolStart { .. } => (
                    None,
                    FactKind::ToolStart {
                        call: String::new(),
                        name: String::new(),
                        summary: None,
                    },
                ),
                FactKind::ToolEnd {
                    outcome: Outcome::Err,
                    ..
                } => (
                    None,
                    FactKind::ToolEnd {
                        call: String::new(),
                        outcome: Outcome::Err,
                    },
                ),
                FactKind::Agent { kind, .. } if *kind != AgentKind::Main => (
                    None,
                    FactKind::Agent {
                        kind: *kind,
                        parent: None,
                        agent_type: None,
                        description: None,
                        spawned_by: None,
                        interactive: false,
                    },
                ),
                FactKind::Prompt(_) if f.agent.as_deref() == Some(MAIN_ID) => {
                    (Some(MAIN_ID.to_owned()), FactKind::Prompt(String::new()))
                }
                _ => return None,
            };
            Some(Fact {
                agent,
                ts: f.ts,
                kind,
            })
        })
        .collect()
}

impl Fleet {
    /// The moment the fleet shows: `None` at the live edge, where members ride
    /// their own edges and lifecycle is read in full.
    pub fn at(&self) -> Option<DateTime<Utc>> {
        if self.timeline.live {
            None
        } else {
            self.overview.timeline.cursor
        }
    }

    /// Whether lifecycle was observed at the moment shown: parked, whether a
    /// coverage window holds it; at the live edge, whether the bridge still is.
    pub fn covered(&self) -> bool {
        match self.at() {
            Some(t) => self.lifecycle.covered(t),
            None => self.lifecycle.observing(Utc::now()),
        }
    }

    /// Rebuild the merged index when a member's items or the lifecycle changed.
    pub fn refresh_timeline(&mut self) {
        // Checked every frame, so compare in place before allocating anything.
        let unchanged = self.timeline.lifecycle_generation == Some(self.lifecycle.generation)
            && self.members.len() == self.timeline.signature.len()
            && self.members.iter().zip(&self.timeline.signature).all(
                |((key, m), (seen, len, generation))| {
                    key == seen
                        && m.app.timeline.items.len() == *len
                        && m.app.timeline.generation == *generation
                },
            );
        if unchanged {
            return;
        }
        let signature: Vec<_> = self
            .members
            .iter()
            .map(|(key, m)| {
                (
                    key.clone(),
                    m.app.timeline.items.len(),
                    m.app.timeline.generation,
                )
            })
            .collect();
        // A stable sort keeps each member's own order within a tie.
        let mut entries: Vec<Entry> = Vec::new();
        for member in self.members.values() {
            for item in &member.app.timeline.items {
                let Some(at) = item.ts() else { continue };
                let facts = reduce(item);
                let chapter = facts.iter().any(|f| matches!(f.kind, FactKind::Prompt(_)));
                entries.push(Entry {
                    at,
                    rank: TRANSCRIPT_RANK,
                    item: ReplayItem::at(Some(at), facts),
                    mark: None,
                    chapter,
                });
            }
        }
        for event in self.lifecycle.ordered() {
            if matches!(event.change, Change::Coverage { .. }) {
                continue;
            }
            let at = event.at.expect("ordered events are dated");
            let kind = MarkKind::of(event);
            entries.push(Entry {
                at,
                rank: event.change.rank(),
                item: ReplayItem::at(Some(at), Vec::new()),
                mark: Some(kind),
                chapter: kind != MarkKind::Other,
            });
        }
        entries.sort_by_key(|e| (e.at, e.rank));
        let mut items = Vec::with_capacity(entries.len());
        self.timeline.marks.clear();
        self.timeline.chapters.clear();
        for (index, entry) in entries.into_iter().enumerate() {
            if let Some(mark) = entry.mark {
                self.timeline.marks.push((index, mark));
            }
            if entry.chapter && self.timeline.chapters.last() != Some(&entry.at) {
                self.timeline.chapters.push(entry.at);
            }
            items.push(entry.item);
        }
        self.overview.timeline.replace_items(items);
        self.overview.timeline.folded = self.overview.timeline.fold_target();
        self.overview.scrubber_tally = None;
        self.timeline.signature = signature;
        self.timeline.lifecycle_generation = Some(self.lifecycle.generation);
    }

    /// Advance the shared playhead one frame and drive the members to it.
    /// Returns whether the projection needs a sync.
    pub fn tick_timeline(&mut self, elapsed: std::time::Duration) -> bool {
        self.refresh_timeline();
        if let Some(f) = self.overview.pending_seek.take() {
            self.seek_to_fraction(f);
        }
        let before = self.overview.timeline.cursor;
        let paused = self.overview.is_paused;
        let timeline = &mut self.overview.timeline;
        timeline.advance(elapsed, paused);
        // Playback only moves forward; a rebuilt index must not pull it back.
        if let (Some(b), Some(a)) = (before, timeline.cursor)
            && a < b
        {
            timeline.cursor = Some(b);
        }
        let fold = timeline.fold_target();
        let mut dirty = fold != timeline.folded;
        timeline.folded = fold;
        dirty |= self.drive_members(false);
        // A jump from input since the last frame still owes a sync.
        dirty || self.timeline.jumped
    }

    /// Put every member at the fleet playhead, or on its own live edge when
    /// the fleet follows its edge. Returns whether any member's model moved.
    fn drive_members(&mut self, jumped: bool) -> bool {
        let timeline = &self.overview.timeline;
        if timeline.following() {
            if self.timeline.live {
                return false;
            }
            self.timeline.live = true;
            // Cards switch from the journal's past back to today's manifest.
            self.timeline.jumped = true;
            self.overview.wall_clock = true;
            for member in self.members.values_mut() {
                member.app.go_live();
            }
            return true;
        }
        let Some(t) = timeline.cursor else {
            return false;
        };
        let entering = std::mem::replace(&mut self.timeline.live, false);
        self.timeline.jumped |= entering;
        self.overview.wall_clock = false;
        let mut moved = entering;
        for member in self.members.values_mut() {
            let folded = member.app.park_at(t);
            if !folded && (jumped || entering) {
                // Nothing new is due, but "now" jumped: liveness follows it.
                member.app.status_tick();
            }
            moved |= folded;
        }
        moved || jumped
    }

    /// Jump the playhead to `t`, re-pinning to the edge at or past it.
    pub fn seek(&mut self, t: DateTime<Utc>) {
        let timeline = &mut self.overview.timeline;
        let head = timeline.head_ts();
        timeline.cursor = Some(t);
        timeline.follow_head = head.is_none_or(|h| t >= h);
        self.commit_seek();
    }

    /// A scrubber click or drag: index-based, like the single-session bar.
    pub fn seek_to_fraction(&mut self, f: f64) {
        let timeline = &mut self.overview.timeline;
        let len = timeline.items.len();
        if len == 0 {
            return;
        }
        let target = timeline.fold_at_fraction(f).clamp(1, len);
        timeline.cursor = timeline.ts_at_index(target - 1);
        timeline.follow_head = target >= len;
        self.commit_seek();
    }

    fn commit_seek(&mut self) {
        let timeline = &mut self.overview.timeline;
        timeline.reset_pacing();
        timeline.folded = timeline.fold_target();
        if timeline.follow_head {
            self.overview.is_paused = false;
        }
        self.timeline.jumped = true;
        self.drive_members(true);
    }

    /// Back to the live edge, playing.
    pub fn go_live(&mut self) {
        self.overview.is_paused = false;
        match self.overview.timeline.head_ts() {
            Some(head) => self.seek(head),
            None => self.overview.timeline.follow_head = true,
        }
    }

    /// Space: freeze the whole crew at this moment, or play on from it.
    pub fn toggle_play_pause(&mut self) {
        self.overview.toggle_play_pause();
        self.drive_members(false);
    }

    /// `[` / `]`: the previous or next chapter — a member's prompt or a
    /// lifecycle transition. Past the last one is the live edge.
    pub fn step(&mut self, forward: bool) {
        let Some(cursor) = self.overview.timeline.cursor else {
            return;
        };
        let chapters = &self.timeline.chapters;
        let target = if forward {
            chapters.iter().find(|t| **t > cursor)
        } else {
            chapters.iter().rev().find(|t| **t < cursor)
        };
        match target.copied() {
            Some(t) => self.seek(t),
            None if forward => self.go_live(),
            None => {
                if let Some(start) = self.overview.timeline.start_ts() {
                    self.seek(start);
                }
            }
        }
    }
}

/// Lifecycle marks and coverage gaps over the upstream scrubber, drawn after
/// it into the track rectangle it reported. The playhead column stays clear.
pub fn draw_overlay(frame: &mut Frame, fleet: &Fleet) {
    let Some(area) = fleet.overview.scrubber_area else {
        return;
    };
    let timeline = &fleet.overview.timeline;
    let width = area.width as usize;
    let len = timeline.items.len();
    if width < 2 || len == 0 || area.height == 0 {
        return;
    }
    let last = (width - 1) as f64;
    let head = ((timeline.progress().clamp(0.0, 1.0) * last).round() as usize).min(width - 1);
    let palette = fleet.overview.flow.theme.palette();
    let buf = frame.buffer_mut();
    if fleet.lifecycle.has_gaps() {
        // A column is a gap when its moment lies outside every window the
        // bridge was observing: lifecycle there is unknown, not steady.
        let floor = timeline.floor();
        let reach = len.saturating_sub(floor);
        for c in (0..width).filter(|&c| c != head) {
            let index = (floor + ((c as f64 / last) * reach as f64).round() as usize).min(len - 1);
            let Some(ts) = timeline.items[index].ts() else {
                continue;
            };
            if fleet.lifecycle.covered(ts) {
                continue;
            }
            // Hatched and dimmed: activity there is real, the crew state is not.
            let x = area.x + c as u16;
            for y in area.y..area.y + area.height {
                let cell = &mut buf[(x, y)];
                if cell.symbol() == " " {
                    cell.set_symbol("░");
                }
                cell.set_fg(palette.muted);
            }
        }
    }
    for &(index, kind) in &fleet.timeline.marks {
        let Some((glyph, color)) = kind.glyph(&palette) else {
            continue;
        };
        let col = (timeline.bar_fraction_for_index(index) * last).round() as usize;
        if col >= width || col == head {
            continue;
        }
        // Like chapter ticks, marks show the whole run: bright once played.
        let style = if col < head {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.subtle)
        };
        buf[(area.x + col as u16, area.y)]
            .set_symbol(glyph)
            .set_style(style);
    }
}
