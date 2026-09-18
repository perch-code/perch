//! `perch --scan [root...]`
//!
//! Prints what the root scan finds: for each `[[project]]` root in config,
//! or for the directories given instead, the repository its `origin` names,
//! its default branch, and every worktree with its path, branch, head and
//! the lines it has added and removed against the default branch. Reads
//! only; nothing is written to the cache or to disk. How the scan is
//! checked without a window.

use std::path::{Path, PathBuf};

use perch_git::cmd::Git;
use perch_git::{branch_churn, scan_root, WorktreeEntry};
use perch_github::Config;

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
        for root in args {
            print_root(&git, Path::new(root)).await;
        }
        return;
    }

    let config = Config::load();
    for warning in &config.warnings {
        eprintln!("warning: {warning}");
    }
    if config.project.is_empty() {
        eprintln!("no [[project]] entries in config.toml");
        return;
    }

    for project in &config.project {
        let roots: Vec<PathBuf> = project.root_paths();
        println!("project {}", project.name);
        if roots.is_empty() {
            println!("  no roots");
            continue;
        }
        for root in &roots {
            print_root(&git, root).await;
        }
    }
}

/// One root, its worktrees indented under it. A root that cannot be
/// scanned prints why and the next one is tried, as the app's scan will.
async fn print_root(git: &Git, root: &Path) {
    println!("  root {}", root.display());
    let scan = match scan_root(git, root).await {
        Ok(scan) => scan,
        Err(e) => {
            println!("    error: {e}");
            return;
        }
    };

    println!(
        "    repo {}, default branch {}",
        scan.repo_full_name
            .as_deref()
            .unwrap_or("(origin is not owner/repo)"),
        scan.default_branch
    );
    for entry in &scan.entries {
        println!("    {}", describe(git, entry, &scan.default_branch).await);
    }
}

async fn describe(git: &Git, entry: &WorktreeEntry, default_branch: &str) -> String {
    let head = entry.head_sha.get(..7).unwrap_or(&entry.head_sha);
    let main = if entry.is_main {
        " [main worktree]"
    } else {
        ""
    };

    match entry.branch.as_deref() {
        None => format!("{}  (detached)  {head}{main}", entry.path.display()),
        Some(branch) if branch == default_branch => {
            format!(
                "{}  {branch}  {head}  (default branch, not an item){main}",
                entry.path.display()
            )
        }
        Some(branch) => {
            let churn = match branch_churn(git, &entry.path, default_branch).await {
                Ok((added, removed)) => format!("+{added} \u{2212}{removed}"),
                Err(e) => format!("churn unavailable: {e}"),
            };
            format!("{}  {branch}  {head}  {churn}{main}", entry.path.display())
        }
    }
}
