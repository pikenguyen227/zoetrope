//! [`AgentNode`]: the `NodeContent` for an agent card.
//!
//! Renders a bordered card showing the agent type, a status glyph + color,
//! a truncated description, a tool count, the last tool name, and an output
//! token count. Visual state (status, tool tallies) is mirrored from the
//! domain model into the node via `flow.node_content_mut` on each sync, so the
//! card needs no back-reference to the `SessionModel`.

use rataflow::{NodeContent, NodeRenderContext};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget};

use crate::state::session::AgentStatus;

/// Fixed card dimensions for main / workflow nodes (world units).
pub const MAIN_NODE_DIMS: (f64, f64) = (32.0, 8.0);
/// Fixed card dimensions for subagent nodes (world units).
pub const SUB_NODE_DIMS: (f64, f64) = (30.0, 8.0);

/// Below this on-screen size a card has no room for any text — it renders at
/// cell level instead (solid status-colored fill). Semantic zoom: zoomed out,
/// nodes show as status cells instead of empty bordered boxes.
pub const CELL_MIN_WIDTH: u16 = 10;
/// See [`CELL_MIN_WIDTH`].
pub const CELL_MIN_HEIGHT: u16 = 3;

/// Custom node content rendered as an agent card.
///
/// Plain `pub` fields: the graph layer reads them back and mutates them in
/// place during incremental sync (mirroring [`crate::state::session::AgentInfo`]).
#[derive(Debug, Clone)]
pub struct AgentNode {
    /// Title line — the agent type, which for the main agent is the provider's
    /// own name (`claude`, `codex`).
    pub title: String,
    /// Truncated description shown under the title.
    pub description: Option<String>,
    pub status: AgentStatus,
    /// Number of tool calls (for the `⚒ N tools` line).
    pub tool_count: usize,
    /// Name of the most recent tool call, if any.
    pub last_tool: Option<String>,
    pub output_tokens: u64,
    pub usage: crate::usage::Summary,
    /// Interactive agents (main, forks) word `Running` as "active": we know
    /// there are recent entries, not that a task is executing.
    pub interactive: bool,
    /// The off beat of the running pulse, flipped by
    /// [`App::tick_pulse`](crate::state::App::tick_pulse). Presentation only:
    /// the graph sync never compares it and carries it across rebuilds.
    pub pulse: bool,
    /// Firstmate lifecycle as of the playhead, on a Fleet card: the task's
    /// status badge, and whether the attempt is gone. `None` everywhere else.
    /// Presentation only, like `pulse`: the Fleet sets it after each sync.
    pub crew: Option<CrewMark>,
    /// The attempt's no-mistakes validation run as of the playhead, on a
    /// Fleet card: drawn in the description row's place. Presentation only,
    /// like `crew`.
    pub validation: Option<ValidationBand>,
}

/// A Fleet card's validation band, and the reading panel's step table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationBand {
    /// The run's state in a glyph and a few words (`▸ test r1 · 2m`), first,
    /// so a narrow card keeps it and drops the step glyphs.
    pub glyph: char,
    pub headline: String,
    pub tone: CrewTone,
    /// One glyph per step, in pipeline order.
    pub steps: Vec<(char, CrewTone)>,
    /// The reading panel's lines above the table: the run, how it was read.
    pub heading: Vec<String>,
    /// One row per step: glyph, tone, text.
    pub rows: Vec<(char, CrewTone, String)>,
    /// The row of the step the run is at (or ended on), which a panel too
    /// short for the whole table keeps in view.
    pub current: Option<usize>,
}

/// A Fleet card's lifecycle badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewMark {
    pub label: String,
    pub tone: CrewTone,
    /// The attempt was torn down by then: drawn muted, still present.
    pub dimmed: bool,
}

/// How a badge reads. `Attention` is the one that must catch the eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrewTone {
    /// Waiting on the operator: `needs-decision`, `blocked`, an open decision.
    Attention,
    Failed,
    Active,
    Settled,
    Quiet,
    /// Outside every coverage window: the last record, not verified.
    Unknown,
}

/// Needs-decision and blocked, distinct from gold selection and red failure.
pub const ATTENTION: ratatui::style::Color = ratatui::style::Color::Indexed(208);

impl CrewTone {
    fn glyph(self) -> char {
        match self {
            Self::Attention => '▲',
            Self::Failed => '✗',
            Self::Active => '▸',
            Self::Settled => '✓',
            Self::Quiet => '·',
            Self::Unknown => '?',
        }
    }

