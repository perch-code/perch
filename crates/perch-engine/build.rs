//! Stamp the build with the commit it came from, for a bug report:
//! `PERCH_COMMIT` is `git rev-parse --short HEAD`, or `unknown` outside a
//! checkout. A release build adds `-dirty` when the tree had changes.
//!
//! A debug build leaves `-dirty` out on purpose. Knowing it means watching
//! the git index, and `git add`, a commit, or an editor that refreshes git
//! status touches the index. Every touch reran this script and recompiled
//! all of perch-app.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let commit = git(&["rev-parse", "--short", "HEAD"]);
    let stamp = match commit {
        Some(commit) => {
            let dirty = release
                && git(&["status", "--porcelain", "--untracked-files=no"])
                    .is_some_and(|out| !out.is_empty());
            if dirty {
                format!("{commit}-dirty")
            } else {
                commit
            }
        }
        None => "unknown".to_string(),
    };
    println!("cargo:rustc-env=PERCH_COMMIT={stamp}");

    // Rebuild the stamp when HEAD moves or the branch it names does. In a
    // worktree `.git` is a file, so ask git where HEAD lives.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        let head = absolute(&head);
        println!("cargo:rerun-if-changed={}", head.display());
        if let Some(reference) = std::fs::read_to_string(&head)
            .ok()
            .and_then(|text| text.strip_prefix("ref: ").map(|r| r.trim().to_string()))
        {
            // A packed branch has no file of its own until its next update
            // writes one, and cargo reruns a script on every build while a
            // watched file is missing. The branch's reflog gets a line on
            // every update, packed or not, so watch that when it is there.
            let existing = |path: &str| {
                git(&["rev-parse", "--git-path", path])
                    .map(|path| absolute(&path))
                    .filter(|path| path.exists())
            };
            let watched = existing(&reference)
                .or_else(|| existing(&format!("logs/{reference}")))
                .or_else(|| existing("packed-refs"));
            if let Some(path) = watched {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    // Only a release build says `-dirty`, so only it follows the index.
    if release {
        if let Some(index) = git(&["rev-parse", "--git-path", "index"]) {
            println!("cargo:rerun-if-changed={}", absolute(&index).display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

fn absolute(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}
