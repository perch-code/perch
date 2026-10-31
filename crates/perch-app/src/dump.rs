//! `perch --dump-file owner/repo#N path/to/file.rs [--whole] [--split]`
//!
//! Renders a file exactly as the review view would, as text: the document
//! the editor draws, with gutter numbers, change bars, the highlight
//! capture under each token, a divider where a run of unchanged lines is
//! folded away, and a spacer for each row a thread's card takes, with the
//! card's messages under its first spacer, one line each: the author, a
//! `[draft]` mark on a message that is on its way to GitHub, a `[perch]`
//! mark on one no one on GitHub can see when the thread has something
//! there, and the body's first line. Lets the data path be checked
//! without a window. The folds are around the
//! changes and the anchors of the item's threads, less the reveals stored
//! for the file's blob; the cards are the item's threads anchored in the
//! file, the reader's own and GitHub's review threads alike, at their
//! estimated heights, since nothing has laid them out; all read from the
//! cache the app uses.
//! `--whole` renders the buffer whole, with nothing folded and no card.
//! The window's Show whole is the open tab's and is not stored, so none
//! of it is read here.
//! `--split` renders both sides of the split, one after the other, each
//! with its dividers and spacers, and ends in the two row counts, so that
//! the sides being the same height can be checked by eye and by test.

use std::sync::Arc;

use perch_git::{ChangeStatus, Preparer, RepoId, Row, Scope, Store};
use perch_github::{resolve_token, Author, Cache, CommentMessage, CommentThread, GitHub, ItemKind};

use crate::review::doc::{self, Card, Doc, DocRow, Reveals, Tint};
use crate::review::editor::{self, Side};
use crate::review::model::LoadedFile;

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
    let [target, path] = positional.as_slice() else {
        usage()
    };

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

    let config = crate::repos::load_resolved_async().await;
    let token = resolve_token(&config).ok_or_else(|| "no token".to_string())?;
    let gh = GitHub::new(&token.value).map_err(|e| e.to_string())?;
    let pr = gh
        .fetch_pull_request(full_name, number)
        .await
        .map_err(|e| e.to_string())?;

    let root = perch_github::config::data_dir().ok_or_else(|| "no data dir".to_string())?;
    let preparer = Preparer::new(Store::new(root), Some(token.value.clone()));
    let checkout = preparer
        .prepare(
            &id,
            number,
            &pr.head_sha,
            &pr.base_ref,
            &format!("perch/pr-{number}"),
            |_| {},
        )
        .await
        .map_err(|e| e.to_string())?;

    // The changed list the window reads, which carries each file's head
    // blob, and the two sides read the way the window reads them.
    let scope = Scope::Branch;
    let git = preparer.git();
    let files = perch_git::changed_files(
        git,
        &checkout.worktree,
        &scope,
        &checkout.merge_base,
        &checkout.head_sha,
    )
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
    let blob = entry
        .as_ref()
        .and_then(|f| f.head_blob.clone())
        .unwrap_or_default();

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
    let Kept {
        anchors,
        reveals,
        cards,
        threads,
    } = if whole {
        Kept::none()
    } else {
        kept_for(full_name, number, &file)
    };
    let folded = !whole;
    let sides = if split {
        let sides = editor::split_doc(&file, folded, &anchors, &reveals, &cards, None);
        if sides.is_none() {
            eprintln!(
                "note: {path} has no base side; the window shows it stacked, and so does this"
            );
        }
        sides
    } else {
        None
    };
    let rendered = match sides {
        Some((left, right)) => Rendered::Split(Box::new(left), Box::new(right)),
        None => Rendered::Stacked(Box::new(editor::stacked_doc(
            &file, folded, &anchors, &reveals, &cards, None,
        ))),
    };

    println!("file:     {}", file.path);
    println!("language: {:?}", file.language.map(|l| l.name()));
    println!("status:   {:?}", file.status);
    println!("lines:    {}", file.buffer.len());
    println!("changes:  {}", file.buffer.change_starts.len());
    println!(
        "blob:     {}",
        if file.blob.is_empty() {
            "none"
        } else {
            &file.blob
        }
    );
    if !whole {
        println!("anchors:  {}", anchors.len());
        println!("reveals:  {}", reveals_line(&reveals));
        println!("hidden:   {}", rendered.hidden_lines());
        println!("cards:    {}", cards.len());
    }

    match rendered {
        Rendered::Stacked(doc) => {
            println!();
            print_rows(&file, &doc, None, &threads);
        }
        Rendered::Split(left, right) => {
            println!();
            println!("base");
            print_rows(&file, &left, Some(Side::Base), &threads);
            println!();
            println!("head");
            print_rows(&file, &right, Some(Side::Head), &threads);
            println!();
            println!("rows:     {} base, {} head", left.len(), right.len());
        }
    }

    Ok(())
}

