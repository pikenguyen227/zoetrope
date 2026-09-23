//! Detail panel for the selected agent.
//!
//! When an agent node is selected, this panel takes the right of the canvas and
//! renders the selected agent's description, model, status, timing, usage, and a
//! scrollable list of recent tool calls (name + summary + ✓/✗/⏳). All data
//! comes from the `SessionModel`, keyed by the selected node id. A short panel
//! gives its rows to the tool list first (see [`fit_layout`]).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::state::App;
use crate::state::session::{AgentInfo, ToolState};
use crate::ui::{truncate, truncate_tail, wrap};

/// Line cap for the prompt in the **provenance** header only — it sits in a
/// fixed-height region, so an unbounded prompt would starve the tool list. The
/// era anchors in the scrollable list are NOT capped. Generous: a normal prompt
/// fits well within it (the upstream ~240-char excerpt bounds the raw length).
const PROMPT_MAX_LINES: usize = 6;

/// Cached era-header flags for the selected agent's tool list. Attribution is
/// O(calls × prompts) (`prompt_for_ts` per call), and the panel renders every
/// frame — so recompute only when the agent, its call count, or the prompt
/// count changes (all three are append-only between rebuilds).
pub(crate) struct EraCache {
    agent_id: String,
    calls: usize,
    prompts: usize,
    flags: Vec<bool>,
    total: usize,
}

/// Get-or-recompute the era cache for `agent_id`.
fn era_flags<'a>(
    cache: &'a mut Option<EraCache>,
    agent_id: &str,
    agent: &AgentInfo,
    model: &crate::state::session::SessionModel,
) -> &'a EraCache {
    let stale = cache.as_ref().is_none_or(|c| {
        c.agent_id != agent_id
            || c.calls != agent.tool_calls.len()
            || c.prompts != model.prompts.len()
    });
    if stale {
        let (flags, total) = era_header_flags(agent, model);
        *cache = Some(EraCache {
            agent_id: agent_id.to_string(),
            calls: agent.tool_calls.len(),
            prompts: model.prompts.len(),
            flags,
            total,
        });
    }
    cache.as_ref().unwrap()
}

/// Render the detail panel for `agent_id` into `area`.
///
/// `agent_id` is copied out of the flow before this call to avoid borrowing
/// `app` both immutably (selection) and the panel state. Uses
/// `app.detail_scroll` for the tool-call list scroll offset.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App, agent_id: &str) {
    let palette = app.flow.theme.palette();
    // Split the borrows up front: the era cache is written while the session is
    // read, which a whole-`app` borrow would forbid.
    let App {
        session,
        era_cache,
        detail_scroll,
        detail_follow,
        ..
    } = app;
    let bg = Style::default().bg(palette.surface);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.muted).bg(palette.surface))
        .style(bg)
        .padding(Padding::horizontal(1))
        // Affordance: the way out is visible, not tribal knowledge.
        .title_top(
            Line::from(" esc ✕ ")
                .right_aligned()
                .style(bg.fg(palette.subtle)),
        );
    let inner = block.inner(area);

    let Some(agent) = session.agent(agent_id) else {
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        // Selected node has no model entry (stale selection) — show a hint.
        let para = Paragraph::new(Line::from(Span::styled(
            "no detail for this agent",
            Style::default().fg(palette.muted),
        )))
        .style(Style::default().bg(palette.surface));
        frame.render_widget(para, inner);
        return;
    };

    // Split: header, provenance when known, tools (fill). The prompt is
    // DERIVED from the spawn timestamp's era — same order-independent
    // attribution as the tool-list headers.
    let provenance = session.provenance(agent).and_then(|c| {
        let prompt = session.provenance_prompt(c).map(str::to_string);
        let reasoning = c.reasoning.clone();
        (prompt.is_some() || reasoning.is_some()).then_some((prompt, reasoning))
    });
    let prov_prompt = provenance.as_ref().and_then(|(p, _)| p.as_deref());
    let prov_thought = provenance.as_ref().and_then(|(_, r)| r.as_deref());
    // Prompts are wrapped (not cut) — they're the panel's highest-signal text.
    // Wrap up front so the layout can size the provenance block to fit.
    let prov_text_w = (inner.width as usize).saturating_sub(10);
    let wrap_prov = |text: Option<&str>, cap: usize| {
        text.map(|t| wrap(t, prov_text_w, cap)).unwrap_or_default()
    };

    let list = tool_list(era_cache, agent_id, agent, session, inner.width as usize);
    let layout = fit_layout(
        inner.height,
        list.total,
        agent.tool_calls.is_empty(),
        |compact| {
            let lines = header_lines(agent, inner.width as usize, compact, &palette);
            header_height(agent, lines.len(), inner.height, compact)
        },
        provenance.is_some(),
        wrap_prov(prov_prompt, PROMPT_MAX_LINES).len(),
        wrap_prov(prov_thought, PROMPT_MAX_LINES).len(),
    );
    let header = header_lines(agent, inner.width as usize, layout.compact, &palette);
    let prov_prompt = wrap_prov(prov_prompt, layout.prompt_lines);
    let prov_thought = wrap_prov(prov_thought, layout.thought_lines);
    let prov_rows = if layout.provenance {
        (1 + prov_prompt.len() + prov_thought.len()) as u16
    } else {
        0
    };
    let [header_area, prov_area, tools_area] = Layout::vertical([
        Constraint::Length(layout.header),
        Constraint::Length(prov_rows),
        Constraint::Fill(1),
    ])
    .areas(inner);

    // Scroll indicator whenever the list has rows out of view (the tool
    // block's top border takes one of its rows).
    if list.total > usize::from(tools_area.height.saturating_sub(1)) {
        let n = era_flags(era_cache, agent_id, agent, session).total;
        // "tail" while auto-following the newest call; the line offset once
        // the user has scrolled up (detached).
        let label = if *detail_follow {
            " j/k ↕ tail ".to_string()
        } else {
            format!(" j/k ↕ {}/{} ", detail_scroll, n)
        };
        block = block.title_bottom(
            Line::from(label)
                .right_aligned()
                .style(bg.fg(palette.subtle)),
        );
    }
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    render_header(frame, header_area, header, &palette);
    if layout.provenance {
        render_provenance(frame, prov_area, &prov_prompt, &prov_thought, &palette);
    }
    // The panel auto-tails the newest call by default; scrolling up detaches it
    // (its own state, independent of the graph camera). The renderer clamps the
    // offset to the real maximum and writes it (+ the re-attach) back.
    render_tools(
        frame,
        tools_area,
        agent,
        &list,
        detail_scroll,
        detail_follow,
        &palette,
    );
}

