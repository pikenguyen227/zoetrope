//! The validation band: the no-mistakes run Firstmate attributed to an
//! attempt, as of the playhead, on its card and in its reading panel.
//!
//! Every time shown says how it is known. A run's start comes from its ID;
//! a change is placed between the read that found it and the read before
//! (`08:10:00–08:10:30`), or only bounded (`by 08:10:30`) when no earlier read
//! is known; an active round's start is derived from the age a read gave.
//! Where the adapter was not reading the run, or once the daemon behind its
//! record was down, the band keeps the last read but marks it `?`
//! (unverified). A run that had ended cannot change, so it is never in doubt;
//! an open gate keeps its colour, since it stays open until someone answers.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::clock;
use super::journal::{Attempt, Gate, Lifecycle, Phase, RunView, Step};
use crate::ui::nodes::{CrewTone, ValidationBand};

/// The band for the newest run among `attempts`, as of `at` (`None`: the
/// live edge, where `now` is the wall clock), verified as of the wall clock
/// `clock`.
pub(super) fn band_for<'a>(
    lifecycle: &Lifecycle,
    runs: &BTreeMap<Attempt, RunView>,
    attempts: impl Iterator<Item = &'a Attempt>,
    at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    clock: DateTime<Utc>,
) -> Option<ValidationBand> {
    let (attempt, view) = attempts
        .filter_map(|a| Some((a, runs.get(a)?)))
        .max_by_key(|(_, view)| view.start())?;
    Some(band(
        view,
        lifecycle.run_verified(attempt, &view.run, at, clock),
        now,
    ))
}

/// `view` as a band. `verified`: whether the adapter was reading the run at
/// that moment.
pub(super) fn band(view: &RunView, verified: bool, now: DateTime<Utc>) -> ValidationBand {
    let doubt = !view.ended() && (!verified || view.lost.is_some());
    let (mut glyph, mut tone, mut headline) = headline(view, (!doubt).then_some(now));
    let read = view.read.as_ref().map(|(_, read)| read);
    let gate = read.and_then(|r| r.gate.as_ref());
    if doubt {
        glyph = '?';
        if tone != CrewTone::Attention {
            tone = CrewTone::Unknown;
        }
        match view.lost {
            Some((Phase::DaemonDown, _)) => headline.push_str(" · daemon down"),
            Some((Phase::Gone, _)) => headline.push_str(" · record gone"),
            _ => {}
        }
    }
    let steps = read.map_or_else(Vec::new, |read| {
        read.steps
            .iter()
            .map(|step| {
                let (glyph, tone) = step_glyph(step, gate);
                (glyph, if doubt { CrewTone::Unknown } else { tone })
            })
            .collect()
    });
    let rows = read.map_or_else(Vec::new, |read| {
        read.steps
            .iter()
            .map(|step| {
                let (glyph, tone) = step_glyph(step, gate);
                (
                    glyph,
                    if doubt { CrewTone::Unknown } else { tone },
                    row(step, view.reached.get(&step.step)),
                )
            })
            .collect()
    });
    let current = read.and_then(|read| {
        let at = read.current().map(|step| &step.step);
        match at {
            Some(at) => read.steps.iter().position(|s| &s.step == at),
            None => read.steps.len().checked_sub(1),
        }
    });
    ValidationBand {
        glyph,
        headline,
        tone,
        steps,
        heading: heading(view, doubt),
        rows,
        current,
    }
}