    pub(crate) fn color(self, palette: &rataflow::Palette) -> ratatui::style::Color {
        match self {
            Self::Attention => ATTENTION,
            Self::Failed => palette.error,
            Self::Active => palette.success,
            Self::Settled => palette.accent,
            Self::Quiet | Self::Unknown => palette.subtle,
        }
    }
}

use crate::ui::truncate;

/// Compact human token count, e.g. `1.2k`, `34`, `2.0M`.
fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

impl NodeContent for AgentNode {
    fn render(&self, ctx: &NodeRenderContext, buf: &mut Buffer) {
        let palette = ctx.theme.palette();
        let area = ctx.area;

        // Degenerate areas: a border needs at least 2x2; bail out gracefully.
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Single-source vocabulary + presence colors (cards/panel/inspect
        // share them; only the pulse override below is card-specific).
        let glyph_color = crate::ui::status_color(self.status, &palette);
        let status_text = crate::state::session::status_word(self.status, self.interactive);

        // Cell level (semantic zoom): too small for any text — a bordered card
        // carries no information here, so paint a solid status-colored block.
        let crew = self.crew.as_ref();
        let attention = crew.is_some_and(|c| c.tone == CrewTone::Attention && !c.dimmed);
        let dimmed = crew.is_some_and(|c| c.dimmed);
        if area.width < CELL_MIN_WIDTH || area.height < CELL_MIN_HEIGHT {
            // Zoomed out, a crew waiting on the operator still stands out.
            let fill_color = if attention {
                ATTENTION
            } else if dimmed {
                palette.muted
            } else {
                glyph_color
            };
            let mut fill = Style::default().bg(fill_color);
            if ctx.selected {
                fill = fill.add_modifier(Modifier::REVERSED);
            }
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    buf[(x, y)].set_char(' ').set_style(fill);
                }
            }
            return;
        }

        let border_color = if ctx.selected {
            palette.accent
        } else if attention {
            ATTENTION
        } else {
            palette.muted
        };
        let text_color = if dimmed { palette.muted } else { palette.text };
        let bg_style = Style::default().bg(palette.surface);
        let border_style = bg_style.fg(border_color);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(bg_style);

        // Only pad when there's horizontal room to spare.
        if area.width >= 4 {
            block = block.padding(Padding::horizontal(1));
        }

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Pulse: alive agents breathe on the app's pulse clock (~1s cycle) —
        // the wide-shot heartbeat. Not rataflow's animation phase: that wraps
        // with the edge dash pattern every few steps, too fast to read.
        let glyph = if self.status == AgentStatus::Running && self.pulse {
            '○'
        } else {
            self.status.glyph()
        };
        let inner_w = inner.width as usize;

        // Title row: status glyph + agent type, bold.
        let title_budget = inner_w.saturating_sub(2); // glyph + space
        let title = truncate(&self.title, title_budget);
        let title_line = Line::from(vec![
            Span::styled(format!("{glyph} "), bg_style.fg(glyph_color)),
            Span::styled(title, bg_style.fg(text_color).add_modifier(Modifier::BOLD)),
        ]);

        // Build the candidate lines in priority order.
        let mut lines: Vec<Line> = Vec::new();
        lines.push(title_line);

        if let Some(crew) = crew {
            let color = if crew.dimmed {
                palette.muted
            } else {
                crew.tone.color(&palette)
            };
            let mut style = bg_style.fg(color);
            if attention {
                style = style.add_modifier(Modifier::BOLD);
            }
            let badge = truncate(&format!("{} {}", crew.tone.glyph(), crew.label), inner_w);
            lines.push(Line::from(Span::styled(badge, style)));
        }

        // A validation run takes the description's row, keeping the card's
        // six: its `provider · session` is the least a crew card says.
        if let Some(band) = &self.validation {
            lines.push(band_line(band, inner_w, bg_style, &palette, dimmed));
        } else if let Some(desc) = self.description.as_ref().filter(|d| !d.is_empty()) {
            let desc = truncate(desc, inner_w);
            lines.push(Line::from(Span::styled(desc, bg_style.fg(palette.subtle))));
        }

        // Tools row: "⚒ N · last_tool".
        let tools_text = if let Some(last) = self.last_tool.as_ref() {
            let prefix = format!("⚒ {} · ", self.tool_count);
            let budget =
                inner_w.saturating_sub(unicode_width::UnicodeWidthStr::width(prefix.as_str()));
            format!("{prefix}{}", truncate(last, budget))
        } else {
            format!("⚒ {} tools", self.tool_count)
        };
        lines.push(Line::from(Span::styled(
            tools_text,
            bg_style.fg(palette.accent),
        )));

