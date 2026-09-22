//! Recorded usage and account quota snapshots. No network or credential access.
//!
//! Input includes cached reads/writes; reasoning is already part of output.
//! Prices are standard short-context API equivalents (2026-09-22), NOT a
//! subscription bill. See docs/FLEET-USAGE.md for sources and estimate limitations.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
}
impl Tokens {
    pub fn total(&self) -> u64 {
        self.input
            .unwrap_or(0)
            .saturating_add(self.output.unwrap_or(0))
    }
    pub fn complete(&self) -> bool {
        self.input.is_some() && self.output.is_some()
    }
    pub fn merge(&mut self, other: &Self) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cached = self.cached.max(other.cached);
        self.cache_write = self.cache_write.max(other.cache_write);
        self.cache_write_1h = self.cache_write_1h.max(other.cache_write_1h);
    }
    pub fn delta(&self, previous: &Self) -> Self {
        Self {
            input: self
                .input
                .map(|v| v.saturating_sub(previous.input.unwrap_or(0))),
            output: self
                .output
                .map(|v| v.saturating_sub(previous.output.unwrap_or(0))),
            cached: self.cached.saturating_sub(previous.cached),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            cache_write_1h: self.cache_write_1h.saturating_sub(previous.cache_write_1h),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    /// Stable request ID, or provider-generated delta identity.
    pub key: String,
    pub model: Option<String>,
    pub tokens: Tokens,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub recorded: bool,
    pub incomplete: bool,
    pub usd: Option<f64>,
}
impl Summary {
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }
    pub fn cost_label(&self) -> String {
        self.usd
            .map_or_else(|| "API est. —".into(), |v| format!("API est. ${v:.3}"))
    }
}

/// Request-keyed max merging makes repeated Claude progress records idempotent
/// and order-independent, while permitting a final higher output count.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Book {
    entries: imbl::OrdMap<String, Usage>,
    pub summary: Summary,
}
impl Book {
    pub fn apply(&mut self, usage: &Usage) {
        if let Some(old) = self.entries.get_mut(&usage.key) {
            old.tokens.merge(&usage.tokens);
            // Stable tie breaking, including enrichment of initially unknown model.
            old.model = old.model.clone().max(usage.model.clone());
        } else {
            self.entries.insert(usage.key.clone(), usage.clone());
        }
        let mut s = Summary {
            recorded: true,
            usd: Some(0.0),
            ..Summary::default()
        };
        for u in self.entries.values() {
            s.input = s.input.saturating_add(u.tokens.input.unwrap_or(0));
            s.output = s.output.saturating_add(u.tokens.output.unwrap_or(0));
            s.cached = s.cached.saturating_add(u.tokens.cached);
            s.cache_write = s.cache_write.saturating_add(u.tokens.cache_write);
            s.incomplete |= !u.tokens.complete();
            s.usd = s.usd.zip(estimate(u)).map(|(a, b)| a + b);
        }
        self.summary = s;
    }
}

fn estimate(u: &Usage) -> Option<f64> {
    let t = &u.tokens;
    let input = t.input?;
    let output = t.output?;
    // Exact supported model IDs: an unknown/new model never silently inherits
    // another model's rate. Date aliases are normalized by the provider if known.
    let (base, out) = match u.model.as_deref()? {
        "gpt-6-astra" => (10.0, 50.0),
        "gpt-5.6-sol" => (4.0, 20.0),
        "gpt-5.6-terra" => (2.0, 12.0),
        "gpt-5.6-luna" => (0.2, 1.2),
        "claude-opus-5"
        | "claude-opus-4-8"
        | "claude-opus-4-7"
        | "claude-opus-4-6"
        | "claude-opus-4-5"
        | "claude-opus-4-5-20251101" => (5.0, 25.0),
        "claude-sonnet-5" => (2.0, 10.0),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" => (3.0, 15.0),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => (1.0, 5.0),
        _ => return None,
    };
    let cached = t.cached.min(input);
    let write = t.cache_write.min(input - cached);
    let write_1h = t.cache_write_1h.min(write);
    Some(
        ((input - cached - write) as f64 * base
            + cached as f64 * base * 0.1
            + (write - write_1h) as f64 * base * 1.25
            + write_1h as f64 * base * 2.0
            + output as f64 * out)
            / 1_000_000.0,
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub minutes: u32,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    pub bucket: String,
    pub observed_at: DateTime<Utc>,
    pub windows: Vec<Window>,
}
impl Quota {
    pub fn valid(&self) -> bool {
        self.windows
            .iter()
            .all(|w| w.used_percent.is_finite() && (0.0..=100.0).contains(&w.used_percent))
    }
    pub fn label(&self, now: DateTime<Utc>) -> String {
        let window = |minutes, name| match self.windows.iter().find(|w| w.minutes == minutes) {
            Some(w) if w.resets_at.is_some_and(|t| t <= now.timestamp()) => {
                format!("{name} refresh pending")
            }
            Some(w) => {
                let reset = w
                    .resets_at
                    .map(|t| {
                        let m = ((t - now.timestamp()).max(0) + 59) / 60;
                        format!(" ↻{}h{:02}m", m / 60, m % 60)
                    })
                    .unwrap_or_default();
                format!("{name} {:.0}% left{reset}", 100.0 - w.used_percent)
            }
            None => format!("{name} —"),
        };
        let age = (now - self.observed_at).num_seconds().max(0);
        format!(
            "{} · {} · {}{}",
            window(300, "5h"),
            window(10080, "week"),
            if age > 300 { "stale " } else { "" },
            format_args!("{}m ago", age / 60)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn usage(key: &str, output: u64) -> Usage {
        Usage {
            key: key.into(),
            model: Some("claude-opus-5".into()),
            tokens: Tokens {
                input: Some(1000),
                output: Some(output),
                cached: 600,
                cache_write: 100,
                cache_write_1h: 20,
            },
        }
    }
    #[test]
    fn repeated_progress_is_idempotent_order_independent_and_cache_is_not_double_counted() {
        let mut a = Book::default();
        let mut b = Book::default();
        for n in [10, 20, 10, 20] {
            a.apply(&usage("a", n));
        }
        for n in [20, 10] {
            b.apply(&usage("a", n));
        }
        assert_eq!(a.summary, b.summary);
        assert_eq!(a.summary.total(), 1020);
        assert!((a.summary.usd.unwrap() - 0.003).abs() < 1e-10);
        a.apply(&usage("b", 20));
        assert_eq!(a.summary.total(), 2040);
    }
    #[test]
    fn unknown_model_and_partial_tokens_are_not_free() {
        let mut a = Book::default();
        let mut u = usage("a", 0);
        u.model = Some("new-model".into());
        a.apply(&u);
        assert_eq!(a.summary.usd, None);
        u.tokens.input = None;
        u.key = "b".into();
        a.apply(&u);
        assert!(a.summary.incomplete);
    }
    #[test]
    fn quotas_use_window_duration_and_do_not_invent_a_reset() {
        let now = Utc::now();
        let q = Quota {
            bucket: "codex".into(),
            observed_at: now,
            windows: vec![Window {
                minutes: 10080,
                used_percent: 42.0,
                resets_at: Some(now.timestamp() - 1),
            }],
        };
        assert!(q.label(now).contains("5h — · week refresh pending"));
    }
}
