//! Recorded usage and account quota snapshots. No network or credential access.
//!
//! Input includes cached reads/writes; reasoning is already part of output.
//! Prices are standard short-context API equivalents (2026-09-22; Opus 5.5
//! 2026-09-23), NOT a subscription bill. An unlisted model in a listed tier is
//! priced approximately and labeled so. See docs/FLEET-USAGE.md for sources and
//! estimate limitations.
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
    /// Part of `usd` was priced at a listed sibling's rate, not the model's own.
    pub approximate: bool,
}
impl Summary {
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }
    pub fn cost_label(&self) -> String {
        match self.usd {
            None => "API est. —".into(),
            Some(v) if self.approximate => format!("API est. ~${v:.3} (unlisted model)"),
            Some(v) => format!("API est. ${v:.3}"),
        }
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
            let est = estimate(u);
            s.approximate |= est.is_some_and(|(_, exact)| !exact);
            s.usd = s.usd.zip(est).map(|(a, (b, _))| a + b);
        }
        self.summary = s;
    }
}

/// Per-MTok USD: base input, output, and the cache-read multiplier of base.
type Rate = (f64, f64, f64);

/// Exact supported model IDs. Date aliases are normalized by the provider if known.
/// Claude rates checked 2026-09-23 against
/// https://platform.claude.com/docs/en/about-claude/pricing: Opus 5.5 is
/// $4 in / $20 out with cache hits at 0.05x base; every other listed Claude
/// model uses the standard 0.1x.
const RATES: &[(&str, Rate)] = &[
    ("gpt-6-astra", (10.0, 50.0, 0.1)),
    ("gpt-5.6-sol", (4.0, 20.0, 0.1)),
    ("gpt-5.6-terra", (2.0, 12.0, 0.1)),
    ("gpt-5.6-luna", (0.2, 1.2, 0.1)),
    ("claude-opus-5-5", (4.0, 20.0, 0.05)),
    ("claude-opus-5", (5.0, 25.0, 0.1)),
    ("claude-opus-4-8", (5.0, 25.0, 0.1)),
    ("claude-opus-4-7", (5.0, 25.0, 0.1)),
    ("claude-opus-4-6", (5.0, 25.0, 0.1)),
    ("claude-opus-4-5", (5.0, 25.0, 0.1)),
    ("claude-opus-4-5-20251101", (5.0, 25.0, 0.1)),
    ("claude-sonnet-5", (2.0, 10.0, 0.1)),
    ("claude-sonnet-4-6", (3.0, 15.0, 0.1)),
    ("claude-sonnet-4-5", (3.0, 15.0, 0.1)),
    ("claude-sonnet-4-5-20250929", (3.0, 15.0, 0.1)),
    ("claude-haiku-4-5", (1.0, 5.0, 0.1)),
    ("claude-haiku-4-5-20251001", (1.0, 5.0, 0.1)),
];

fn listed(model: &str) -> Option<Rate> {
    RATES
        .iter()
        .find(|(id, _)| *id == model)
        .map(|&(_, rate)| rate)
}

