//! `perch --dump-file owner/repo#N path/to/file.rs [--whole]`
//!
//! Renders a file exactly as the review view would, as text: the document
//! the editor draws, with gutter numbers, change bars, the highlight
//! capture under each token, and a divider where a run of unchanged lines
//! is folded away. Lets the data path be checked without a window. The
//! folds are around the changes and the anchors of the item's threads,
//! read from the cache the app uses; `--whole` renders the buffer whole,
//! with nothing folded.

use std::sync::Arc;

use perch_git::{ChangeStatus, Preparer, RepoId, Row, Store};
use perch_github::{resolve_token, Cache, Config, GitHub, ItemKind};

use crate::review::doc::{self, DocRow};

pub fn run(args: &[String]) {
    let mut positional = Vec::new();
    let mut whole = false;
    for arg in args {
        match arg.as_str() {
            "--whole" => whole = true,
            flag if flag.starts_with("--") => usage(),
            _ => positional.push(arg.clone()),
        }
    }
    let [target, path] = positional.as_slice() else { usage() };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    if let Err(e) = runtime.block_on(dump(target, path, whole)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("usage: perch --dump-file owner/repo#number path [--whole]");
    std::process::exit(2);
}

async fn dump(target: &str, path: &str, whole: bool) -> Result<(), String> {
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
        perch_git::Scope::Branch,
    ));

    // The document as the window would build it: the reveals are not
    // stored yet, so the folds are the defaults.
    let anchors = if whole { Vec::new() } else { anchors_from_cache(full_name, number, path) };
    let folds = if whole { Vec::new() } else { doc::runs(&file.buffer, &anchors) };
    let doc = doc::build(&file.buffer, &folds, &[]);

    println!("file:     {}", file.path);
    println!("language: {:?}", file.language.map(|l| l.name()));
    println!("status:   {:?}", file.status);
    println!("lines:    {}", file.buffer.len());
    println!("changes:  {}", file.buffer.change_starts.len());
    if !whole {
        println!("anchors:  {}", anchors.len());
        println!("hidden:   {}", doc.hidden_lines());
    }
    println!();
    println!("  gut  bar  text");
    println!("  ---  ---  ----");

    for (index, entry) in doc.rows.iter().enumerate() {
        let row = match entry {
            DocRow::Line { display, .. } => match file.buffer.row(*display) {
                Some(row) => row,
                None => continue,
            },
            DocRow::Divider { run } => {
                let hidden = doc.folds.get(*run).map(doc::Run::len).unwrap_or(0);
                println!("  \u{00b7}\u{00b7}\u{00b7} {} \u{00b7}\u{00b7}\u{00b7}", doc::divider_label(hidden));
                continue;
            }
            DocRow::Spacer { card, row } => {
                let total = doc.rows.iter().filter(|r| matches!(r, DocRow::Spacer { card: c, .. } if c == card));
                println!("  [{}, row {} of {}]", card_name(card), row + 1, total.count());
                continue;
            }
            DocRow::Filler => {
                println!();
                continue;
            }
        };
        let text = doc.line(index);
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

/// How a spacer names its card: `thread abc123`, `github 12345`, or
/// `composer`.
fn card_name(card: &doc::CardId) -> String {
    match card {
        doc::CardId::Thread(id) => format!("thread {id}"),
        doc::CardId::GitHub(id) => format!("github {id}"),
        doc::CardId::Composer => "composer".to_string(),
    }
}

/// The head lines the item's threads are anchored on in a file, from the
/// cache: the reader's own and GitHub's review comments, as the window
/// counts them. None when there is no cache or the item is not in it, and
/// a note on stderr says so, so stdout stays the document alone.
fn anchors_from_cache(full_name: &str, number: u64, path: &str) -> Vec<(usize, usize)> {
    let Some(db) = perch_github::config::db_path() else {
        return Vec::new();
    };
    if !db.exists() {
        eprintln!("note: no cache at {}; folding around the changes alone", db.display());
        return Vec::new();
    }
    match anchors_in(&db, full_name, number, path) {
        Ok(Some(anchors)) => anchors,
        Ok(None) => {
            eprintln!("note: {full_name}#{number} is not in the cache; folding around the changes alone");
            Vec::new()
        }
        Err(e) => {
            eprintln!("note: could not read the cache ({e}); folding around the changes alone");
            Vec::new()
        }
    }
}

/// `None` when the item is not in the cache.
fn anchors_in(
    db: &std::path::Path,
    full_name: &str,
    number: u64,
    path: &str,
) -> Result<Option<Vec<(usize, usize)>>, String> {
    let cache = Cache::open(db, None).map_err(|e| e.to_string())?;
    let Some(item) = cache
        .find_item_by_subject(full_name, ItemKind::PullRequest, number)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let threads = cache.load_comment_threads(&item.id).map_err(|e| e.to_string())?;
    let discussion = cache.load_discussion(&item.id).map_err(|e| e.to_string())?.map(|(_, d)| d);
    Ok(Some(crate::review::model::anchors_of(&threads, discussion.as_ref(), path)))
}