        // Footer row: status word + token count, separated to the edges.
        let tokens = fmt_tokens(if self.usage.recorded {
            self.usage.total()
        } else {
            self.output_tokens
        });
        let footer = if self.usage.recorded || self.output_tokens > 0 {
            Line::from(vec![
                Span::styled(status_text, bg_style.fg(glyph_color)),
                Span::styled(
                    format!(
                        "  {tokens} {}",
                        if self.usage.recorded && !self.usage.incomplete {
                            "tok"
                        } else {
                            "tok+"
                        }
                    ),
                    bg_style.fg(palette.muted),
                ),
            ])
        } else {
            Line::from(Span::styled(status_text, bg_style.fg(glyph_color)))
        };
        lines.push(footer);
        if self.usage.recorded {
            lines.push(Line::from(Span::styled(
                self.usage.cost_label(),
                bg_style.fg(palette.text),
            )));
        }

        // Each row is one cell tall, stacked from the top; render only what fits.
        for (i, line) in lines.into_iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).style(bg_style).render(rect, buf);
        }
    }
}

/// The band's row: the headline, then the step glyphs as far as they fit.
/// It reads left-first, so a narrow card keeps what matters: the glyphs go
/// before the headline does, and a strip that cannot fit whole ends in `…`
/// rather than passing for a shorter pipeline.
fn band_line(
    band: &ValidationBand,
    width: usize,
    bg: Style,
    palette: &rataflow::Palette,
    dimmed: bool,
) -> Line<'static> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let color = |tone: CrewTone| {
        if dimmed {
            palette.muted
        } else {
            tone.color(palette)
        }
    };
    let mut style = bg.fg(color(band.tone));
    if band.tone == CrewTone::Attention && !dimmed {
        style = style.add_modifier(Modifier::BOLD);
    }
    let head = truncate(&format!("{} {}", band.glyph, band.headline), width);
    let mut used = head.width();
    let mut spans = vec![Span::styled(head, style)];
    let strip: usize = band.steps.iter().map(|(g, _)| g.width().unwrap_or(1)).sum();
    // A space, and at least one glyph beside the ellipsis.
    if !band.steps.is_empty() && used + 3 <= width {
        spans.push(Span::styled(" ", bg));
        used += 1;
        let whole = used + strip <= width;
        for (glyph, tone) in &band.steps {
            let w = glyph.width().unwrap_or(1);
            if !whole && used + w + 1 > width {
                spans.push(Span::styled("…", bg.fg(palette.muted)));
                break;
            }
            spans.push(Span::styled(glyph.to_string(), bg.fg(color(*tone))));
            used += w;
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_budget() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("hello", 1), "…");
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn truncate_handles_multibyte() {
        // Must not panic on a char boundary; counts columns not bytes.
        let s = "café crème brûlée";
        let out = truncate(s, 5);
        assert_eq!(out.chars().count(), 5);
    }

    #[test]
    fn truncate_measures_display_columns_not_chars() {
        use unicode_width::UnicodeWidthStr;
        // CJK chars are 2 columns wide: 6 chars = 12 columns must truncate to
        // fit a 10-column budget (chars-based counting would pass it through
        // and overflow the card).
        let s = "修复解析错误";
        let out = truncate(s, 10);
        assert!(out.width() <= 10, "{out:?} is {} columns", out.width());
        assert!(out.ends_with('…'));
        // And an exact fit is left alone.
        assert_eq!(truncate(s, 12), s);
    }

    #[test]
    fn token_formatting() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_500), "1.5k");
        assert_eq!(fmt_tokens(2_000_000), "2.0M");
    }

    #[test]
    fn glyphs_per_status() {
        assert_eq!(AgentStatus::Running.glyph(), '●');
        assert_eq!(AgentStatus::Done.glyph(), '✓');
        assert_eq!(AgentStatus::Failed.glyph(), '✗');
    }

    fn render_into(area: ratatui::layout::Rect) -> Buffer {
        use rataflow::Theme;
        use rataflow::types::Position;

        let node = AgentNode {
            title: "claude".into(),
            description: None,
            status: AgentStatus::Done,
            tool_count: 3,
            last_tool: Some("Bash".into()),
            output_tokens: 1200,
            usage: crate::usage::Summary::default(),
            interactive: false,
            pulse: false,
            crew: None,
            validation: None,
        };
        let ctx = NodeRenderContext {
            id: "main",
            area,
            selected: false,
            dragging: false,
            position_absolute: Position::new(0.0, 0.0),
            theme: Theme::default(),
            animation_phase: 0,
        };
        let mut buf = Buffer::empty(area);
        node.render(&ctx, &mut buf);
        buf
    }

    /// A card's inner rows, drawn at `width`.
    fn card_rows(node: &AgentNode, width: u16) -> Vec<String> {
        use rataflow::Theme;
        use rataflow::types::Position;
        let area = ratatui::layout::Rect::new(0, 0, width, 8);
        let ctx = NodeRenderContext {
            id: "main",
            area,
            selected: false,
            dragging: false,
            position_absolute: Position::new(0.0, 0.0),
            theme: Theme::default(),
            animation_phase: 0,
        };
        let mut buf = Buffer::empty(area);
        node.render(&ctx, &mut buf);
        (1..area.height - 1)
            .map(|y| {
                (1..area.width - 1)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim()
                    .to_owned()
            })
            .collect()
    }

    fn validating() -> AgentNode {
        use CrewTone::*;
        let steps = "✓✓✓▸·····".chars().enumerate();
        AgentNode {
            title: "impl".into(),
            description: Some("codex · c0c0c0c0-0000-4000-8000-000000000002".into()),
            status: AgentStatus::Idle,
            tool_count: 7,
            last_tool: Some("shell".into()),
            output_tokens: 0,
            usage: crate::usage::Summary {
                input: 1000,
                output: 200,
                recorded: true,
                usd: Some(0.01),
                ..crate::usage::Summary::default()
            },
            interactive: true,
            pulse: false,
            crew: Some(CrewMark {
                label: "done 08:07:40".into(),
                tone: Settled,
                dimmed: false,
            }),
            validation: Some(ValidationBand {
                glyph: '▸',
                headline: "test r1 · 2m".into(),
                tone: Active,
                steps: steps
                    .map(|(i, g)| (g, if i < 3 { Settled } else { Active }))
                    .collect(),
                heading: Vec::new(),
                rows: Vec::new(),
                current: None,
            }),
        }
    }

    #[test]
    fn a_validation_band_takes_the_description_row_and_keeps_six() {
        let rows = card_rows(&validating(), 32);
        assert_eq!(rows.len(), 6, "{rows:#?}");
        assert!(rows[0].ends_with("impl"), "{rows:#?}");
        assert!(rows[1].starts_with("✓ done"), "{rows:#?}");
        assert_eq!(rows[2], "▸ test r1 · 2m ✓✓✓▸·····");
        assert!(rows[3].starts_with("⚒ 7"), "{rows:#?}");
        assert!(rows.iter().all(|r| !r.contains("c0c0c0c0")), "{rows:#?}");
        assert!(rows[5].contains("API est."), "{rows:#?}");
        // Without a run, the description is back.
        let mut plain = validating();
        plain.validation = None;
        assert!(card_rows(&plain, 32)[2].starts_with("codex · c0c0"));
    }

    #[test]
    fn a_narrow_card_keeps_the_headline_and_drops_glyphs_first() {
        let node = validating();
        // Room for part of the strip: it ends in an ellipsis, never passing
        // for a shorter pipeline.
        assert_eq!(card_rows(&node, 24)[2], "▸ test r1 · 2m ✓✓✓▸…");
        // No room for a glyph beside the ellipsis: the headline alone.
        assert_eq!(card_rows(&node, 19)[2], "▸ test r1 · 2m");
        // Narrower still (8 columns inside): the headline itself is cut,
        // left-first.
        assert_eq!(card_rows(&node, 12)[2], "▸ test …");
        for width in 10..=40 {
            let rows = card_rows(&node, width);
            let inner = usize::from(width.saturating_sub(4));
            for row in &rows {
                assert!(
                    unicode_width::UnicodeWidthStr::width(row.as_str()) <= inner,
                    "{row:?} overflows a {width}-column card"
                );
            }
        }
    }

    #[test]
    fn card_level_renders_text() {
        let area = ratatui::layout::Rect::new(0, 0, 24, 6);
        let buf = render_into(area);
        let text: String = (0..area.width).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(
            text.contains("claude"),
            "card level must show title: {text}"
        );
    }

    #[test]
    fn cell_level_renders_status_fill() {
        use rataflow::Theme;
        use ratatui::style::Color;

        // Below the text threshold: solid status-colored fill, no border.
        let area = ratatui::layout::Rect::new(0, 0, 6, 2);
        let buf = render_into(area);
        let palette = Theme::default().palette();
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &buf[(x, y)];
                assert_eq!(cell.symbol(), " ", "cell level draws no text/border");
                assert_eq!(cell.style().bg, Some(palette.accent)); // Done = calm accent
                assert_ne!(cell.style().bg, Some(Color::Reset));
            }
        }
    }
}