/// A model's family, tier and numeric version by ID shape:
/// `claude-<tier>-<n>-<n>…[-<yyyymmdd>]` or `gpt-<n>.<n>-<tier>`; a snapshot
/// date is not part of the version. Anything else has no family.
fn shape(model: &str) -> Option<(&'static str, &str, Vec<u64>)> {
    let nums = |s: &str, sep| {
        s.split(sep)
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
    };
    if let Some(rest) = model.strip_prefix("claude-") {
        let (tier, version) = rest.split_once('-')?;
        let version = version
            .rsplit_once('-')
            .filter(|(_, d)| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
            .map_or(version, |(v, _)| v);
        return Some(("claude", tier, nums(version, '-').ok()?));
    }
    let (version, tier) = model.strip_prefix("gpt-")?.split_once('-')?;
    Some(("gpt", tier, nums(version, '.').ok()?))
}

/// The listed model an unlisted one is priced like: same family and tier, the
/// newest version not above it. No family or no older listed version, no sibling.
fn sibling(model: &str) -> Option<&'static str> {
    let (family, tier, version) = shape(model)?;
    let mut same: Vec<(Vec<u64>, &'static str)> = RATES
        .iter()
        .filter_map(|&(id, _)| {
            let (f, t, v) = shape(id)?;
            (f == family && t == tier).then_some((v, id))
        })
        .collect();
    same.sort();
    same.dedup_by(|a, b| a.0 == b.0);
    same.iter()
        .rev()
        .find(|(v, _)| *v <= version)
        .map(|&(_, id)| id)
}

/// USD and whether the rate is the model's own. An unlisted model never
/// silently inherits another model's rate: it borrows a same-tier sibling's
/// rate only as an estimate the label marks approximate, and a model with no
/// listed tier, or older than every listed model of its tier, gets no estimate.
fn estimate(u: &Usage) -> Option<(f64, bool)> {
    let t = &u.tokens;
    let input = t.input?;
    let output = t.output?;
    let model = u.model.as_deref()?;
    let (exact, (base, out, read)) = match listed(model) {
        Some(rate) => (true, rate),
        None => (false, listed(sibling(model)?)?),
    };
    let cached = t.cached.min(input);
    let write = t.cache_write.min(input - cached);
    let write_1h = t.cache_write_1h.min(write);
    Some((
        ((input - cached - write) as f64 * base
            + cached as f64 * base * read
            + (write - write_1h) as f64 * base * 1.25
            + write_1h as f64 * base * 2.0
            + output as f64 * out)
            / 1_000_000.0,
        exact,
    ))
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
    fn priced(model: &str) -> Summary {
        let mut b = Book::default();
        let mut u = usage("a", 1_000_000);
        u.model = Some(model.into());
        b.apply(&u);
        b.summary
    }
    #[test]
    fn listed_models_keep_their_exact_rate_and_label() {
        for (model, usd) in [
            ("claude-opus-5", 25.0025),
            ("claude-opus-4-5-20251101", 25.0025),
            ("claude-sonnet-4-6", 15.0015),
            ("gpt-5.6-luna", 1.2001),
        ] {
            let s = priced(model);
            assert!((s.usd.unwrap() - usd).abs() < 1e-9, "{model}: {s:?}");
            assert!(!s.approximate, "{model}");
            assert_eq!(s.cost_label(), format!("API est. ${usd:.3}"));
        }
    }
    #[test]
    fn opus_5_5_is_listed_with_its_own_cache_read_rate() {
        // 300 uncached × $4 + 600 read × $0.20 + 80 × $5 + 20 × $8, + 1M out × $20.
        let s = priced("claude-opus-5-5");
        assert!((s.usd.unwrap() - 20.00188).abs() < 1e-9, "{s:?}");
        assert!(!s.approximate);
    }
    #[test]
    fn unlisted_model_in_a_listed_tier_is_approximate_and_says_so() {
        for (model, like) in [
            ("claude-opus-6", "claude-opus-5-5"),
            ("claude-opus-5-5-20260801", "claude-opus-5-5"),
            ("claude-opus-5-20260301", "claude-opus-5"),
            ("claude-haiku-5", "claude-haiku-4-5"),
            ("gpt-6-luna", "gpt-5.6-luna"),
        ] {
            assert_eq!(sibling(model), Some(like), "{model}");
            let s = priced(model);
            assert!(s.approximate, "{model}");
            assert_eq!(s.usd, priced(like).usd, "{model}");
            assert!(s.cost_label().starts_with("API est. ~$"), "{model}");
            assert!(s.cost_label().ends_with(" (unlisted model)"), "{model}");
        }
        // One approximate request marks the whole total approximate.
        let mut b = Book::default();
        b.apply(&usage("a", 10));
        let mut u = usage("b", 10);
        u.model = Some("claude-opus-6".into());
        b.apply(&u);
        assert!(b.summary.approximate);
    }
    #[test]
    fn unlisted_model_outside_or_older_than_every_listed_tier_has_no_estimate() {
        for model in [
            "new-model",
            "claude-fable-5-1",
            "claude-3-5-sonnet-20241022",
            "gpt-5.6-cyber",
            "gpt-5.6-sol-codex",
            "gpt-oss-120b",
            "claude-opus-4-1-20250805",
            "claude-opus-4-20250514",
            "claude-sonnet-4",
        ] {
            assert_eq!(sibling(model), None, "{model}");
            let s = priced(model);
            assert_eq!(s.usd, None, "{model}");
            assert_eq!(s.cost_label(), "API est. —");
        }
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