/// What is printed: the stacked document, or a split's two sides. Boxed,
/// so the split is not twice the size of the other.
enum Rendered {
    Stacked(Box<Doc>),
    Split(Box<Doc>, Box<Doc>),
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
/// divider as its expanders and its hunk header, a spacer as its card,
/// with the thread's messages under its first, a filler blank. The base
/// side's document
/// is over the base file, whose lines are its own; the stacked document
/// and the head side are over the unified buffer, which says which file
/// each line is from.
fn print_rows(file: &LoadedFile, doc: &Doc, side: Option<Side>, threads: &[CommentThread]) {
    println!("  gut  bar  text");
    println!("  ---  ---  ----");
    for (index, entry) in doc.rows.iter().enumerate() {
        let (display, tint) = match entry {
            DocRow::Line { display, tint } => (*display, *tint),
            DocRow::Divider { run, expand } => {
                // The header on the divider's first row, as the pane draws
                // it; each row's expander is named at its left.
                let first = doc.divider_span(index).is_some_and(|s| s.start == index);
                let label = if first {
                    let base = file.base_buffer.as_ref().map(|b| b.text.as_str());
                    let labels = doc::hunk_labels(&file.buffer, base, &doc.folds);
                    labels.get(*run).cloned().flatten().unwrap_or_default()
                } else {
                    String::new()
                };
                println!("  {:<4} {label}", format!("{expand:?}"));
                continue;
            }
            DocRow::Spacer { card, row } => {
                let total = doc
                    .rows
                    .iter()
                    .filter(|r| matches!(r, DocRow::Spacer { card: c, .. } if c == card));
                println!(
                    "  [{}, row {} of {}]",
                    card_name(card),
                    row + 1,
                    total.count()
                );
                if let (doc::CardId::Thread(id), 0) = (card, row) {
                    if let Some(thread) = threads.iter().find(|t| &t.id == id) {
                        let mixed = thread.has_github_message();
                        for message in &thread.messages {
                            println!("{}", message_line(message, mixed));
                        }
                    }
                }
                continue;
            }
            DocRow::Filler => {
                println!();
                continue;
            }
            // The rest of a wrapped line, under it, with no number.
            DocRow::Wrap { .. } => {
                println!("            {}", doc.line(index));
                continue;
            }
        };
        let (number, spans) = match side {
            Some(Side::Base) => (display + 1, file.base_spans.line(display)),
            Some(Side::Head) | None => match file.buffer.row(display) {
                Some(Row::Head { head_line, .. }) => {
                    (head_line + 1, file.head_spans.line(head_line))
                }
                Some(Row::Deleted { base_line }) => {
                    (base_line + 1, file.base_spans.line(base_line))
                }
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

/// How a spacer names its card: `thread abc123`, `thread gh-12345` for
/// one of GitHub's, or `composer`.
fn card_name(card: &doc::CardId) -> String {
    match card {
        doc::CardId::Thread(id) => format!("thread {id}"),
        doc::CardId::Composer => "composer".to_string(),
        doc::CardId::Step(i) => format!("step {}", i + 1),
        doc::CardId::Entry(id) => id.clone(),
    }
}

/// A message under its card's spacer: `    you [draft]: I will fix it`,
/// `    sonnet [perch]: because`, `    rohan: epoch`. The author as the
/// byline names them, `[draft]` after one that has not gone to GitHub
/// yet, `[perch]` after one no one on GitHub can see when the thread is
/// `mixed`, that is, has something on GitHub, as the window labels a
/// message Perch only there; and the body's first line.
fn message_line(message: &CommentMessage, mixed: bool) -> String {
    let author = match &message.author {
        Author::User => "you",
        Author::Agent { model, .. } => model.as_str(),
        Author::GitHubUser(login) => login.as_str(),
    };
    let mark = if message.is_draft() {
        " [draft]"
    } else if mixed && message.is_private() {
        " [perch]"
    } else {
        ""
    };
    let body = message.body.lines().next().unwrap_or_default();
    format!("    {author}{mark}: {body}")
}

/// The reveals as the gutter numbers them: `5-24, 44-63`, or `none`.
fn reveals_line(reveals: &Reveals) -> String {
    if reveals.is_empty() {
        return "none".to_string();
    }
    reveals
        .iter()
        .map(|(first, last)| format!("{}-{}", first + 1, last + 1))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the cache holds for an item that shapes a file's document: the
/// head lines its threads are anchored on, the reader's own and GitHub's
/// review comments, as the window counts them; the reveals stored for the
/// file's blob; and the reader's threads anchored in the file and GitHub's
/// review threads on it as cards, each at its estimate, open, since
/// nothing has been collapsed or laid out here; and the threads
/// themselves, for the messages printed under each card. No composer:
/// nothing is being written.
struct Kept {
    anchors: Vec<(usize, usize)>,
    reveals: Reveals,
    cards: Vec<Card>,
    threads: Vec<CommentThread>,
}

impl Kept {
    /// Nothing kept: folded around the changes alone, as a new item is.
    fn none() -> Self {
        Self {
            anchors: Vec::new(),
            reveals: Reveals::default(),
            cards: Vec::new(),
            threads: Vec::new(),
        }
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
        eprintln!(
            "note: no cache at {}; folding around the changes alone",
            db.display()
        );
        return Kept::none();
    }
    match kept_in(&db, full_name, number, file) {
        Ok(Some(kept)) => kept,
        Ok(None) => {
            eprintln!(
                "note: {full_name}#{number} is not in the cache; folding around the changes alone"
            );
            Kept::none()
        }
        Err(e) => {
            eprintln!("note: could not read the cache ({e}); folding around the changes alone");
            Kept::none()
        }
    }
}

/// `None` when the item is not in the cache.
fn kept_in(
    db: &std::path::Path,
    full_name: &str,
    number: u64,
    file: &LoadedFile,
) -> Result<Option<Kept>, String> {
    let cache = Cache::open(db, None).map_err(|e| e.to_string())?;
    let Some(item) = cache
        .find_item_by_subject(full_name, ItemKind::PullRequest, number)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let threads = cache
        .load_comment_threads(&item.id)
        .map_err(|e| e.to_string())?;
    let anchors = crate::review::model::anchors_of(&threads, &file.path);
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
    let cards = crate::review::model::cards_of(
        &threads,
        None,
        file,
        &Default::default(),
        &Default::default(),
    );
    Ok(Some(Kept {
        anchors,
        reveals,
        cards,
        threads,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::ThreadOrigin;

    fn message(author: Author, origin: ThreadOrigin, github_id: Option<u64>) -> CommentMessage {
        CommentMessage {
            id: "m".into(),
            thread_id: "t".into(),
            origin,
            author,
            body: "I will fix it\nlater".into(),
            created_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            run: None,
            github_id,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Default::default(),
        }
    }

    #[test]
    fn a_draft_is_marked_after_its_author_and_a_posted_comment_is_not() {
        let draft = message(Author::User, ThreadOrigin::GitHub, None);
        assert_eq!(message_line(&draft, true), "    you [draft]: I will fix it");
        let posted = message(
            Author::GitHubUser("rohan".into()),
            ThreadOrigin::GitHub,
            Some(31),
        );
        assert_eq!(message_line(&posted, true), "    rohan: I will fix it");
        let answer = message(
            Author::Agent {
                backend: "claude-code".into(),
                model: "sonnet".into(),
            },
            ThreadOrigin::Local,
            None,
        );
        assert_eq!(message_line(&answer, false), "    sonnet: I will fix it");
    }

    /// The private rows of a thread with something on GitHub are marked;
    /// in a thread that is wholly Perch's nothing is, as the window draws
    /// no lane there.
    #[test]
    fn a_private_message_is_marked_only_in_a_thread_with_a_github_message() {
        let question = message(Author::User, ThreadOrigin::Local, None);
        let answer = message(
            Author::Agent {
                backend: "claude-code".into(),
                model: "sonnet".into(),
            },
            ThreadOrigin::Local,
            None,
        );
        assert_eq!(
            message_line(&question, true),
            "    you [perch]: I will fix it"
        );
        assert_eq!(
            message_line(&answer, true),
            "    sonnet [perch]: I will fix it"
        );
        assert_eq!(message_line(&question, false), "    you: I will fix it");
        assert_eq!(message_line(&answer, false), "    sonnet: I will fix it");
    }
}