/// The run's state in a glyph and a few words. `now` for an active round's
/// age, when the read is verified.
fn headline(view: &RunView, now: Option<DateTime<Utc>>) -> (char, CrewTone, String) {
    let Some((_, read)) = &view.read else {
        return ('?', CrewTone::Unknown, "started · not read yet".into());
    };
    let said = |word| read.status.as_deref() == Some(word) || read.outcome.as_deref() == Some(word);
    if read.phase == Phase::Ended {
        if said("failed") {
            let step = read.steps.iter().find(|s| s.status == "failed");
            let at = step.map_or_else(String::new, |s| format!(" at {}", s.step));
            return ('✗', CrewTone::Failed, format!("failed{at}"));
        }
        if said("cancelled") {
            return ('⊘', CrewTone::Quiet, "cancelled".into());
        }
        let outcome = read.outcome.as_deref().unwrap_or("completed");
        let pr = read
            .pr
            .as_deref()
            .and_then(pr_number)
            .map_or_else(String::new, |n| format!(" · PR #{n}"));
        return ('✓', CrewTone::Settled, format!("{outcome}{pr}"));
    }
    if let Some(gate) = &read.gate {
        let (tone, what) = if gate.ask_user > 0 {
            (CrewTone::Attention, format!("{} ask-user", gate.ask_user))
        } else {
            (CrewTone::Quiet, count(gate.findings, "finding"))
        };
        return ('▲', tone, format!("{} gate · {what}", gate.step));
    }
    match read.current() {
        Some(step) => {
            let (glyph, tone) = step_glyph(step, None);
            let mut words = vec![step.step.clone()];
            if matches!(step.status.as_str(), "fixing" | "failed") {
                words.push(step.status.clone());
            }
            words.extend(round(step));
            if let (Some(now), Some(since), true) = (
                now,
                step.round_since,
                matches!(step.status.as_str(), "running" | "fixing"),
            ) {
                words.push(format!("· {}", age((now - since).num_seconds())));
            }
            (glyph, tone, words.join(" "))
        }
        None if read.status.as_deref() == Some("pending") => {
            ('·', CrewTone::Quiet, "queued".into())
        }
        None => match &read.outcome {
            Some(outcome) => ('✓', CrewTone::Settled, outcome.clone()),
            None => (
                '▸',
                CrewTone::Active,
                read.status.clone().unwrap_or_default(),
            ),
        },
    }
}

