//! The headless text view of a session: what `zoe inspect` prints, and what a
//! provider's golden test compares against. One renderer for both, so the
//! thing a human reads to check a provider is the thing the test checks.

use crate::fact::AgentKind;
use crate::state::info::SessionInfo;
use crate::state::session::{SessionModel, ToolState};

/// The session header: title, the provider's labelled rows, and totals.
pub fn header(model: &SessionModel, info: &SessionInfo) -> String {
    let mut out = String::new();
    let title = info.title.as_deref().unwrap_or("(untitled)");
    out.push_str(&format!("session {} — {title}\n", model.session_id));
    for (label, value) in &info.fields {
        out.push_str(&format!("  {label}: {value}\n"));
    }
    let tallies = info
        .tallies
        .iter()
        .map(|(label, n)| format!("{n} {label}"))
        .collect::<Vec<_>>()
        .join(" · ");
    out.push_str(&format!(
        "  {} agent(s), {} tool call(s)",
        model.agent_count(),
        model.tool_count()
    ));
    if !tallies.is_empty() {
        out.push_str(&format!(" · {tallies}"));
    }
    out.push('\n');
    out
}

/// The agent tree: roots first, children indented underneath, in spawn order.
pub fn agents(model: &SessionModel) -> String {
    let mut out = String::new();
    tree(model, None, 0, &mut out);
    out
}

/// Header, a blank line, then the tree: the whole `inspect` report.
pub fn report(model: &SessionModel, info: &SessionInfo) -> String {
    format!("{}\n{}", header(model, info), agents(model))
}

fn tree(model: &SessionModel, parent: Option<&str>, depth: usize, out: &mut String) {
    for id in model.spawn_order() {
        let Some(agent) = model.agent(id) else {
            continue;
        };
        if agent.parent.as_deref() != parent {
            continue;
        }

        let indent = "  ".repeat(depth + 1);
        let kind = match agent.kind {
            AgentKind::Main => "main",
            AgentKind::Subagent => "subagent",
            AgentKind::Group => "group",
        };
        // Single source: same wording + glyph the cards/panel use.
        let status = agent.status_word();
        let glyph = agent.status.glyph();

        let label = agent
            .agent_type
            .as_deref()
            .or(agent.description.as_deref())
            .unwrap_or(id);

        let (mut ok, mut err, mut pending) = (0u32, 0u32, 0u32);
        for t in agent.tool_calls() {
            match t.state {
                ToolState::Ok => ok += 1,
                ToolState::Err => err += 1,
                ToolState::Pending => pending += 1,
            }
        }

        out.push_str(&format!(
            "{indent}{glyph} [{kind}] {label}  ({status}) — id={id}\n"
        ));
        if let Some(desc) = &agent.description
            && agent.agent_type.is_some()
        {
            out.push_str(&format!("{indent}    {desc}\n"));
        }
        if let Some(model_name) = &agent.model {
            out.push_str(&format!("{indent}    model: {model_name}\n"));
        }
        // Tokens as the card and panel count them: the recorded input (cache
        // reads and writes included) plus output, and the API-equivalent
        // cost; output alone when no input was recorded.
        let u = &agent.usage.summary;
        let tokens = if u.recorded {
            format!(
                "{}{} ({} in + {} out) · {}",
                u.total(),
                if u.incomplete { "+" } else { "" },
                u.input,
                u.output,
                u.cost_label()
            )
        } else {
            format!("{} output · total/cost unavailable", agent.output_tokens)
        };
        out.push_str(&format!(
            "{indent}    tools: {} ({ok}✓ {err}✗ {pending}⏳)   tokens: {tokens}\n",
            agent.tool_calls().len(),
        ));
        // Provenance: what triggered this agent (the panel's `↳ prompt`/`↳ thought`).
        if let Some(ctx) = model.provenance(agent) {
            if let Some(prompt) = model.provenance_prompt(ctx) {
                out.push_str(&format!("{indent}    ↳ prompt: {prompt}\n"));
            }
            if let Some(reasoning) = &ctx.reasoning {
                out.push_str(&format!("{indent}    ↳ thought: {reasoning}\n"));
            }
        }

        // Recurse into this agent's children (groups have subagent children,
        // main has direct subagents + groups).
        tree(model, Some(id), depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::session::MAIN_ID;

    /// The main agent's `tools:` row as `inspect` prints it.
    fn tokens_row(model: &SessionModel) -> String {
        agents(model)
            .lines()
            .find(|l| l.trim_start().starts_with("tools:"))
            .unwrap()
            .trim()
            .to_string()
    }

    fn with_usage(summary: crate::usage::Summary) -> SessionModel {
        let mut model = SessionModel::new("s".into());
        let main = model.agents.get_mut(MAIN_ID).unwrap();
        main.output_tokens = 5;
        main.usage.summary = summary;
        model
    }

    #[test]
    fn tokens_read_as_the_card_counts_them() {
        // Input (cache reads and writes included) plus output, then the cost:
        // the total the card shows, not the output alone.
        let model = with_usage(crate::usage::Summary {
            input: 1000,
            output: 20,
            cached: 800,
            recorded: true,
            usd: Some(0.0124),
            ..Default::default()
        });
        assert_eq!(
            tokens_row(&model),
            "tools: 0 (0✓ 0✗ 0⏳)   tokens: 1020 (1000 in + 20 out) · API est. $0.012"
        );
    }

    #[test]
    fn partial_and_unpriced_usage_say_so() {
        let model = with_usage(crate::usage::Summary {
            input: 10,
            output: 2,
            recorded: true,
            incomplete: true,
            ..Default::default()
        });
        assert_eq!(
            tokens_row(&model),
            "tools: 0 (0✓ 0✗ 0⏳)   tokens: 12+ (10 in + 2 out) · API est. —"
        );
    }

    #[test]
    fn output_alone_when_no_input_was_recorded() {
        let model = with_usage(crate::usage::Summary::default());
        assert_eq!(
            tokens_row(&model),
            "tools: 0 (0✓ 0✗ 0⏳)   tokens: 5 output · total/cost unavailable"
        );
    }
}
