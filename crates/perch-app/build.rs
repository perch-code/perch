//! Stamp the build with the commit it came from, for a bug report:
//! `PERCH_COMMIT` is `git rev-parse --short HEAD`, with `-dirty` when the
//! tree had changes, or `unknown` outside a checkout.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let commit = git(&["rev-parse", "--short", "HEAD"]);
    let stamp = match commit {
        Some(commit) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
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
            if let Some(path) = git(&["rev-parse", "--git-path", &reference]) {
                println!("cargo:rerun-if-changed={}", absolute(&path).display());
            }
        }
    }
    if let Some(index) = git(&["rev-parse", "--git-path", "index"]) {
        println!("cargo:rerun-if-changed={}", absolute(&index).display());
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