/// Tool rows the list keeps before the header and provenance get theirs: the
/// list is what the panel is for, so a short panel trims the context above it
/// rather than hiding calls.
const TOOLS_MIN_ROWS: u16 = 8;

/// How the panel's rows are shared out.
#[derive(Debug, PartialEq)]
struct PanelLayout {
    /// Header height.
    header: u16,
    /// The header folds the usage rows into one line.
    compact: bool,
    /// Whether the provenance block is drawn at all.
    provenance: bool,
    /// Line caps for the provenance prompt and thought.
    prompt_lines: usize,
    thought_lines: usize,
}

/// Share `height` rows between header, provenance and the tool list. With room,
/// everything is drawn in full. Otherwise the tool list keeps up to
/// [`TOOLS_MIN_ROWS`] rows (plus its border) and the context above it gives way
/// in order: the header folds its usage rows into one line, then the
/// provenance prompt and thought lose lines (ellipsized), longest first, and
/// finally the provenance block goes.
fn fit_layout(
    height: u16,
    list_rows: usize,
    no_calls: bool,
    header_height: impl Fn(bool) -> u16,
    provenance: bool,
    prompt: usize,
    thought: usize,
) -> PanelLayout {
    // "no tool calls" still takes a row.
    let want = if no_calls { 1 } else { list_rows };
    let tools = (want.min(usize::from(TOOLS_MIN_ROWS)) + 1) as u16;
    let room = height.saturating_sub(tools);
    let prov_rows = |p: usize, t: usize| if provenance { (1 + p + t) as u16 } else { 0 };

    let full = header_height(false);
    if full + prov_rows(prompt, thought) <= room {
        return PanelLayout {
            header: full,
            compact: false,
            provenance,
            prompt_lines: PROMPT_MAX_LINES,
            thought_lines: PROMPT_MAX_LINES,
        };
    }
    let header = header_height(true);
    let (mut p, mut t) = (prompt, thought);
    let budget = room.saturating_sub(header);
    while provenance && prov_rows(p, t) > budget && p + t > 0 {
        if p > t { p -= 1 } else { t -= 1 }
    }
    let provenance = provenance && p + t > 0 && prov_rows(p, t) <= budget;
    PanelLayout {
        header,
        compact: true,
        provenance,
        prompt_lines: p,
        thought_lines: t,
    }
}

