//! `perch --dump-file owner/repo#N path/to/file.rs`
//!
//! Renders a file exactly as the review view would, as text: gutter numbers,
//! change bars, and the highlight capture under each token. Lets the data
//! path be checked without a window.

use std::sync::Arc;

use perch_git::{ChangeStatus, Preparer, RepoId, Row, Store};
use perch_github::{resolve_token, Config, GitHub};

pub fn run(args: &[String]) {
    if args.len() < 2 {
        eprintln!("usage: perch --dump-file owner/repo#number path");
        std::process::exit(2);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    if let Err(e) = runtime.block_on(dump(&args[0], &args[1])) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn dump(target: &str, path: &str) -> Result<(), String> {
    let (full_name, number) = target
        .split_once('#')
        .ok_or_else(|| "expected owner/repo#number".to_string())?;
    let number: u64 = number.parse().map_err(|_| "bad number".to_string())?;
    let id = RepoId::parse(full_name).ok_or_else(|| "bad repo".to_string())?;

    let config = Config::load();
    let token = resolve_token(&config).ok_or_else(|| "no token".to_string())?;
    let gh = GitHub::new(&token.value).map_err(|e| e.to_string())?;
    let pr = gh
        .fetch_pull_request(full_name, number)
        .await
        .map_err(|e| e.to_string())?;

    let root = perch_github::config::data_dir().ok_or_else(|| "no data dir".to_string())?;
    let preparer = Preparer::new(Store::new(root), Some(token.value.clone()));
    let checkout = preparer
        .prepare(&id, number, &pr.head_sha, &pr.base_ref, &format!("perch/pr-{number}"), |_| {})
        .await
        .map_err(|e| e.to_string())?;

    let name_status = preparer
        .git()
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
            std::time::Duration::from_secs(60),
        )
        .await
        .map_err(|e| e.to_string())?;
    let numstat = preparer
        .git()
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
            std::time::Duration::from_secs(60),
        )
        .await
        .map_err(|e| e.to_string())?;

    let files = perch_git::diff::merge_file_lists(
        perch_git::diff::parse_name_status(&name_status),
        perch_git::diff::parse_numstat(&numstat),
    );
    let entry = files.iter().find(|f| f.path == path).cloned();

    let head = if entry.as_ref().map(|f| f.status) == Some(ChangeStatus::Deleted) {
        String::new()
    } else {
        std::fs::read_to_string(checkout.worktree.join(path)).unwrap_or_default()
    };
    let base = match &entry {
        Some(f) if f.status != ChangeStatus::Added => {
            let source = f.old_path.clone().unwrap_or_else(|| f.path.clone());
            preparer
                .show_blob(&checkout.bare, &checkout.merge_base, &source)
                .await
                .unwrap_or_default()
        }
        _ => String::new(),
    };

    let file = Arc::new(crate::backend::build_loaded_file(
        path.to_string(),
        head,
        base,
        entry.as_ref().map(|f| f.status),
        entry.as_ref().map(|f| f.binary).unwrap_or(false),
    ));

    println!("file:     {}", file.path);
    println!("language: {:?}", file.language.map(|l| l.name()));
    println!("status:   {:?}", file.status);
    println!("lines:    {}", file.buffer.len());
    println!("changes:  {}", file.buffer.change_starts.len());
    println!();
    println!("  gut  bar  text");
    println!("  ---  ---  ----");

    for (display_line, row) in file.buffer.rows.iter().enumerate() {
        let text = file.buffer.text.lines().nth(display_line).unwrap_or("");
        let (number, bar) = match row {
            Row::Head { head_line, status } => (
                head_line + 1,
                match status {
                    perch_git::LineStatus::Added => '+',
                    perch_git::LineStatus::Modified => '~',
                    perch_git::LineStatus::Unchanged => ' ',
                },
            ),
            Row::Deleted { base_line } => (base_line + 1, '-'),
        };

        // Only print around the changes, so a long file stays readable.
        let near_change = file
            .buffer
            .change_starts
            .iter()
            .any(|c| display_line + 6 >= *c && *c + 10 >= display_line);
        if !near_change && !file.buffer.change_starts.is_empty() {
            continue;
        }

        let spans = match &row {
            Row::Head { head_line, .. } => file.head_spans.line(*head_line),
            Row::Deleted { base_line } => file.base_spans.line(*base_line),
        };
        let captures = spans.len();

        println!(
            "  {number:>4} {bar}   {text}{}",
            if captures > 0 {
                format!("   [{captures} spans]")
            } else {
                String::new()
            }
        );
    }

    Ok(())
}
