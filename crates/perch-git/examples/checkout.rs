//! Prepare a pull request checkout and print its changed files.
//!
//!     cargo run -p perch-git --example checkout -- slatedb/slatedb#2091

use std::time::Duration;

use perch_git::{diff, Preparer, RepoId, Step, Store};
use perch_github::{resolve_token, Config, GitHub};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let target = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: checkout owner/repo#number"))?;
    let (full_name, number) = target
        .split_once('#')
        .ok_or_else(|| anyhow::anyhow!("expected owner/repo#number"))?;
    let number: u64 = number.parse()?;
    let id = RepoId::parse(full_name).ok_or_else(|| anyhow::anyhow!("bad repo name"))?;

    let config = Config::load();
    let token = resolve_token(&config).ok_or_else(|| anyhow::anyhow!("no GitHub token"))?;
    let gh = GitHub::new(&token.value)?;

    eprintln!("resolving {full_name}#{number} ...");
    let pr = gh.fetch_pull_request(full_name, number).await?;
    eprintln!(
        "  {} by {}  {} <- {}  (+{} -{}, {} files)",
        pr.title,
        pr.author_login,
        pr.base_ref,
        pr.head_ref,
        pr.additions,
        pr.deletions,
        pr.changed_files
    );

    let root =
        perch_github::config::data_dir().ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    let preparer = Preparer::new(Store::new(root), Some(token.value.clone()));

    let started = std::time::Instant::now();
    let branch = format!("perch/pr-{number}");
    let checkout = preparer
        .prepare(
            &id,
            number,
            &pr.head_sha,
            &pr.base_ref,
            &branch,
            |step| match step {
                Step::Cloning { phase, percent } => match percent {
                    Some(p) => eprint!("\r  cloning: {phase} {p}%   "),
                    None => eprint!("\r  cloning: {phase}   "),
                },
                Step::Fetching => eprintln!("\n  fetching pull request ref"),
                Step::Downloading { phase, percent } => match percent {
                    Some(p) => eprint!("\r  downloading files: {phase} {p}%   "),
                    None => eprint!("\r  downloading files: {phase}   "),
                },
                Step::CreatingWorktree => eprintln!("  creating worktree"),
                Step::ComputingDiff => eprintln!("  diffing"),
            },
        )
        .await?;

    eprintln!(
        "  worktree: {}  ({})",
        checkout.worktree.display(),
        checkout.branch
    );
    if let Some(wanted) = &checkout.stale {
        eprintln!(
            "  stale: dirty, left at {} instead of {}",
            &checkout.head_sha[..12],
            &wanted[..12]
        );
    }
    eprintln!(
        "  merge base: {}",
        &checkout.merge_base[..12.min(checkout.merge_base.len())]
    );
    eprintln!("  prepared in {:.1}s", started.elapsed().as_secs_f32());

    let git = preparer.git();
    let name_status = git
        .run(
            Some(&checkout.bare),
            [
                "diff",
                "-M",
                "--name-status",
                "-z",
                &checkout.merge_base,
                &checkout.head_sha,
            ],
            Duration::from_secs(60),
        )
        .await?;
    let numstat = git
        .run(
            Some(&checkout.bare),
            [
                "diff",
                "-M",
                "--numstat",
                "-z",
                &checkout.merge_base,
                &checkout.head_sha,
            ],
            Duration::from_secs(60),
        )
        .await?;

    let files = diff::merge_file_lists(
        diff::parse_name_status(&name_status),
        diff::parse_numstat(&numstat),
    );

    eprintln!("\nchanged files: {}", files.len());
    for f in &files {
        eprintln!(
            "  {} {:<60} +{:<5} -{}",
            f.status.letter(),
            f.path,
            f.additions,
            f.deletions
        );
    }

    // Exercise the line diff on the first text file that still exists.
    if let Some(f) = files
        .iter()
        .find(|f| !f.binary && f.status != diff::ChangeStatus::Deleted && f.path.ends_with(".rs"))
    {
        let head = preparer
            .show_blob(&checkout.bare, &checkout.head_sha, &f.path)
            .await?;
        let base = if f.status == diff::ChangeStatus::Added {
            String::new()
        } else {
            let source = f.old_path.as_deref().unwrap_or(&f.path);
            preparer
                .show_blob(&checkout.bare, &checkout.merge_base, source)
                .await?
        };

        let d = diff::file_diff(&base, &head);
        let added = d
            .line_status
            .iter()
            .filter(|s| **s == diff::LineStatus::Added)
            .count();
        let modified = d
            .line_status
            .iter()
            .filter(|s| **s == diff::LineStatus::Modified)
            .count();
        let ghosts: usize = d.deleted.iter().map(|r| r.lines.len()).sum();
        eprintln!(
            "\nline diff for {}: {} lines, {added} added, {modified} modified, {ghosts} ghost, {} hunks",
            f.path,
            d.line_status.len(),
            d.hunks.len()
        );
    }

    let tree = perch_git::list_tree(&checkout.worktree, 20_000)?;
    eprintln!("\nworktree entries: {}", tree.len());
    for e in tree.iter().take(8) {
        eprintln!("  {}{}", "  ".repeat(e.depth), e.path);
    }

    Ok(())
}