/// Header rows: the usual height, grown to fit longer text (a fleet node's
/// task detail) but never past a third of the panel, so the tools stay visible.
fn header_height(agent: &AgentInfo, rows: usize, panel_height: u16, compact: bool) -> u16 {
    let base = if agent.usage.summary.recorded && !compact {
        8
    } else {
        6
    };
    let cap = base.max(panel_height / 3);
    (rows as u16).clamp(base, cap)
}

fn render_header(frame: &mut Frame, area: Rect, lines: Vec<Line>, palette: &rataflow::Palette) {
    let bg = Style::default().bg(palette.surface);
    frame.render_widget(
        Paragraph::new(lines).style(bg).wrap(Wrap { trim: true }),
        area,
    );
}

/// The header's lines, text rows pre-wrapped to `width` so the count is the
/// rendered height. `compact` folds the usage rows into one `tools · tok ·
/// cost` line for a panel too short to show them in full.
fn header_lines(
    agent: &AgentInfo,
    width: usize,
    compact: bool,
    palette: &rataflow::Palette,
) -> Vec<Line<'static>> {
    // Single-source vocabulary + presence colors (shared with cards/inspect).
    let status_text = agent.status_word();
    let status_color = crate::ui::status_color(agent.status, palette);

    let bg = Style::default().bg(palette.surface);

    let mut lines: Vec<Line> = Vec::new();

    // Title: agent type, bold.
    let title = agent
        .agent_type
        .as_deref()
        .unwrap_or(agent.kind.default_label());
    lines.push(Line::from(Span::styled(
        title.to_string(),
        bg.fg(palette.text).add_modifier(Modifier::BOLD),
    )));

    // Status + model.
    let mut status_spans = vec![Span::styled(status_text.to_string(), bg.fg(status_color))];
    if let Some(model) = agent.model.as_ref() {
        status_spans.push(Span::styled("  ", bg));
        status_spans.push(Span::styled(model.clone(), bg.fg(palette.subtle)));
    }
    lines.push(Line::from(status_spans));

    // Timing: duration if both ends known, else first seen.
    if let Some(timing) = fmt_timing(agent) {
        lines.push(Line::from(Span::styled(timing, bg.fg(palette.muted))));
    }

    // Counts: tools + tokens. A `Span` drops control characters, so each row
    // of multi-line text needs its own `Line`.
    let counts = if agent.usage.summary.recorded && compact {
        let u = &agent.usage.summary;
        vec![format!(
            "{} tools · {} tok{} · {}",
            agent.tool_calls.len(),
            u.total(),
            if u.incomplete { "+" } else { "" },
            u.cost_label()
        )]
    } else if agent.usage.summary.recorded {
        let u = &agent.usage.summary;
        vec![
            format!(
                "{} tools · {} in + {} out = {} tok{} · {}",
                agent.tool_calls.len(),
                u.input,
                u.output,
                u.total(),
                if u.incomplete { "+ (partial)" } else { "" },
                u.cost_label()
            ),
            format!(
                "Cache: {} read / {} written · standard API equivalent, not your bill",
                u.cached, u.cache_write
            ),
        ]
    } else {
        vec![format!(
            "{} tools · {} output tok · total/cost unavailable",
            agent.tool_calls.len(),
            agent.output_tokens
        )]
    };
    let wrapped = |row: &str, style: Style| {
        let mut rows = crate::ui::wrap(row, width, usize::MAX);
        if rows.is_empty() {
            rows.push(String::new());
        }
        rows.into_iter()
            .map(move |r| Line::from(Span::styled(r, style)))
            .collect::<Vec<_>>()
    };
    for row in &counts {
        lines.extend(wrapped(row, bg.fg(palette.muted)));
    }

    // Description (wrapped) on the remaining rows.
    if let Some(desc) = agent.description.as_ref().filter(|d| !d.is_empty()) {
        for row in desc.lines() {
            lines.extend(wrapped(row, bg.fg(palette.subtle)));
        }
    }
    lines
}

