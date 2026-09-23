//! Stamp the build so a running `zoe` can name itself: the commit it was built
//! from and when. Read at runtime through `zoetrope::build`.
//!
//! `ZOE_BUILD_COMMIT` is a short hash, `-dirty` appended when the working tree
//! differed from it, or else the reason there is none: a build from the
//! crates.io tarball, a copy vendored into someone else's repository, or a
//! machine without git must say so, never borrow a hash that is not its own.
//! `ZOE_BUILD_EPOCH` is when this script ran, in Unix seconds
//! (`SOURCE_DATE_EPOCH` wins, for reproducible builds).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let commit = match probe(&dir, "git") {
        Ok((commit, watched)) => {
            // Rerun when HEAD moves or the index does (a commit, a checkout,
            // a `git add`), besides the sources: the hash follows the tree.
            for path in watched {
                println!("cargo:rerun-if-changed={}", path.display());
            }
            commit
        }
        Err(reason) => reason,
    };
    for path in ["build.rs", "Cargo.toml", "Cargo.lock", "src"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        });
    println!("cargo:rustc-env=ZOE_BUILD_COMMIT={commit}");
    println!(
        "cargo:rustc-env=ZOE_BUILD_EPOCH={}",
        epoch.map(|e| e.to_string()).unwrap_or_default()
    );
}

/// The commit `dir` was checked out at, and the git files that move with it;
/// or why there is none. `dir` counts only as the top of its own checkout.
pub fn probe(dir: &Path, git: &str) -> Result<(String, Vec<PathBuf>), String> {
    let run = |args: &[&str]| -> Result<Option<String>, String> {
        let out = Command::new(git)
            .args(args)
            .current_dir(dir)
            // Keep `git status` from refreshing the index it is watched by.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|_| "git unavailable".to_string())?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
    };
    let unchecked = || "not built from a git checkout".to_string();
    let top = run(&["rev-parse", "--show-toplevel"])?.ok_or_else(unchecked)?;
    let same = |a: &Path, b: &Path| a.canonicalize().ok() == b.canonicalize().ok();
    if !same(Path::new(&top), dir) {
        return Err(unchecked());
    }
    let hash = run(&["rev-parse", "--short=12", "HEAD"])?
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "no commit checked out".to_string())?;
    let status = run(&["status", "--porcelain"])?.ok_or_else(|| "git status failed".to_string())?;
    let dirty = if status.is_empty() { "" } else { "-dirty" };
    let mut watched = Vec::new();
    let mut git_path = |name: &str| -> Result<(), String> {
        // A path that does not exist would rerun this script on every build.
        let path = run(&["rev-parse", "--git-path", name])?.map(|p| dir.join(p));
        watched.extend(path.filter(|p| p.exists()));
        Ok(())
    };
    git_path("HEAD")?;
    git_path("index")?;
    git_path("packed-refs")?;
    if let Some(branch) = run(&["symbolic-ref", "-q", "HEAD"])? {
        git_path(&branch)?;
    }
    Ok((format!("{hash}{dirty}"), watched))
}
