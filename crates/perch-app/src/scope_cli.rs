//! `perch --scope <item-id> <scope>`
//!
//! Prints the changed list the Files tab would show for an item under a
//! scope: status, path, churn, the head blob, and a check where the reader
//! has viewed that content. `<scope>` is `branch`, `since:<sha>`,
//! `commit:<sha>`, `uncommitted`, `unstaged` or `staged`. Reads the cache
//! and the checkout; writes nothing. How the scope menu is checked without
//! a window.

use std::path::Path;
use std::time::Duration;

use perch_git::cmd::Git;
use perch_git::{changed_files, Scope};
use perch_github::{Cache, Item};

pub fn run(args: &[String]) {
    let (Some(id), Some(scope)) = (args.first(), args.get(1)) else {
        eprintln!("usage: perch --scope <item-id> <branch|since:<sha>|commit:<sha>|uncommitted|unstaged|staged>");
        std::process::exit(2);
    };
    let Some(scope) = Scope::parse(scope) else {
        eprintln!("not a scope: {scope}");
        std::process::exit(2);
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    if let Err(e) = runtime.block_on(print(id, &scope)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn print(id: &str, scope: &Scope) -> Result<(), String> {
    let path = perch_github::config::db_path().ok_or("no data dir")?;
    if !path.exists() {
        return Err(format!("no cache at {}; run the app once", path.display()));
    }
    let (cache, reset) = Cache::open_checked(&path, None).map_err(|e| e.to_string())?;
    if reset {
        eprintln!("note: the database was older than this build and has been recreated");
    }
    let item = cache.load_item(id).map_err(|e| e.to_string())?.ok_or_else(|| format!("no item {id}"))?;
    let checkout = item.checkout.as_ref().ok_or("that item has no checkout")?;
    if !checkout.path.join(".git").exists() {
        return Err(format!("{} is not on disk", checkout.path.display()));
    }
    let viewed = cache.load_viewed(id).map_err(|e| e.to_string())?;

    let git = Git::new(None);
    let head = git
        .run(Some(&checkout.path), ["rev-parse", "HEAD"], Duration::from_secs(30))
        .await
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let merge_base = merge_base(&git, &checkout.path, &item, &head).await?;
    eprintln!("scope: {scope}  base: {}  head: {}", &merge_base[..7.min(merge_base.len())], &head[..7.min(head.len())]);

    let files = changed_files(&git, &checkout.path, scope, &merge_base, &head)
        .await
        .map_err(|e| e.to_string())?;
    let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
    for f in &files {
        let mark = match &f.head_blob {
            Some(blob) if viewed.contains(&(f.path.clone(), blob.clone())) => "\u{2713}",
            _ => " ",
        };
        let blob = f.head_blob.as_deref().map(|b| &b[..7.min(b.len())]).unwrap_or("-");
        let churn = if f.binary { "binary".to_string() } else { format!("+{} \u{2212}{}", f.additions, f.deletions) };
        println!("{mark} {} {:<40} {:<12} {blob}", f.status.letter(), f.path, churn);
    }
    println!("{} files  +{adds} \u{2212}{dels}", files.len());
    Ok(())
}

/// The base the app diffs against, found the way the app finds it: the
/// pull request's fetched base, else the home ref, else origin's branch,
/// else the branch name as the checkout knows it.
async fn merge_base(git: &Git, worktree: &Path, item: &Item, head: &str) -> Result<String, String> {
    let checkout = item.checkout.as_ref().ok_or("no checkout")?;
    let mut candidates = Vec::new();
    if let Some(pr) = item.pull_request() {
        candidates.push(format!("refs/perch/base/{}", pr.number));
    }
    candidates.push(perch_git::home_ref(&checkout.base_ref));
    candidates.push(format!("origin/{}", checkout.base_ref));
    candidates.push(checkout.base_ref.clone());
    for base in candidates {
        if let Ok(out) = git.run(Some(worktree), ["merge-base", &base, head], Duration::from_secs(30)).await {
            let sha = out.trim().to_string();
            if !sha.is_empty() {
                return Ok(sha);
            }
        }
    }
    Err(format!("no base for {} reaches {head}", checkout.base_ref))
}