/// "Why does this agent exist": the triggering prompt + the assistant's
/// reasoning right before the spawn.
fn render_provenance(
    frame: &mut Frame,
    area: Rect,
    prompt: &[String],
    reasoning: &[String],
    palette: &rataflow::Palette,
) {
    if area.height == 0 {
        return;
    }
    let bg = Style::default().bg(palette.surface);
    let label = bg.fg(palette.accent);
    let width = area.width as usize;

    let block = Style::default().bg(palette.muted);

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "─ triggered by ".to_string() + &"─".repeat(width.saturating_sub(15)),
        bg.fg(palette.muted),
    ))];
    // The user's prompt: a label on the first line, continuations indented, all
    // on a full-width subtle GRAY block — a quiet anchor, since gold is reserved
    // for agent activity/focus, not context.
    for (i, l) in prompt.iter().enumerate() {
        let prefix = if i == 0 { "↳ prompt  " } else { "          " };
        let pad = width.saturating_sub(
            unicode_width::UnicodeWidthStr::width(prefix)
                + unicode_width::UnicodeWidthStr::width(l.as_str()),
        );
        lines.push(Line::from(vec![
            Span::styled(prefix, block.fg(palette.accent)),
            Span::styled(l.clone(), block.fg(palette.text)),
            Span::styled(" ".repeat(pad), block),
        ]));
    }
    // The assistant's reasoning: dim, no highlight (not the user's words).
    for (i, l) in reasoning.iter().enumerate() {
        let prefix = if i == 0 { "↳ thought " } else { "          " };
        lines.push(Line::from(vec![
            Span::styled(prefix, label),
            Span::styled(l.clone(), bg.fg(palette.subtle)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).style(bg), area);
}

/// The tool list's era headers, wrapped, and its total virtual line count —
/// computed once per frame, before layout, so the panel can size itself and
/// its scroll indicator against what the list will really draw.
struct ToolList {
    headers: Vec<Option<Vec<String>>>,
    total: usize,
}

fn tool_list(
    era_cache: &mut Option<EraCache>,
    agent_id: &str,
    agent: &AgentInfo,
    model: &crate::state::session::SessionModel,
    width: usize,
) -> ToolList {
    // Prompt-era group headers: a separator whenever consecutive calls fall
    // under a different user prompt (timestamp-derived, cached — see
    // [`EraCache`]). Skipped when the whole list shares one era — the
    // provenance section already names it.
    let header_before = &era_flags(era_cache, agent_id, agent, model).flags;

    // Wrap the (few) era headers and total the virtual line count — WITHOUT
    // building a styled Line per tool call. Only the viewport's worth of rows
    // is materialized when drawing; formatting every call of a tool-heavy
    // agent each frame dominated render time.
    let mut headers: Vec<Option<Vec<String>>> = Vec::with_capacity(agent.tool_calls.len());
    let mut total = 0usize;
    for (tc, is_header) in agent.tool_calls.iter().zip(header_before) {
        let wrapped = if *is_header
            && let Some(e) = model.prompt_for_ts(tc.ts)
            && let Some(p) = model.prompts.get(e)
        {
            // Era anchor: the user prompt that starts this group. NOT
            // line-capped: it lives in the scrollable list, so a long prompt
            // just takes more rows (the upstream ~240-char excerpt bounds it).
            Some(wrap(&p.excerpt, width.saturating_sub(2), usize::MAX))
        } else {
            None
        };
        total += wrapped.as_ref().map_or(0, Vec::len) + 1;
        headers.push(wrapped);
    }
    ToolList { headers, total }
}

fn render_tools(
    frame: &mut Frame,
    area: Rect,
    agent: &AgentInfo,
    list: &ToolList,
    detail_scroll: &mut u16,
    detail_follow: &mut bool,
    palette: &rataflow::Palette,
) {
    let bg = Style::default().bg(palette.surface);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(bg.fg(palette.muted))
        .title(Span::styled(" tool calls ", bg.fg(palette.subtle)))
        .style(bg);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if agent.tool_calls.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no tool calls",
                bg.fg(palette.muted),
            )))
            .style(bg),
            inner,
        );
        return;
    }

    let width = inner.width as usize;
    let (headers, total) = (&list.headers, list.total);

    // Resolve the scroll + tail state against the real line count, and write both
    // back so the scroll indicator and the next keypress match what's on screen.
    let (scroll, follow) = resolve_scroll(
        total.min(u16::MAX as usize) as u16,
        inner.height,
        *detail_scroll,
        *detail_follow,
    );
    *detail_scroll = scroll;
    *detail_follow = follow;

    // Pass 2: materialize only the rows intersecting the viewport, rendering
    // with a residual scroll from the first materialized row.
    let view_start = scroll as usize;
    let view_end = view_start + inner.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize + 4);
    let mut idx = 0usize;
    let mut first_built: Option<usize> = None;
    for (tc, wrapped) in agent.tool_calls.iter().zip(headers) {
        let rows = wrapped.as_ref().map_or(0, Vec::len) + 1;
        if idx + rows > view_start && idx < view_end {
            if first_built.is_none() {
                first_built = Some(idx);
            }
            if let Some(wrapped) = wrapped {
                // Wrapped onto a subtle GRAY block (prompts are context, not
                // the agent activity gold is reserved for) — thin gold tick +
                // bright text, padded full-width so the band reads.
                let block = Style::default().bg(palette.muted);
                for l in wrapped {
                    let pad =
                        width.saturating_sub(2 + unicode_width::UnicodeWidthStr::width(l.as_str()));
                    lines.push(Line::from(vec![
                        Span::styled("▍ ", block.fg(palette.accent)),
                        Span::styled(l.clone(), block.fg(palette.text)),
                        Span::styled(" ".repeat(pad), block),
                    ]));
                }
            }
            lines.push(tool_line(tc, width, palette));
        }
        idx += rows;
        if idx >= view_end {
            break;
        }
    }
    let local_scroll = scroll.saturating_sub(first_built.unwrap_or(0) as u16);
    frame.render_widget(
        Paragraph::new(lines).style(bg).scroll((local_scroll, 0)),
        inner,
    );
}