/// The panel's lines above the step table: which run, how it stands, and
/// when and how it was read.
fn heading(view: &RunView, doubt: bool) -> Vec<String> {
    let read = view.read.as_ref();
    let state = match read {
        Some((_, r)) => [r.status.as_deref(), r.outcome.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        None => "not read yet".into(),
    };
    let mut lines = vec![format!("run {} · {state}", view.run)];
    let mut times = Vec::new();
    if let Some((at, _)) = read {
        times.push(format!("read {}", clock(*at)));
    }
    if let Some((at, quality)) = view.started {
        let tag = quality.tag();
        times.push(if tag.is_empty() {
            format!("created {}", clock(at))
        } else {
            format!("created {} ({tag})", clock(at))
        });
    }
    if !times.is_empty() {
        lines.push(times.join(" · "));
    }
    match view.lost {
        Some((Phase::DaemonDown, at)) => lines.push(format!(
            "daemon down at {}: the record may be stale, unverified",
            clock(at)
        )),
        Some((Phase::Gone, at)) => lines.push(format!(
            "record gone at {}: last read shown, unverified",
            clock(at)
        )),
        _ if doubt => lines.push("not read at this moment: last read, unverified".into()),
        _ => {}
    }
    if let Some((_, read)) = read {
        if let Some(gate) = &read.gate {
            let mut what = format!(
                "gate: {} waits · {}",
                gate.step,
                count(gate.findings, "finding")
            );
            if gate.ask_user > 0 {
                what.push_str(&format!(", {} ask-user", gate.ask_user));
            }
            lines.push(what);
        }
        lines.extend(read.pr.as_ref().map(|pr| format!("PR {pr}")));
        lines.extend(read.error.as_ref().map(|error| format!("error: {error}")));
    }
    lines
}

/// One step's row: its name and status, then what the read says about it
/// and when it reached that, with how that time is known.
fn row(step: &Step, reached: Option<&(DateTime<Utc>, Option<DateTime<Utc>>)>) -> String {
    let mut status = step.status.replace('_', " ");
    if let Some(round) = round(step) {
        status = format!("{status} {round}");
    }
    let mut details = Vec::new();
    if let Some(n) = step.findings.filter(|n| *n > 0) {
        details.push(count(n, "finding"));
    }
    if let Some(ms) = step.duration_ms {
        details.push(format!("ran {}", duration(ms)));
    }
    let active = matches!(step.status.as_str(), "running" | "fixing");
    match (step.round_since, reached) {
        (Some(since), _) if active => details.push(format!("since {} (derived)", clock(since))),
        _ if step.status == "pending" => {}
        (_, Some((at, Some(before)))) => details.push(format!("{}–{}", clock(*before), clock(*at))),
        (_, Some((at, None))) => details.push(format!("by {}", clock(*at))),
        _ => {}
    }
    let mut text = format!("{:<9}{status}", step.step);
    if !details.is_empty() {
        text.push_str(" · ");
        text.push_str(&details.join(" · "));
    }
    text
}

fn step_glyph(step: &Step, gate: Option<&Gate>) -> (char, CrewTone) {
    match step.status.as_str() {
        "completed" => ('✓', CrewTone::Settled),
        "skipped" => ('-', CrewTone::Quiet),
        "pending" => ('·', CrewTone::Quiet),
        "running" => ('▸', CrewTone::Active),
        "fixing" => ('⚒', CrewTone::Active),
        "awaiting_approval" if gate.is_some_and(|g| g.ask_user > 0) => ('▲', CrewTone::Attention),
        "awaiting_approval" => ('▲', CrewTone::Quiet),
        "failed" => ('✗', CrewTone::Failed),
        _ => ('?', CrewTone::Unknown),
    }
}

/// A round as the card says it: `round 2` is `r2`; `starting` says nothing.
fn round(step: &Step) -> Option<String> {
    let round = step.round.as_deref()?.trim();
    match round.strip_prefix("round ") {
        Some(n) => Some(format!("r{n}")),
        None if round.is_empty() || round == "starting" => None,
        None => Some(round.to_owned()),
    }
}

fn count(n: u32, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0 => format!("{ms}ms"),
        1..60 => format!("{secs}s"),
        60..3600 => format!("{}m{}s", secs / 60, secs % 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

fn pr_number(pr: &str) -> Option<&str> {
    let (_, n) = pr.rsplit_once("/pull/")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n)
}

/// A footer's words for a validation event.
#[cfg(feature = "native")]
pub(super) fn narrate(v: &super::journal::Validation) -> String {
    match v.phase {
        Phase::Started => "validation started".into(),
        Phase::DaemonDown => "validation unverified: daemon down".into(),
        Phase::Gone => "validation record gone".into(),
        Phase::Parked => match &v.gate {
            Some(gate) => format!("validation gate {}", gate.step),
            None => "validation parked".into(),
        },
        Phase::Ended => format!(
            "validation {}",
            v.outcome
                .as_deref()
                .or(v.status.as_deref())
                .unwrap_or("ended")
        ),
        Phase::Seen => match v.current() {
            Some(step) => format!("validation {} {}", step.step, step.status.replace('_', " ")),
            None => format!("validation {}", v.status.as_deref().unwrap_or("read")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::journal::{AtQuality, parse_chunk};

    fn at(hms: &str) -> DateTime<Utc> {
        format!("2026-09-22T{hms}Z").parse().unwrap()
    }

    /// The crew's lifecycle and the validation runs read beside it.
    fn store() -> Lifecycle {
        let mut store = Lifecycle::default();
        for name in ["crew", "validation"] {
            let bytes = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/fleet")
                    .join(name)
                    .join("fleet.events.jsonl"),
            )
            .unwrap();
            store.insert(parse_chunk(&bytes).1);
        }
        store
    }

    fn attempt(task: &str) -> Attempt {
        let spawn_gen = if task == "impl" {
            "s1790064100.4101.1"
        } else {
            "s1790064400.4102.2"
        };
        Attempt {
            task: task.into(),
            spawn_gen: spawn_gen.into(),
        }
    }

    /// The band `task`'s card shows at `t` (`None`: the live edge).
    fn shown(store: &Lifecycle, task: &str, t: Option<&str>) -> ValidationBand {
        let t = t.map(at);
        let runs = store.validation_at(t);
        let attempt = attempt(task);
        band_for(
            store,
            &runs,
            [&attempt].into_iter(),
            t,
            t.unwrap_or_else(Utc::now),
            Utc::now(),
        )
        .unwrap()
    }

    fn strip(band: &ValidationBand) -> String {
        band.steps.iter().map(|(g, _)| g).collect()
    }

    #[test]
    fn the_band_reads_the_run_as_of_the_playhead() {
        let store = store();
        assert!(store.validation_at(Some(at("08:07:44"))).is_empty());
        // Created, not read yet: nothing verified to say.
        let band = shown(&store, "impl", Some("08:07:50"));
        assert_eq!((band.glyph, band.tone), ('?', CrewTone::Unknown));
        assert_eq!(band.headline, "started · not read yet");
        assert!(band.steps.is_empty());
        // Read: the step, its round, and how long that round has run.
        let band = shown(&store, "impl", Some("08:08:00"));
        assert_eq!((band.glyph, band.tone), ('▸', CrewTone::Active));
        assert_eq!(band.headline, "review r1 · 12s");
        assert_eq!(strip(&band), "✓✓▸······");
        // Nobody read it here: the last read, marked, without a running age.
        let band = shown(&store, "impl", Some("08:09:00"));
        assert_eq!(
            (band.glyph, band.tone, band.headline.as_str()),
            ('?', CrewTone::Unknown, "review r1")
        );
        assert!(
            band.steps
                .iter()
                .all(|(_, tone)| *tone == CrewTone::Unknown)
        );
        let band = shown(&store, "impl", Some("08:10:00"));
        assert_eq!(band.headline, "test · 40s", "starting says nothing");
        let band = shown(&store, "impl", Some("08:12:00"));
        assert_eq!(
            (band.headline.as_str(), strip(&band).as_str()),
            ("ci r1 · 1m", "✓✓✓✓✓✓✓✓▸")
        );
        // Ended: settled for good, read or not.
        for t in [Some("08:14:00"), None] {
            let band = shown(&store, "impl", t);
            assert_eq!((band.glyph, band.tone), ('✓', CrewTone::Settled));
            assert_eq!(band.headline, "passed · PR #12");
        }
    }

    #[test]
    fn a_dead_daemon_or_an_unread_gate_is_in_doubt_but_a_gate_stays_open() {
        let store = store();
        let band = shown(&store, "tests", Some("08:15:40"));
        assert_eq!(band.glyph, '?');
        assert_eq!(band.headline, "review r1 · daemon down");
        assert!(band.heading.iter().any(|l| l.starts_with("daemon down at")));
        // Read after it: verified again, parked on a question for the operator.
        let band = shown(&store, "tests", Some("08:16:00"));
        assert_eq!((band.glyph, band.tone), ('▲', CrewTone::Attention));
        assert_eq!(band.headline, "review gate · 1 ask-user");
        assert_eq!(strip(&band), "✓✓▲······");
        // Today, long after the adapter stopped: unverified, still asking.
        let band = shown(&store, "tests", None);
        assert_eq!((band.glyph, band.tone), ('?', CrewTone::Attention));
        assert!(
            band.heading
                .contains(&"not read at this moment: last read, unverified".to_string())
        );
    }

    #[test]
    fn the_table_says_how_each_time_is_known() {
        let store = store();
        let band = shown(&store, "impl", Some("08:10:00"));
        assert_eq!(band.heading[0], "run 01M342G5B2S1MP13RVNVA11DAT · running");
        let tag = AtQuality::Derived.tag();
        assert_eq!(
            band.heading[1],
            format!(
                "read {} · created {} ({tag})",
                clock(at("08:10:00")),
                clock(at("08:07:45"))
            )
        );
        let row = |step: &str| {
            band.rows
                .iter()
                .find(|(_, _, text)| text.starts_with(step))
                .unwrap()
                .2
                .clone()
        };
        // Seen before anyone read it: only a bound.
        assert_eq!(
            row("intent"),
            format!(
                "intent   completed · ran 1ms · by {}",
                clock(at("08:08:00"))
            )
        );
        // Ended between two reads, across the adapter's gap.
        assert_eq!(
            row("review"),
            format!(
                "review   completed · 1 finding · ran 1m43s · {}–{}",
                clock(at("08:08:00")),
                clock(at("08:10:00"))
            )
        );
        // An active round's start, derived from the age the read gave.
        assert_eq!(
            row("test"),
            format!(
                "test     running · since {} (derived)",
                clock(at("08:09:20"))
            )
        );
        assert_eq!(row("document"), "document pending");
        let band = shown(&store, "impl", None);
        assert!(
            band.heading
                .contains(&"PR https://github.com/example/synthetic/pull/12".to_string())
        );
    }

    #[test]
    fn rounds_ages_and_durations_read_short() {
        let step = |round: &str| Step {
            step: "test".into(),
            status: "running".into(),
            findings: None,
            duration_ms: None,
            round: Some(round.into()),
            round_since: None,
        };
        assert_eq!(round(&step("round 2")).as_deref(), Some("r2"));
        assert_eq!(
            round(&step("auto-fix 1/3")).as_deref(),
            Some("auto-fix 1/3")
        );
        assert_eq!(round(&step("starting")), None);
        assert_eq!(
            (age(42), age(600), age(3900)),
            ("42s".into(), "10m".into(), "1h5m".into())
        );
        assert_eq!(duration(38), "38ms");
        assert_eq!(duration(282_128), "4m42s");
        assert_eq!(pr_number("https://github.com/o/r/pull/12"), Some("12"));
        assert_eq!(pr_number("https://example.invalid/pr/1"), None);
    }
}
