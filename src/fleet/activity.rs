//! A live, merged activity strip. It never folds another session into an App
//! or seeks multiple independent histories using one session's clock.
use super::{Fleet, SessionKey};
use crate::fact::{FactKind, Outcome};
use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Sparkline},
};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct Point {
    at: DateTime<Utc>,
    failed: bool,
    label: String,
}
#[derive(Default)]
pub struct Activity {
    signature: Vec<(SessionKey, usize, u64)>,
    points: Vec<Point>,
    counts: Vec<u64>,
    failures: usize,
}
impl Activity {
    pub fn refresh(fleet: &mut Fleet) {
        let signature: Vec<_> = fleet
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
        if signature == fleet.activity.signature {
            return;
        }
        let mut points = BTreeMap::new();
        for (key, member) in &fleet.members {
            for item in &member.app.timeline.items {
                let Some(at) = item.ts() else { continue };
                for fact in &item.facts {
                    let agent = fact.agent.as_deref().unwrap_or("");
                    let (id, failed, description) = match &fact.kind {
                        FactKind::ToolStart { call, name, .. } => {
                            (format!("tool:{call}"), false, name.clone())
                        }
                        FactKind::ToolEnd {
                            call,
                            outcome: Outcome::Err,
                        } => (format!("error:{call}"), true, "tool failed".into()),
                        FactKind::Prompt(_) => (format!("prompt:{at}"), false, "prompt".into()),
                        FactKind::Agent { .. } => ("agent".into(), false, "joined".into()),
                        _ => continue,
                    };
                    points
                        .entry((key.clone(), agent.to_owned(), id))
                        .or_insert_with(|| Point {
                            at,
                            failed,
                            label: format!("{} · {description}", member.spec.label),
                        });
                }
            }
        }
        let mut points: Vec<_> = points.into_values().collect();
        points.sort_by_key(|p| p.at);
        let failures = points.iter().filter(|p| p.failed).count();
        fleet.activity = Self {
            signature,
            points,
            counts: Vec::new(),
            failures,
        };
    }
    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        if area.height < 3 || area.width < 4 {
            return;
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Fleet activity · LIVE · Enter on agent: session replay ");
        let inner = block.inner(area);
        frame.render_widget(block.style(Style::default().fg(Color::DarkGray)), area);
        let Some(first) = self.points.first() else {
            frame.render_widget(Paragraph::new(" Waiting for recorded activity…"), inner);
            return;
        };
        let last = self.points.last().unwrap();
        if self.counts.len() != inner.width as usize {
            let span = (last.at - first.at).num_milliseconds().max(1) as u64;
            self.counts = vec![0; inner.width as usize];
            for p in &self.points {
                let x = (p.at - first.at).num_milliseconds().max(0) as u128
                    * (inner.width.saturating_sub(1)) as u128
                    / span as u128;
                self.counts[x as usize] += 1;
            }
        }
        let chart = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(2),
        );
        frame.render_widget(
            Sparkline::default()
                .data(&self.counts)
                .style(Style::default().fg(Color::Indexed(178))),
            chart,
        );
        let errors = self.failures;
        let text = format!(
            "{} → {} · {} events · {errors} failures\nLatest: {}",
            first
                .at
                .with_timezone(&chrono::Local)
                .format("%b %d %H:%M:%S"),
            last.at
                .with_timezone(&chrono::Local)
                .format("%b %d %H:%M:%S"),
            self.points.len(),
            last.label
        );
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Gray)),
            Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2),
        );
    }
}