/// Resolve the panel's scroll offset for one render: clamp to the reachable
/// maximum (keep the last screenful in view — no over-scroll into blank) and
/// reconcile the tail. Following pins to the bottom; scrolling back down to the
/// bottom (or content that fits) re-attaches. Returns `(offset, tailing)`.
fn resolve_scroll(total: u16, height: u16, scroll: u16, follow: bool) -> (u16, bool) {
    let max = total.saturating_sub(height);
    let offset = if follow { max } else { scroll.min(max) };
    (offset, offset >= max)
}

/// Which tool rows get an era header above them, plus the total rendered
/// line count (rows + headers). Shared by the renderer, the scroll clamp
/// (handler), and the scroll indicator so they can never disagree about the
/// list's true length.
fn era_header_flags(
    agent: &AgentInfo,
    model: &crate::state::session::SessionModel,
) -> (Vec<bool>, usize) {
    let mut flags = Vec::with_capacity(agent.tool_calls.len());
    let mut distinct = 0usize;
    let mut prev: Option<usize> = None;
    let mut headers = 0usize;
    for tc in &agent.tool_calls {
        let era = model.prompt_for_ts(tc.ts);
        let is_boundary = matches!(era, Some(e) if prev != Some(e));
        if is_boundary {
            distinct += 1;
        }
        flags.push(is_boundary);
        if let Some(e) = era {
            prev = Some(e);
        }
        if is_boundary {
            headers += 1;
        }
    }
    // Single-era lists get no headers — the provenance section names it.
    if distinct <= 1 {
        return (vec![false; agent.tool_calls.len()], agent.tool_calls.len());
    }
    (flags, agent.tool_calls.len() + headers)
}

/// One row of the tool-call list: state glyph, name, summary, local time.
fn tool_line(
    tc: &crate::state::session::ToolCallInfo,
    width: usize,
    palette: &rataflow::Palette,
) -> Line<'static> {
    let bg = Style::default().bg(palette.surface);
    let (glyph, color) = match tc.state {
        ToolState::Pending => ('⏳', palette.accent),
        ToolState::Ok => ('✓', palette.success),
        ToolState::Err => ('✗', palette.error),
    };
    let w = |s: &str| unicode_width::UnicodeWidthStr::width(s);
    let head = format!("{glyph} ");
    let mut used = w(&head) + w(tc.name.as_str());
    let mut spans = vec![
        Span::styled(head, bg.fg(color)),
        Span::styled(
            tc.name.clone(),
            bg.fg(palette.text).add_modifier(Modifier::BOLD),
        ),
    ];
    // Recorded transcript timestamps (UTC on the wire), shown in the viewer's
    // local time — and RIGHT-ALIGNED to the panel edge, not tacked onto the end
    // of the summary (which left it floating mid-line on a wide panel).
    let time = tc.ts.map(|t| {
        t.with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string()
    });
    let time_w = time.as_ref().map(|t| t.chars().count() + 1).unwrap_or(0);
    if let Some(summary) = tc.summary.as_ref().filter(|s| !s.is_empty()) {
        // The summary fills the space between the name and the right-aligned
        // time — the full panel width, so a wide screen shows more of it.
        let budget = width.saturating_sub(used + 1 + time_w);
        // Path tools: keep the basename (truncate the front); everything else
        // front-loads its meaning, so keep the head.
        let summary = if matches!(tc.name.as_str(), "Read" | "Write" | "Edit") {
            truncate_tail(summary, budget)
        } else {
            truncate(summary, budget)
        };
        used += 1 + w(&summary);
        spans.push(Span::styled(format!(" {summary}"), bg.fg(palette.subtle)));
    }
    if let Some(time) = time {
        // Pad from the content out to where the flush-right time begins.
        let pad = width.saturating_sub(used + time_w);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg));
        }
        spans.push(Span::styled(format!(" {time}"), bg.fg(palette.muted)));
    }
    Line::from(spans)
}

