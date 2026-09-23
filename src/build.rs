//! Which build this is: the version, the commit it was built from and when,
//! as the build script stamped them. A running viewer outlives the file it was
//! started from, so this is the only thing that can still name it.

use chrono::{DateTime, Utc};
use std::fmt;

/// The commit a build was made from, or why that is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commit {
    /// A short hash; `dirty` when the working tree differed from it.
    Known { hash: &'static str, dirty: bool },
    /// Built outside a git checkout, or without git: the reason, never a guess.
    Unknown(&'static str),
}

impl Commit {
    /// Read the build script's value: a hash with an optional `-dirty`, or a
    /// reason. Anything that is not a hash is a reason, an empty one included.
    pub fn parse(stamp: &'static str) -> Self {
        let (hash, dirty) = match stamp.strip_suffix("-dirty") {
            Some(hash) => (hash, true),
            None => (stamp, false),
        };
        let hex = hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit());
        match (hex, stamp.trim()) {
            (true, _) => Self::Known { hash, dirty },
            (false, "") => Self::Unknown("no commit was recorded"),
            (false, reason) => Self::Unknown(reason),
        }
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known { hash, dirty } => {
                write!(f, "{hash}{}", if *dirty { "-dirty" } else { "" })
            }
            Self::Unknown(reason) => write!(f, "commit unknown ({reason})"),
        }
    }
}

/// A build's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    pub version: &'static str,
    pub commit: Commit,
    /// When the build script last ran; `None` if it could not tell.
    pub built: Option<DateTime<Utc>>,
}

impl Build {
    /// This binary's own.
    pub fn current() -> Self {
        Self::parse(
            env!("CARGO_PKG_VERSION"),
            env!("ZOE_BUILD_COMMIT"),
            env!("ZOE_BUILD_EPOCH"),
        )
    }

    pub fn parse(version: &'static str, commit: &'static str, epoch: &str) -> Self {
        let built = epoch
            .parse::<i64>()
            .ok()
            .and_then(|s| DateTime::from_timestamp(s, 0));
        Self {
            version,
            commit: Commit::parse(commit),
            built,
        }
    }

    /// For machine-read reports: every field, unknowns as `null`.
    pub fn json(&self) -> serde_json::Value {
        let (commit, dirty, unknown) = match self.commit {
            Commit::Known { hash, dirty } => (Some(hash), Some(dirty), None),
            Commit::Unknown(reason) => (None, None, Some(reason)),
        };
        serde_json::json!({
            "version": self.version,
            "commit": commit,
            "dirty": dirty,
            "commit_unknown": unknown,
            "built": self.built.map(|t| t.to_rfc3339()),
        })
    }
}

impl fmt::Display for Build {
    /// `0.2.0 (1a2b3c4d5e6f-dirty, built 2026-09-23 10:12:03 UTC)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}, ", self.version, self.commit)?;
        match self.built {
            Some(at) => write!(f, "built {})", at.format("%Y-%m-%d %H:%M:%S UTC")),
            None => write!(f, "build time unknown)"),
        }
    }
}

/// The build script itself, for the tests to run where it cannot stamp.
#[cfg(test)]
#[allow(dead_code)]
#[path = "../build.rs"]
mod script;

#[cfg(test)]
mod tests {
    use super::script;
    use super::*;

    #[test]
    fn a_checkout_build_names_its_commit_and_time() {
        let build = Build::current();
        let Commit::Known { hash, .. } = build.commit else {
            panic!("the test build comes from a checkout: {build}");
        };
        assert_eq!(hash.len(), 12, "{hash}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
        let built = build.built.expect("the build script stamps a time");
        assert!(built <= Utc::now() && built.timestamp() > 1_700_000_000);
        let shown = build.to_string();
        assert!(shown.starts_with(&format!("{} ({hash}", env!("CARGO_PKG_VERSION"))));
        assert!(shown.ends_with(" UTC)"), "{shown}");
    }

    #[test]
    fn a_dirty_tree_is_marked() {
        let commit = Commit::parse("1a2b3c4d5e6f-dirty");
        assert_eq!(
            commit,
            Commit::Known {
                hash: "1a2b3c4d5e6f",
                dirty: true
            }
        );
        assert_eq!(commit.to_string(), "1a2b3c4d5e6f-dirty");
    }

    #[test]
    fn an_unknown_commit_says_why_instead_of_guessing() {
        let build = Build::parse("0.2.0", "not built from a git checkout", "");
        assert_eq!(
            build.to_string(),
            "0.2.0 (commit unknown (not built from a git checkout), build time unknown)"
        );
        let json = build.json();
        assert_eq!(json["commit"], serde_json::Value::Null);
        assert_eq!(json["commit_unknown"], "not built from a git checkout");
        assert_eq!(Commit::parse(""), Commit::Unknown("no commit was recorded"));
        // Not a hash, however hash-like: a word never passes for a commit.
        assert!(matches!(Commit::parse("deadbeefzz"), Commit::Unknown(_)));
    }

    #[test]
    fn the_build_script_degrades_outside_a_checkout_or_without_git() {
        let dir = std::env::temp_dir().join(format!("zoe-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let outside = script::probe(&dir, "git");
        let no_git = script::probe(&dir, "/nonexistent/zoe-no-git");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(outside.unwrap_err(), "not built from a git checkout");
        assert_eq!(no_git.unwrap_err(), "git unavailable");
        // And from this checkout, a well-formed hash.
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let (stamp, watched) = script::probe(here, "git").unwrap();
        let stamp: &'static str = Box::leak(stamp.into_boxed_str());
        assert!(
            matches!(Commit::parse(stamp), Commit::Known { .. }),
            "{stamp}"
        );
        assert!(watched.iter().all(|p| p.exists()));
    }
}
