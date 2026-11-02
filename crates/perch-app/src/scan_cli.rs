//! `perch --scan [root...]`
//!
//! Prints what the root scan finds: for each `[[repo]]` root in config,
//! or for the directories given instead, the repository its GitHub remote
//! names and which remote that was, its base branch, and every worktree
//! with its path, branch, head and the lines it has added and removed
//! against the base. Reads only; nothing is written to the cache or to
//! disk. How the scan is checked without a window.

use std::path::Path;

use perch_git::cmd::Git;
use perch_git::{branch_churn, scan_root, WorktreeEntry};

pub fn run(args: &[String]) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(scan(args));
}

async fn scan(args: &[String]) {
    let git = Git::new(None);

    if !args.is_empty() {
        // The directories given, with the hosts config names: github.com,
        // and the enterprise host when the file has one. A config that
        // does not load leaves github.com alone, which is what the app
        // would count too.
        let hosts = perch_github::Config::load().github_hosts();
        let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
        for root in args {
            print_root(&git, Path::new(root), None, &hosts).await;
        }
        return;
    }

    let config = crate::repos::load_resolved_async().await;
    for warning in &config.warnings {
        eprintln!("warning: {warning}");
    }
    if config.repo.is_empty() {
        eprintln!("no [[repo]] entries in config.toml");
        return;
    }
    let hosts = config.github_hosts();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();

    for repo in &config.repo {
        let root = repo.root_path();
        println!(
            "repo {} ({})",
            repo.display_name(),
            repo.identity().unwrap_or("unresolved")
        );
        match root {
            Some(root) => print_root(&git, &root, repo.base.as_deref(), &hosts).await,
            None => println!("  no root"),
        }
    }
}

/// One root, its worktrees indented under it. A root that cannot be
/// scanned prints why and the next one is tried, as the app's scan will.
async fn print_root(git: &Git, root: &Path, base: Option<&str>, hosts: &[&str]) {
    println!("  root {}", root.display());
    let scan = match scan_root(git, root, base, hosts).await {
        Ok(scan) => scan,
        Err(e) => {
            println!("    error: {e}");
            return;
        }
    };

    match (&scan.github, &scan.remote) {
        (Some(github), Some(remote)) => {
            println!(
                "    github {github} (remote {remote}), base {}, diffed against {}",
                scan.base, scan.base_ref
            )
        }
        _ => println!("    no GitHub remote (local), base {}", scan.base),
    }
    for entry in &scan.entries {
        println!(
            "    {}",
            describe(git, entry, &scan.base, &scan.base_ref).await
        );
    }
}

async fn describe(git: &Git, entry: &WorktreeEntry, base: &str, base_ref: &str) -> String {
    let head = entry.head_sha.get(..7).unwrap_or(&entry.head_sha);
    let main = if entry.is_main {
        " [main worktree]"
    } else {
        ""
    };

    match entry.branch.as_deref() {
        None => format!("{}  (detached)  {head}{main}", entry.path.display()),
        Some(branch) if entry.prunable || !entry.path.join(".git").exists() => {
            format!(
                "{}  {branch}  {head}  (gone, not an item){main}",
                entry.path.display()
            )
        }
        Some(branch) if branch == base => {
            format!(
                "{}  {branch}  {head}  (base branch, not an item){main}",
                entry.path.display()
            )
        }
        Some(branch) => {
            let churn = match branch_churn(git, &entry.path, base_ref).await {
                Ok((added, removed)) => format!("+{added} \u{2212}{removed}"),
                Err(e) => format!("churn unavailable: {e}"),
            };
            format!("{}  {branch}  {head}  {churn}{main}", entry.path.display())
        }
    }
}