/// Format a timing line from an agent's first/last timestamps.
fn fmt_timing(agent: &AgentInfo) -> Option<String> {
    match (agent.first_ts, agent.last_ts) {
        (Some(first), Some(last)) => {
            let secs = (last - first).num_seconds().max(0);
            if secs >= 60 {
                Some(format!("⏱ {}m {}s", secs / 60, secs % 60))
            } else {
                Some(format!("⏱ {secs}s"))
            }
        }
        (Some(first), None) => Some(format!(
            "⏱ started {}",
            first.with_timezone(&chrono::Local).format("%H:%M:%S")
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_scroll_clamps_and_reconciles_tail() {
        // Content shorter than the viewport → always tailing, offset 0.
        assert_eq!(resolve_scroll(5, 10, 3, false), (0, true));
        // Following → pinned to the bottom (max = 20 - 8 = 12).
        assert_eq!(resolve_scroll(20, 8, 0, true), (12, true));
        // Detached and scrolled up → keep the offset, stay detached.
        assert_eq!(resolve_scroll(20, 8, 5, false), (5, false));
        // Detached but (over-)scrolled to the bottom → clamp + re-attach.
        assert_eq!(resolve_scroll(20, 8, 99, false), (12, true));
    }

    #[test]
    fn tool_list_lines_counts_era_headers() {
        use crate::provider::claude::wire::parse_line;
        use crate::provider::claude::{Record, Source};
        use crate::state::session::{SessionModel, ToolCallInfo, ToolState};

        let mut m = SessionModel::new("s".into());
        for (uid, ts, text) in [
            ("p1", "2026-06-07T10:00:00.000Z", "first"),
            ("p2", "2026-06-07T11:00:00.000Z", "second"),
        ] {
            let line = format!(
                r#"{{"type":"user","uuid":"{uid}","parentUuid":null,"origin":{{"kind":"human"}},"timestamp":"{ts}","message":{{"role":"user","content":"{text}"}}}}"#
            );
            m.apply_update(&Record::Entry {
                source: Source::Main,
                entry: parse_line(&line).unwrap(),
            });
        }
        let agent = m.agents.get_mut(crate::state::session::MAIN_ID).unwrap();
        for (i, ts) in [
            "2026-06-07T10:30:00.000Z",
            "2026-06-07T11:30:00.000Z",
            "2026-06-07T11:31:00.000Z",
        ]
        .iter()
        .enumerate()
        {
            agent.tool_calls.push_back(ToolCallInfo {
                id: format!("t{i}"),
                name: "Bash".into(),
                summary: None,
                ts: Some(ts.parse().unwrap()),
                end_ts: None,
                state: ToolState::Ok,
            });
        }
        let agent = m.agent(crate::state::session::MAIN_ID).unwrap();
        // 3 tool rows + 2 era headers (eras 0 and 1) = 5 rendered lines —
        // the scroll ceiling the handler clamps against.
        assert_eq!(era_header_flags(agent, &m).1, 5);
    }

    /// Render the header, sized as the panel sizes it for a panel of
    /// `panel_height` rows, into a buffer and return its rows as text.
    fn header_rows(agent: &AgentInfo, width: u16, panel_height: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let palette = rataflow::Theme::default().palette();
        let lines = header_lines(agent, width as usize, false, &palette);
        let height = header_height(agent, lines.len(), panel_height, false);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render_header(frame, frame.area(), lines, &palette))
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn header_breaks_multi_line_text_into_rows() {
        // A `Span` drops control characters, so an embedded `\n` would glue
        // the rows together instead of breaking them.
        let mut a = AgentInfo::new(crate::state::session::AgentKind::Subagent);
        a.usage.summary = crate::usage::Summary {
            input: 10,
            output: 5,
            cached: 7,
            cache_write: 3,
            recorded: true,
            incomplete: false,
            usd: Some(0.5),
            approximate: false,
        };
        a.description = Some("waiting for session registration\ntask: queued (manifest)".into());
        let rows = header_rows(&a, 120, 24);
        assert_eq!(rows.len(), 8, "{rows:#?}");
        let row = |prefix: &str| {
            rows.iter()
                .position(|r| r.starts_with(prefix))
                .unwrap_or_else(|| panic!("no row starting {prefix:?} in {rows:#?}"))
        };
        let counts = row("0 tools · 10 in + 5 out = 15 tok · API est. $0.500");
        assert!(rows[counts].ends_with("$0.500"), "{rows:#?}");
        assert_eq!(row("Cache: 7 read / 3 written"), counts + 1);
        let desc = row("waiting for session registration");
        assert_eq!(rows[desc], "waiting for session registration");
        assert_eq!(row("task: queued (manifest)"), desc + 1);
    }

    #[test]
    fn header_grows_to_show_a_fleet_workers_task_detail() {
        // A 70-column panel: 66 columns inside the border and padding.
        let mut a = AgentInfo::new(crate::state::session::AgentKind::Subagent);
        a.agent_type = Some("Worker · Implementation".into());
        a.first_ts = Some(chrono::Utc::now());
        a.usage.summary = crate::usage::Summary {
            input: 10000,
            output: 800,
            cached: 5000,
            cache_write: 0,
            recorded: true,
            incomplete: false,
            usd: Some(0.095),
            approximate: false,
        };
        a.description = Some(
            "codex · 22222222-2222-2222-2222-222222222222\n\
             task demo-worker-1 [attempt-1]: working (synthetic fixture, 2026-09-22 00:00:00 UTC)\n\
             runtime: 12m (manifest)\n\
             UNAVAILABLE: transcript unreadable"
                .into(),
        );
        let rows = header_rows(&a, 66, 40);
        let text = rows.join("\n");
        for tail in [
            "2026-09-22 00:00:00 UTC)",
            "runtime: 12m (manifest)",
            "UNAVAILABLE: transcript unreadable",
        ] {
            assert!(text.contains(tail), "{tail:?} clipped from {rows:#?}");
        }
        // The graph keeps the rest: never more than a third of the panel.
        let short = header_rows(&a, 66, 24);
        assert_eq!(short.len(), 8, "{short:#?}");
    }

    /// The Codex 0.153.4 capture under `assets/`, folded as a finished replay.
    fn codex_capture() -> Option<App> {
        use crate::provider::codex::{Stream, discovery};
        let dir = crate::provider::harness::fixture_dir("codex")?.join("cli-0.153.4");
        let root = discovery::all_rollouts(&dir)
            .into_iter()
            .find(|p| discovery::read_meta(p).is_some_and(|m| m.is_root()))?;
        let mut app = App::new("cli-0.153.4".into(), crate::state::Mode::Live);
        for (path, _) in discovery::session_rollouts(&root) {
            let mut stream = Stream::new();
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                for fact in stream.push(line).map(|s| s.facts).unwrap_or_default() {
                    app.session.apply_fact(&fact);
                }
            }
        }
        app.session.recompute_group_status();
        app.session.end_of_stream();
        Some(app)
    }

    /// Draw the panel for `id` into a `width`×`height` area and return its
    /// rows, trailing blanks trimmed.
    fn panel_rows(app: &mut App, id: &str, width: u16, height: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), app, id))
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn tool_rows(rows: &[String]) -> usize {
        rows.iter()
            .filter(|r| r.starts_with("│ ✓ ") || r.starts_with("│ ✗ "))
            .count()
    }

    const EXPLORE: &str = "01a07c0b-7aaf-7d43-bca3-9b01faca045d";
    const IMPLEMENT: &str = "01a07c0b-8d0c-7022-a0df-f7d14290e076";

    #[test]
    fn a_120x36_terminal_shows_every_call_of_a_seven_call_subagent() {
        // A 120×36 terminal gives the panel 40% of 120 columns and the canvas
        // height, 36 minus the 8-row timeline and the status bar: 48×27.
        let Some(mut app) = codex_capture() else {
            return;
        };
        assert_eq!(app.session.agent(EXPLORE).unwrap().tool_calls.len(), 7);
        let rows = panel_rows(&mut app, EXPLORE, 48, 27);
        assert_eq!(tool_rows(&rows), 7, "{rows:#?}");
        // Nothing hidden, so no scroll hint.
        assert!(!rows.last().unwrap().contains("j/k"), "{rows:#?}");
        // The usage still reads, folded into one line.
        assert!(
            rows.iter()
                .any(|r| r.contains("7 tools · 84281 tok · API est. $0.039")),
            "{rows:#?}"
        );
        // The provenance still says what triggered the agent.
        assert!(rows.iter().any(|r| r.contains("↳ prompt")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("↳ thought")), "{rows:#?}");
    }

    #[test]
    fn hidden_calls_always_get_the_scroll_hint() {
        let Some(mut app) = codex_capture() else {
            return;
        };
        // 17 calls cannot fit a 48×27 panel: the list keeps its minimum rows
        // and says more are out of view.
        let rows = panel_rows(&mut app, IMPLEMENT, 48, 27);
        assert_eq!(tool_rows(&rows), usize::from(TOOLS_MIN_ROWS), "{rows:#?}");
        assert!(rows.last().unwrap().contains("j/k ↕ tail"), "{rows:#?}");
        // Seven calls in a panel too short for them: the hint appears though
        // the list is under the old nine-call threshold.
        let rows = panel_rows(&mut app, EXPLORE, 48, 12);
        assert!(tool_rows(&rows) < 7, "{rows:#?}");
        assert!(rows.last().unwrap().contains("j/k ↕ tail"), "{rows:#?}");
    }

    #[test]
    fn a_roomy_panel_keeps_the_full_usage_header() {
        // 200×50: an 80×41 panel has room for everything as it was.
        let Some(mut app) = codex_capture() else {
            return;
        };
        let rows = panel_rows(&mut app, EXPLORE, 80, 41);
        assert_eq!(tool_rows(&rows), 7, "{rows:#?}");
        assert!(
            rows.iter()
                .any(|r| r.contains("7 tools · 83471 in + 810 out = 84281 tok · API est. $0.039")),
            "{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("Cache: 76544 read / 0 written")),
            "{rows:#?}"
        );
    }

    #[test]
    fn a_short_panel_trims_context_before_tools() {
        let header = |compact: bool| if compact { 6 } else { 8 };
        // Room for everything: nothing gives way.
        let full = fit_layout(40, 7, false, header, true, 6, 6);
        assert!(!full.compact && full.provenance);
        assert_eq!((full.prompt_lines, full.thought_lines), (6, 6));
        // 25 rows, 7 calls (8 with the border): the header folds, then the
        // longer provenance part loses lines first.
        let fit = fit_layout(25, 7, false, header, true, 6, 3);
        assert!(fit.compact && fit.provenance);
        assert_eq!(fit.header, 6);
        assert_eq!((fit.prompt_lines, fit.thought_lines), (6, 3));
        let fit = fit_layout(22, 7, false, header, true, 6, 6);
        assert_eq!((fit.prompt_lines, fit.thought_lines), (4, 3));
        // No room at all for provenance: it goes rather than squeezing tools.
        let fit = fit_layout(15, 7, false, header, true, 6, 6);
        assert!(!fit.provenance);
    }

    #[test]
    fn header_keeps_its_height_without_extra_rows() {
        let a = AgentInfo::new(crate::state::session::AgentKind::Main);
        assert_eq!(header_rows(&a, 66, 60).len(), 6);
    }

    use chrono::{TimeZone, Utc};

    fn agent_with_ts(first: Option<i64>, last: Option<i64>) -> AgentInfo {
        let mut a = AgentInfo::new(crate::state::session::AgentKind::Subagent);
        a.first_ts = first.map(|s| Utc.timestamp_opt(s, 0).unwrap());
        a.last_ts = last.map(|s| Utc.timestamp_opt(s, 0).unwrap());
        a
    }

    #[test]
    fn timing_duration_under_a_minute() {
        let a = agent_with_ts(Some(100), Some(142));
        assert_eq!(fmt_timing(&a).as_deref(), Some("⏱ 42s"));
    }

    #[test]
    fn timing_duration_over_a_minute() {
        let a = agent_with_ts(Some(0), Some(125));
        assert_eq!(fmt_timing(&a).as_deref(), Some("⏱ 2m 5s"));
    }

    #[test]
    fn timing_negative_clamped() {
        let a = agent_with_ts(Some(100), Some(50));
        assert_eq!(fmt_timing(&a).as_deref(), Some("⏱ 0s"));
    }

    #[test]
    fn timing_none_when_no_first() {
        let a = agent_with_ts(None, None);
        assert!(fmt_timing(&a).is_none());
    }
}
