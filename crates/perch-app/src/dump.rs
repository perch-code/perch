//! `perch --dump-file owner/repo#N path/to/file.rs [--whole] [--split]`
//!
//! Renders a file exactly as the review view would, as text: the document
//! the editor draws, with gutter numbers, change bars, the highlight
//! capture under each token, and a divider where a run of unchanged lines
//! is folded away. Lets the data path be checked without a window. The
//! folds are around the changes and the anchors of the item's threads,
//! less the reveals stored for the file's blob, and none at all when Show
//! whole is off for the item, all read from the cache the app uses;
//! `--whole` renders the buffer whole, with nothing folded. `--split`
//! renders both sides of the split, one after the other, each with its
//! dividers, and ends in the two row counts, so that the sides being the
//! same height can be checked by eye and by test.

use std::sync::Arc;

use perch_git::{ChangeStatus, Preparer, RepoId, Row, Scope, Store};
use perch_github::{resolve_token, Cache, Config, GitHub, ItemKind};

use crate::review::doc::{self, Doc, DocRow, Reveals, Tint};
use crate::review::editor::{self, Side};
use crate::review::model::{folded_from_meta, folded_meta_key, LoadedFile};

pub fn run(args: &[String]) {
    let mut positional = Vec::new();
    let mut whole = false;
    let mut split = false;
    for arg in args {
        match arg.as_str() {
            "--whole" => whole = true,
            "--split" => split = true,
            flag if flag.starts_with("--") => usage(),
            _ => positional.push(arg.clone()),
        }
    }
    let [target, path] = positional.as_slice() else { usage() };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    if let Err(e) = runtime.block_on(dump(target, path, whole, split)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("usage: perch --dump-file owner/repo#number path [--whole] [--split]");
    std::process::exit(2);
}

async fn dump(target: &str, path: &str, whole: bool, split: bool) -> Result<(), String> {
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

    // The changed list the window reads, which carries each file's head
    // blob, and the two sides read the way the window reads them.
    let scope = Scope::Branch;
    let git = preparer.git();
    let files = perch_git::changed_files(git, &checkout.worktree, &scope, &checkout.merge_base, &checkout.head_sha)
        .await
        .map_err(|e| e.to_string())?;
    let entry = files.iter().find(|f| f.path == path).cloned();
    let (base_rev, head_rev) = scope.sides(&checkout.merge_base, &checkout.head_sha);

    let head = match &entry {
        Some(f) if f.status == ChangeStatus::Deleted => String::new(),
        Some(_) => perch_git::read_side(git, &checkout.worktree, &head_rev, path)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{path} is not there"))?,
        // A file outside the changed list is read from disk, as the
        // window reads it.
        None => std::fs::read_to_string(checkout.worktree.join(path)).map_err(|e| e.to_string())?,
    };
    let base = match &entry {
        Some(f) if f.status != ChangeStatus::Added => {
            let source = f.old_path.clone().unwrap_or_else(|| f.path.clone());
            perch_git::read_side(git, &checkout.worktree, &base_rev, &source)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        }
        _ => String::new(),
    };
    let blob = entry.as_ref().and_then(|f| f.head_blob.clone()).unwrap_or_default();

    let file = Arc::new(crate::backend::build_loaded_file(
        path.to_string(),
        head,
        base,
        entry.as_ref().map(|f| f.status),
        entry.as_ref().map(|f| f.binary).unwrap_or(false),
        scope,
        blob,
    ));

    // The documents as the window would build them, from what the cache
    // holds for the item. A file with no base side is stacked in the
    // window whatever the layout, and here too.
    let Kept { anchors, reveals, folded } = if whole { Kept::none() } else { kept_for(full_name, number, &file) };
    let folded = folded && !whole;
    let sides = if split {
        let sides = editor::split_doc(&file, folded, &anchors, &reveals);
        if sides.is_none() {
            eprintln!("note: {path} has no base side; the window shows it stacked, and so does this");
        }
        sides
    } else {
        None
    };
    let rendered = match sides {
        Some((left, right)) => Rendered::Split(left, right),
        None => Rendered::Stacked(editor::stacked_doc(&file, folded, &anchors, &reveals)),
    };

    println!("file:     {}", file.path);
    println!("language: {:?}", file.language.map(|l| l.name()));
    println!("status:   {:?}", file.status);
    println!("lines:    {}", file.buffer.len());
    println!("changes:  {}", file.buffer.change_starts.len());
    println!("blob:     {}", if file.blob.is_empty() { "none" } else { &file.blob });
    if !whole {
        println!("anchors:  {}", anchors.len());
        println!("reveals:  {}", reveals_line(&reveals));
        println!("folded:   {}", if folded { "yes" } else { "no, the item's files are shown whole" });
        println!("hidden:   {}", rendered.hidden_lines());
    }

    match rendered {
        Rendered::Stacked(doc) => {
            println!();
            print_rows(&file, &doc, None);
        }
        Rendered::Split(left, right) => {
            println!();
            println!("base");
            print_rows(&file, &left, Some(Side::Base));
            println!();
            println!("head");
            print_rows(&file, &right, Some(Side::Head));
            println!();
            println!("rows:     {} base, {} head", left.len(), right.len());
        }
    }

    Ok(())
}

/// What is printed: the stacked document, or a split's two sides.
enum Rendered {
    Stacked(Doc),
    Split(Doc, Doc),
}

impl Rendered {
    /// The folds are the same on both sides of a split, so either side
    /// says how many lines they hide.
    fn hidden_lines(&self) -> usize {
        match self {
            Rendered::Stacked(doc) | Rendered::Split(_, doc) => doc.hidden_lines(),
        }
    }
}

/// The rows of one document, one per line: the gutter number, the
/// change bar, the text and how many highlight spans it carries; a
/// divider as its label, a spacer as its card, a filler blank. The base
/// side's document is over the base file, whose lines are its own; the
/// stacked document and the head side are over the unified buffer, which
/// says which file each line is from.
fn print_rows(file: &LoadedFile, doc: &Doc, side: Option<Side>) {
    println!("  gut  bar  text");
    println!("  ---  ---  ----");
    for (index, entry) in doc.rows.iter().enumerate() {
        let (display, tint) = match entry {
            DocRow::Line { display, tint } => (*display, *tint),
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
        let (number, spans) = match side {
            Some(Side::Base) => (display + 1, file.base_spans.line(display)),
            Some(Side::Head) | None => match file.buffer.row(display) {
                Some(Row::Head { head_line, .. }) => (head_line + 1, file.head_spans.line(head_line)),
                Some(Row::Deleted { base_line }) => (base_line + 1, file.base_spans.line(base_line)),
                None => continue,
            },
        };
        let bar = match tint {
            Tint::Added => '+',
            Tint::Modified => '~',
            Tint::Deleted => '-',
            Tint::Plain => ' ',
        };
        let text = doc.line(index);
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

/// The reveals as the gutter numbers them: `5-24, 44-63`, or `none`.
fn reveals_line(reveals: &Reveals) -> String {
    if reveals.is_empty() {
        return "none".to_string();
    }
    reveals.iter().map(|(first, last)| format!("{}-{}", first + 1, last + 1)).collect::<Vec<_>>().join(", ")
}

/// What the cache holds for an item that shapes a file's document: the
/// head lines its threads are anchored on, the reader's own and GitHub's
/// review comments, as the window counts them; the reveals stored for the
/// file's blob; and whether the item's files are folded at all.
struct Kept {
    anchors: Vec<(usize, usize)>,
    reveals: Reveals,
    folded: bool,
}

impl Kept {
    /// Nothing kept: folded around the changes alone, as a new item is.
    fn none() -> Self {
        Self { anchors: Vec::new(), reveals: Reveals::default(), folded: true }
    }
}

/// What is kept for the item, or nothing when there is no cache or the
/// item is not in it, with a note on stderr saying so, so stdout stays
/// the document alone.
fn kept_for(full_name: &str, number: u64, file: &LoadedFile) -> Kept {
    let Some(db) = perch_github::config::db_path() else {
        return Kept::none();
    };
    if !db.exists() {
        eprintln!("note: no cache at {}; folding around the changes alone", db.display());
        return Kept::none();
    }
    match kept_in(&db, full_name, number, file) {
        Ok(Some(kept)) => kept,
        Ok(None) => {
            eprintln!("note: {full_name}#{number} is not in the cache; folding around the changes alone");
            Kept::none()
        }
        Err(e) => {
            eprintln!("note: could not read the cache ({e}); folding around the changes alone");
            Kept::none()
        }
    }
}

/// `None` when the item is not in the cache.
fn kept_in(db: &std::path::Path, full_name: &str, number: u64, file: &LoadedFile) -> Result<Option<Kept>, String> {
    let cache = Cache::open(db, None).map_err(|e| e.to_string())?;
    let Some(item) = cache
        .find_item_by_subject(full_name, ItemKind::PullRequest, number)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let threads = cache.load_comment_threads(&item.id).map_err(|e| e.to_string())?;
    let discussion = cache.load_discussion(&item.id).map_err(|e| e.to_string())?.map(|(_, d)| d);
    let anchors = crate::review::model::anchors_of(&threads, discussion.as_ref(), &file.path);
    // A file with no blob has no rows, as in the window.
    let reveals = if file.blob.is_empty() {
        Reveals::default()
    } else {
        cache
            .load_reveals(&item.id)
            .map_err(|e| e.to_string())?
            .remove(&(file.path.clone(), file.blob.clone()))
            .unwrap_or_default()
            .into_iter()
            .map(|(a, b)| (a as usize, b as usize))
            .collect()
    };
    let folded = folded_from_meta(cache.meta(&folded_meta_key(&item.id)).map_err(|e| e.to_string())?.as_deref());
    Ok(Some(Kept { anchors, reveals, folded }))
}
