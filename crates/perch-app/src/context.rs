//! What an agent is given of the discussion on the item it is asked about.
//!
//! The prompt carries what is near and the disk carries everything. Every
//! run gets the whole discussion as files (see
//! [`perch_knowledge::discussion`]), and its prompt gets the part chosen
//! here: the reviews, the checks, the threads near the lines asked about,
//! then the rest in a fixed order until a budget in characters is spent.
//! What did not fit is listed by one line each. A follow-up that resumes a
//! session is told only what changed since the run it resumes.

use std::path::{Path, PathBuf};

use perch_github::comments::{review_threads_of, Author};
use perch_github::{Anchor, Checks, Discussion, PullEvent, PullEventKind};
use perch_knowledge::discussion::{
    self as files, CheckEntry, CheckOutcome, DiscussionArchive, EntryKey, EventLine, Manifest,
    Post, ReviewEntry, ThreadEntry, Verdict,
};
use perch_knowledge::sanitize::{clean, strip_format_chars};

use crate::ask::untrusted_block;

/// How much of the prompt the discussion may take, in characters: about
/// six thousand tokens. The description is not counted.
pub const MAX_DISCUSSION_CHARS: usize = 24_000;
/// How much of one comment the prompt quotes before pointing at its file.
pub const MAX_COMMENT_CHARS: usize = 2_000;
/// How far from the lines asked about a thread is still near them.
pub const NEAR_LINES: u32 = 20;
/// How many lines the prompt's index lists before `and N more`.
pub const MAX_INDEX_LINES: usize = 60;

/// The discussion in the shape the files and the prompt are made from.
/// Review threads come from the discussion itself, never from Perch's own
/// threads, so the reader's private questions and an agent's answers stay
/// out. A pending review, the reader's own unfinished one, is left out,
/// and so is a review that only commented and said nothing: it is the
/// wrapper GitHub puts around comments on lines, which are in the threads.
pub fn archive(
    discussion: &Discussion,
    item_id: &str,
    head_sha: &str,
    opener: &str,
    checks: Option<&Checks>,
) -> DiscussionArchive {
    let description = (!discussion.body.trim().is_empty()).then(|| Post {
        id: 0,
        author: opener.to_string(),
        body: discussion.body.clone(),
        created_at: discussion.opened_at.unwrap_or_default(),
    });
    let reviews = discussion
        .reviews
        .iter()
        .filter(|review| !(review.state == "COMMENTED" && review.body.trim().is_empty()))
        .filter_map(|review| {
            Some(ReviewEntry {
                verdict: Verdict::parse(&review.state)?,
                post: Post {
                    id: review.id,
                    author: review.user.clone(),
                    body: review.body.clone(),
                    created_at: review.submitted_at?,
                },
            })
        })
        .collect();
    let comments = discussion
        .comments
        .iter()
        .map(|comment| Post {
            id: comment.id,
            author: comment.user.clone(),
            body: comment.body.clone(),
            created_at: comment.created_at,
        })
        .collect();
    let threads = review_threads_of(discussion, item_id, head_sha)
        .into_iter()
        .filter_map(|thread| {
            let root_id = thread.github_id?;
            let anchor = thread.anchor.as_ref()?;
            Some(ThreadEntry {
                root_id,
                path: anchor.path.clone(),
                start_line: anchor.start_line,
                line: anchor.line,
                side: anchor.side.name().to_string(),
                resolved: thread.resolved,
                outdated: thread.outdated,
                diff_hunk: thread
                    .messages
                    .first()
                    .and_then(|m| m.diff_hunk.clone())
                    .unwrap_or_default(),
                messages: thread
                    .messages
                    .iter()
                    .filter_map(|message| {
                        let Author::GitHubUser(login) = &message.author else {
                            return None;
                        };
                        Some(Post {
                            id: message.github_id?,
                            author: login.clone(),
                            body: message.body.clone(),
                            created_at: message.created_at,
                        })
                    })
                    .collect(),
            })
        })
        .collect();
    let events = discussion.events.iter().filter_map(event_line).collect();
    let checks = checks
        .map(|checks| {
            checks
                .runs
                .iter()
                .map(|run| CheckEntry {
                    name: run.name.clone(),
                    workflow: run.workflow.clone(),
                    outcome: if run.state.is_failing() {
                        CheckOutcome::Failing
                    } else if run.state.is_pending() {
                        CheckOutcome::Running
                    } else if run.state.is_passing() {
                        CheckOutcome::Passing
                    } else {
                        CheckOutcome::Other
                    },
                    description: run.description.clone(),
                    url: run.url.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    DiscussionArchive {
        description,
        reviews,
        comments,
        threads,
        events,
        checks,
    }
}

/// A timeline event in words, for the conversation file. The opening is
/// the description's own heading, and the rest of what is left out says
/// nothing about the change.
fn event_line(event: &PullEvent) -> Option<EventLine> {
    let actor = if event.actor.is_empty() {
        "someone"
    } else {
        event.actor.as_str()
    };
    let short = |sha: &str| sha.chars().take(7).collect::<String>();
    let text = match &event.kind {
        PullEventKind::Committed {
            sha,
            author,
            subject,
            ..
        } => {
            let who = if author.is_empty() {
                actor
            } else {
                author.as_str()
            };
            format!("{who} committed {} {subject}", short(sha))
        }
        PullEventKind::ForcePushed { sha } => format!("{actor} force-pushed to {}", short(sha)),
        PullEventKind::Merged => format!("{actor} merged it"),
        PullEventKind::Closed => format!("{actor} closed it"),
        PullEventKind::Reopened => format!("{actor} reopened it"),
        PullEventKind::Draft => format!("{actor} marked it a draft"),
        PullEventKind::Ready => format!("{actor} marked it ready for review"),
        PullEventKind::ReviewRequested { who } => format!("{actor} asked {who} for a review"),
        PullEventKind::ReviewRequestRemoved { who } => {
            format!("{actor} withdrew the review request to {who}")
        }
        PullEventKind::ReviewDismissed { who } => format!("{actor} dismissed {who}'s review"),
        PullEventKind::Labeled { label } => format!("{actor} added the label {}", label.name),
        PullEventKind::Unlabeled { label } => format!("{actor} removed the label {}", label.name),
        PullEventKind::Renamed { from, to } => format!("{actor} renamed it from {from} to {to}"),
        PullEventKind::CrossReferenced { reference } => {
            format!("{actor} mentioned it from {}", reference.slug())
        }
        PullEventKind::HeadDeleted => format!("{actor} deleted the head branch"),
        PullEventKind::HeadRestored => format!("{actor} restored the head branch"),
        _ => return None,
    };
    Some(EventLine { at: event.at, text })
}

/// One candidate for the prompt, with its block as the prompt would
/// quote it and its line for the index.
struct Candidate {
    key: EntryKey,
    /// Where it is shown among what fits: the tier, then an order within.
    rank: (u8, i64),
    block: String,
    index: String,
}

/// What the prompt shows of the discussion and what it only lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// The entries that fit, in the order they are shown.
    pub shown: Vec<EntryKey>,
    /// The entries that did not, most relevant first.
    pub omitted: Vec<EntryKey>,
    /// The section as it goes into the prompt, without the directory line.
    pub text: String,
}

/// Choose what a first question is shown: the tiers in order, each entry
/// whole or not at all, until `budget` characters are spent, keeping room
/// for the index of whatever is left out. `asked_in` is the thread the
/// question is asked in, which is its history and not repeated here.
pub fn select(
    archive: &DiscussionArchive,
    anchor: Option<&Anchor>,
    asked_in: Option<u64>,
    budget: usize,
) -> Selection {
    let candidates = candidates(archive, anchor, asked_in, |_| true);
    let checks = checks_block(archive);
    let (shown, omitted) = fill(&candidates, budget, checks.len(), true);
    let text = compose(&candidates, &shown, &omitted, &checks);
    Selection {
        shown: shown.iter().map(|&i| candidates[i].key).collect(),
        omitted: omitted.iter().map(|&i| candidates[i].key).collect(),
        text,
    }
}

/// Every entry the prompt could show, most relevant first. Bots are never
/// candidates: their comments are in the files only.
fn candidates(
    archive: &DiscussionArchive,
    anchor: Option<&Anchor>,
    asked_in: Option<u64>,
    keep: impl Fn(EntryKey) -> bool,
) -> Vec<Candidate> {
    let mut out = Vec::new();

    // 1. Each reviewer's latest verdict, and a later summary with words in
    // it, which a verdict-less follow-up review would otherwise hide.
    let mut reviewers: Vec<&str> = archive
        .reviews
        .iter()
        .filter(|r| !r.post.is_bot())
        .map(|r| r.post.author.as_str())
        .collect();
    reviewers.sort_unstable();
    reviewers.dedup();
    let mut reviews: Vec<&ReviewEntry> = Vec::new();
    for reviewer in reviewers {
        let theirs: Vec<&ReviewEntry> = archive
            .reviews
            .iter()
            .filter(|r| r.post.author == reviewer)
            .collect();
        let verdict = theirs
            .iter()
            .filter(|r| r.verdict != Verdict::Commented)
            .max_by_key(|r| r.post.created_at)
            .copied();
        let said = theirs
            .iter()
            .filter(|r| r.verdict == Verdict::Commented && !r.post.body.trim().is_empty())
            .max_by_key(|r| r.post.created_at)
            .copied();
        reviews.extend(verdict);
        if let Some(said) = said {
            if verdict.is_none_or(|v| said.post.created_at > v.post.created_at) {
                reviews.push(said);
            }
        }
    }
    reviews.sort_by_key(|r| std::cmp::Reverse(r.post.created_at));
    for review in reviews {
        let key = EntryKey::Review(review.post.id);
        out.push(Candidate {
            key,
            rank: (1, -review.post.created_at.as_second()),
            block: untrusted_block(
                &format!(
                    "review by {}, {}",
                    label(&review.post.author),
                    review.verdict.words()
                ),
                &cut(&review.post.body, key),
            ),
            index: files::index_line(archive, key).unwrap_or_default(),
        });
    }

    // 3, 4 and 6. Threads: near the lines first, then the unresolved ones,
    // this file's before the rest, then the resolved.
    let near = |t: &ThreadEntry| -> Option<u32> {
        let a = anchor?;
        (t.path == a.path && t.side == a.side.name()).then_some(())?;
        let gap = if t.line < a.start_line {
            a.start_line - t.line
        } else {
            t.start_line.saturating_sub(a.line)
        };
        (gap <= NEAR_LINES).then_some(gap)
    };
    let mut threads: Vec<(u8, (bool, u32, i64), &ThreadEntry)> = archive
        .threads
        .iter()
        .filter(|t| Some(t.root_id) != asked_in && !t.is_bot())
        .map(|t| {
            let recent = -t.last_activity().map_or(0, |at| at.as_second());
            let same_file = anchor.is_some_and(|a| a.path == t.path);
            match near(t) {
                Some(gap) => (3, (false, gap, recent), t),
                None if !t.resolved => (4, (!same_file, 0, recent), t),
                None => (6, (false, 0, recent), t),
            }
        })
        .collect();
    threads.sort_by_key(|(tier, order, _)| (*tier, *order));
    for (tier, order, thread) in threads {
        let key = EntryKey::Thread(thread.root_id);
        let messages: Vec<String> = thread
            .messages
            .iter()
            .filter(|m| !m.is_bot())
            .map(|m| format!("{}: {}", label(&m.author), cut(&m.body, key)))
            .collect();
        out.push(Candidate {
            key,
            rank: (tier, order.2),
            block: untrusted_block(
                &format!(
                    "thread by {} on {}, {}",
                    label(thread.author()),
                    label(&thread.location()),
                    thread.state()
                ),
                &messages.join("\n\n"),
            ),
            index: files::index_line(archive, key).unwrap_or_default(),
        });
    }

    // 5. The conversation tab, newest first to fill, oldest first to read.
    let mut comments: Vec<&Post> = archive.comments.iter().filter(|c| !c.is_bot()).collect();
    comments.sort_by_key(|c| std::cmp::Reverse(c.created_at));
    for comment in comments {
        let key = EntryKey::Comment(comment.id);
        out.push(Candidate {
            key,
            rank: (5, comment.created_at.as_second()),
            block: untrusted_block(
                &format!("comment by {}", label(&comment.author)),
                &cut(&comment.body, key),
            ),
            index: files::index_line(archive, key).unwrap_or_default(),
        });
    }

    // The threads were pushed 3, 4, 6 and the comments after them; the
    // conversation tab fills before the resolved threads.
    out.sort_by_key(|c| c.rank.0);
    out.retain(|c| keep(c.key));
    out
}

/// The checks as one line in a block of their own: the names are the
/// repository's, not Perch's.
fn checks_block(archive: &DiscussionArchive) -> String {
    if archive.checks.is_empty() {
        return String::new();
    }
    untrusted_block(
        "checks",
        &format!("Checks: {}", files::checks_summary(&archive.checks)),
    )
}

/// Which candidates fit in `budget`, greedily in order, and which do not.
/// With `index`, room is kept for the index of everything not yet shown,
/// so the section as a whole stays inside the budget.
fn fill(
    candidates: &[Candidate],
    budget: usize,
    already: usize,
    index: bool,
) -> (Vec<usize>, Vec<usize>) {
    let mut used = already + HEADER.len();
    let mut shown = Vec::new();
    let mut omitted: Vec<usize> = Vec::new();
    for (i, candidate) in candidates.iter().enumerate() {
        let reserve = if index {
            let rest: Vec<&str> = omitted
                .iter()
                .map(|&j| candidates[j].index.as_str())
                .chain(candidates[i + 1..].iter().map(|c| c.index.as_str()))
                .collect();
            index_block(&rest).len()
        } else {
            0
        };
        if used + candidate.block.len() + reserve <= budget {
            used += candidate.block.len();
            shown.push(i);
        } else {
            omitted.push(i);
        }
    }
    (shown, omitted)
}

const HEADER: &str = "What was said on GitHub, as much as fits:\n";
const INDEX_HEADER: &str = "Not shown above, most relevant first:\n";

/// The index of what was left out, at most [`MAX_INDEX_LINES`] of it.
/// Empty when nothing was.
fn index_block(lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines[..lines.len().min(MAX_INDEX_LINES)].join("\n");
    if lines.len() > MAX_INDEX_LINES {
        text.push_str(&format!(
            "\nand {} more in INDEX.md",
            lines.len() - MAX_INDEX_LINES
        ));
    }
    format!("{INDEX_HEADER}{}", untrusted_block("index", &text))
}

fn compose(candidates: &[Candidate], shown: &[usize], omitted: &[usize], checks: &str) -> String {
    let mut order: Vec<usize> = shown.to_vec();
    // By tier, in the order they filled, except that the threads read
    // together and the conversation tab comes last, oldest first.
    order.sort_by_key(|&i| {
        let c = &candidates[i];
        if c.rank.0 == 5 {
            (7, c.rank.1)
        } else {
            (c.rank.0, 0)
        }
    });
    let mut out = String::new();
    let mut checks_done = checks.is_empty();
    for i in order {
        if !checks_done && candidates[i].rank.0 > 1 {
            out.push_str(checks);
            checks_done = true;
        }
        out.push_str(&candidates[i].block);
    }
    if !checks_done {
        out.push_str(checks);
    }
    if !out.is_empty() {
        out.insert_str(0, HEADER);
    }
    let lines: Vec<&str> = omitted
        .iter()
        .map(|&i| candidates[i].index.as_str())
        .collect();
    out.push_str(&index_block(&lines));
    out
}

/// What a resumed follow-up is told has changed since the run it resumes,
/// or `None` when nothing has. Replies in the thread asked in are sent
/// whole; everything else goes through the same order as a first question
/// with what earlier runs of the session were shown counted as spent.
pub fn since(
    archive: &DiscussionArchive,
    previous: &Manifest,
    anchor: Option<&Anchor>,
    asked_in: Option<u64>,
) -> Option<String> {
    let now = Manifest::of(archive);
    let mut out = String::new();

    if let Some(thread) =
        asked_in.and_then(|root| archive.threads.iter().find(|t| t.root_id == root))
    {
        for message in thread.messages.iter().filter(|m| !m.is_bot()) {
            if let Some(edited) = previous.reply_change(message) {
                out.push_str(&untrusted_block(
                    &format!(
                        "{} by {} in this thread",
                        if edited { "edited reply" } else { "reply" },
                        label(&message.author)
                    ),
                    clean(&message.body).trim(),
                ));
            }
        }
    }

    let changed = candidates(archive, anchor, asked_in, |key| {
        previous.changed(&now, archive, key)
    });
    let budget = MAX_DISCUSSION_CHARS.saturating_sub(previous.spent + out.len());
    let (shown, _) = fill(&changed, budget, 0, false);
    let mut order = shown.clone();
    order.sort_by_key(|&i| changed[i].rank);
    for i in order {
        let candidate = &changed[i];
        let edited =
            !matches!(candidate.key, EntryKey::Thread(_)) && previous.was_seen(candidate.key);
        if edited {
            out.push_str("(edited)\n");
        }
        out.push_str(&candidate.block);
    }
    if !changed.is_empty() {
        let count = |pick: fn(&EntryKey) -> bool, one: &str, many: &str| {
            let n = changed.iter().filter(|c| pick(&c.key)).count();
            (n > 0).then(|| format!("{n} {}", if n == 1 { one } else { many }))
        };
        let parts: Vec<String> = [
            count(|k| matches!(k, EntryKey::Review(_)), "review", "reviews"),
            count(|k| matches!(k, EntryKey::Thread(_)), "thread", "threads"),
            count(|k| matches!(k, EntryKey::Comment(_)), "comment", "comments"),
        ]
        .into_iter()
        .flatten()
        .collect();
        out.push_str(&format!(
            "New or changed elsewhere: {}.\n",
            parts.join(", ")
        ));
    }

    let checks: Vec<String> = archive
        .checks
        .iter()
        .filter_map(|check| {
            let state = check.outcome.words();
            let name = one_line(&strip_format_chars(&check.name));
            match previous.checks.get(&check.name) {
                Some(before) if before == state => None,
                Some(_) => Some(format!("{name} now {state}")),
                None => Some(format!("{name} {state}")),
            }
        })
        .collect();
    if !checks.is_empty() {
        out.push_str(&untrusted_block(
            "checks",
            &format!("Checks: {}", checks.join(", ")),
        ));
    }

    (!out.is_empty()).then(|| format!("Since your last answer:\n{out}"))
}

/// The line that names the directory. Perch chose the path, so it is not
/// quoted; `now` says it replaces one named earlier in the session.
pub fn directory_line(dir: &Path, now: bool) -> String {
    format!(
        "The whole discussion {} in {}/, INDEX.md first.\n",
        if now { "is now" } else { "is" },
        dir.display()
    )
}

/// A comment as the prompt quotes it: cleaned, and cut at
/// [`MAX_COMMENT_CHARS`] with where the rest is.
fn cut(body: &str, key: EntryKey) -> String {
    let text = clean(body);
    let text = text.trim();
    if text.is_empty() {
        return "(no text)".into();
    }
    if text.chars().count() <= MAX_COMMENT_CHARS {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(MAX_COMMENT_CHARS)
        .map_or(text.len(), |(i, _)| i);
    let head = &text[..end];
    // Back up to the last whitespace, so no word is cut in half.
    let head = head
        .rfind(char::is_whitespace)
        .filter(|&i| i > end / 2)
        .map_or(head, |i| &head[..i]);
    format!("{}\n(continues in {})", head.trim_end(), key.file())
}

/// Text for a block's label, which is on the marker line: one line, and no
/// bracket that could end the marker early.
fn label(text: &str) -> String {
    one_line(&strip_format_chars(text)).replace(['[', ']'], "")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the runs' directories live: `$PERCH_HOME/context`.
pub fn root(data_root: &Path) -> PathBuf {
    data_root.join("context")
}

/// Write the discussion's files into `dir`, which must not exist. The
/// error is for a notice: a run goes ahead without its directory.
pub fn write_files(archive: &DiscussionArchive, dir: &Path) -> Result<(), String> {
    files::write(dir, &files::render(archive))
        .map_err(|e| format!("Could not write the discussion to {}: {e}", dir.display()))
}

/// A first question's section, the line naming the directory after it,
/// and the manifest of what the run was shown.
pub fn for_first(
    archive: &DiscussionArchive,
    anchor: Option<&Anchor>,
    asked_in: Option<u64>,
    dir: Option<&Path>,
) -> (String, Manifest) {
    let mut text = select(archive, anchor, asked_in, MAX_DISCUSSION_CHARS).text;
    if let Some(dir) = dir {
        text.push_str(&directory_line(dir, false));
    }
    let mut manifest = Manifest::of(archive);
    manifest.spent = text.len();
    (text, manifest)
}

/// A resumed follow-up's news, `None` when nothing changed, and the
/// manifest this run leaves for the next: what it has seen now, with what
/// the session has spent.
pub fn for_resumed(
    archive: &DiscussionArchive,
    previous: &Manifest,
    anchor: Option<&Anchor>,
    asked_in: Option<u64>,
    dir: Option<&Path>,
) -> (Option<String>, Manifest) {
    let text = since(archive, previous, anchor, asked_in).map(|mut text| {
        if let Some(dir) = dir {
            text.push_str(&directory_line(dir, true));
        }
        text
    });
    let mut manifest = Manifest::of(archive);
    manifest.spent = previous.spent + text.as_ref().map_or(0, String::len);
    (text, manifest)
}

/// Remove the directories under `root` that no run points at any more
/// and that are older than `grace`: a directory is written a moment before
/// its run records it, and `perch --ask` writes one no run ever records.
/// Anything that is not a directory is left alone.
pub fn sweep(root: &Path, keep: &std::collections::HashSet<PathBuf>, grace: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() || keep.contains(&path) {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age >= grace);
        if old {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                tracing::warn!(path = %path.display(), %error, "could not remove a discussion directory");
            }
        }
    }
}

/// Remove directories that runs pointed at before and no longer do, as
/// deleting a thread, its private messages or an item leaves them. Only
/// directories under `root` are touched, whatever a row says.
pub fn drop_unreferenced(
    root: &Path,
    before: &std::collections::HashSet<PathBuf>,
    after: &std::collections::HashSet<PathBuf>,
) {
    for dir in before.difference(after) {
        if dir.starts_with(root) && dir != root && dir.is_dir() {
            if let Err(error) = std::fs::remove_dir_all(dir) {
                tracing::warn!(path = %dir.display(), %error, "could not remove a discussion directory");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use perch_github::Side;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn post(id: u64, author: &str, body: &str, when: &str) -> Post {
        Post {
            id,
            author: author.into(),
            body: body.into(),
            created_at: at(when),
        }
    }

    fn thread(root: u64, path: &str, lines: (u32, u32), resolved: bool, when: &str) -> ThreadEntry {
        ThreadEntry {
            root_id: root,
            path: path.into(),
            start_line: lines.0,
            line: lines.1,
            side: "right".into(),
            resolved,
            outdated: false,
            diff_hunk: String::new(),
            messages: vec![post(
                root,
                "alice",
                &format!("About {path}:{}", lines.1),
                when,
            )],
        }
    }

    fn anchor(path: &str, start: u32, line: u32) -> Anchor {
        Anchor {
            path: path.into(),
            start_line: start,
            line,
            side: Side::Right,
            commit_sha: "h".into(),
        }
    }

    fn discussion() -> DiscussionArchive {
        DiscussionArchive {
            description: None,
            reviews: vec![
                ReviewEntry {
                    post: post(90, "alice", "Please batch.", "2026-09-15T10:00:00Z"),
                    verdict: Verdict::ChangesRequested,
                },
                ReviewEntry {
                    post: post(91, "alice", "", "2026-09-16T10:00:00Z"),
                    verdict: Verdict::Commented,
                },
                ReviewEntry {
                    post: post(92, "bob", "", "2026-09-15T11:00:00Z"),
                    verdict: Verdict::Approved,
                },
                ReviewEntry {
                    post: post(93, "Copilot", "Looks fine.", "2026-09-15T12:00:00Z"),
                    verdict: Verdict::Commented,
                },
            ],
            comments: vec![
                post(500, "carol", "Rebased.", "2026-09-16T09:00:00Z"),
                post(501, "codecov[bot]", "Coverage up.", "2026-09-16T09:05:00Z"),
                post(502, "dave", "Ship it.", "2026-09-17T09:00:00Z"),
            ],
            threads: vec![
                thread(10, "src/sync.rs", (190, 191), false, "2026-09-15T10:00:00Z"),
                thread(11, "src/sync.rs", (100, 100), false, "2026-09-15T10:01:00Z"),
                thread(12, "src/other.rs", (5, 5), false, "2026-09-15T10:02:00Z"),
                thread(13, "src/other.rs", (9, 9), true, "2026-09-15T10:03:00Z"),
                thread(14, "src/sync.rs", (230, 232), true, "2026-09-15T10:04:00Z"),
                thread(15, "src/sync.rs", (240, 240), false, "2026-09-15T10:05:00Z"),
            ],
            events: Vec::new(),
            checks: vec![
                CheckEntry {
                    name: "clippy".into(),
                    workflow: None,
                    outcome: CheckOutcome::Failing,
                    description: None,
                    url: None,
                },
                CheckEntry {
                    name: "test".into(),
                    workflow: None,
                    outcome: CheckOutcome::Passing,
                    description: None,
                    url: None,
                },
            ],
        }
    }

    fn review_comment(
        id: u64,
        reply_to: Option<u64>,
        user: &str,
        body: &str,
    ) -> perch_github::ReviewComment {
        perch_github::ReviewComment {
            id,
            node_id: String::new(),
            pull_request_review_id: Some(if user == "me" { 3 } else { 1 }),
            in_reply_to_id: reply_to,
            user: user.into(),
            user_avatar_url: String::new(),
            body: body.into(),
            path: "src/x.rs".into(),
            line: Some(41),
            original_line: None,
            start_line: Some(40),
            original_start_line: None,
            start_side: None,
            commit_id: "h".into(),
            original_commit_id: String::new(),
            diff_hunk: "@@ -1 +1 @@".into(),
            side: Side::Right,
            created_at: at(&format!("2026-09-16T11:{:02}:00Z", id % 60)),
            updated_at: None,
            html_url: String::new(),
            reactions: Default::default(),
        }
    }

    #[test]
    fn a_discussion_becomes_an_archive_without_the_reader_s_pending_review() {
        let review = |id: u64, user: &str, state: &str| perch_github::Review {
            id,
            node_id: String::new(),
            user: user.into(),
            body: String::new(),
            state: state.into(),
            submitted_at: (state != "PENDING").then(|| at("2026-09-16T12:00:00Z")),
            html_url: String::new(),
            reactions: Default::default(),
        };
        let discussion = Discussion {
            opened_at: Some(at("2026-09-14T00:00:00Z")),
            body: "Fence it.".into(),
            reviews: vec![
                review(1, "alice", "CHANGES_REQUESTED"),
                review(2, "bob", "COMMENTED"),
                review(3, "me", "PENDING"),
            ],
            review_comments: vec![
                review_comment(31, None, "alice", "Why?"),
                review_comment(32, Some(31), "rohan", "Because."),
                review_comment(33, None, "me", "Draft on GitHub"),
            ],
            events: vec![PullEvent::new(
                PullEventKind::Committed {
                    sha: "a1b2c3d4e5".into(),
                    author: "Rohan".into(),
                    login: String::new(),
                    subject: "Batch writes".into(),
                },
                at("2026-09-16T13:00:00Z"),
                "",
            )],
            ..Default::default()
        };
        let checks = Checks {
            state: perch_github::CheckState::Failing {
                workflow: "CI".into(),
            },
            url: None,
            runs: vec![perch_github::Check {
                name: "clippy".into(),
                workflow: Some("CI".into()),
                event: None,
                state: perch_github::CheckRunState::Failure,
                description: None,
                url: None,
                started_at: None,
                completed_at: None,
                kind: perch_github::CheckKind::CheckRun,
            }],
        };
        let archive = archive(&discussion, "item", "h", "rohan", Some(&checks));
        assert_eq!(archive.description.as_ref().unwrap().author, "rohan");
        assert_eq!(archive.reviews.len(), 1);
        assert_eq!(archive.reviews[0].verdict, Verdict::ChangesRequested);
        assert_eq!(
            archive.threads.len(),
            1,
            "the pending review's comment is left out"
        );
        let thread = &archive.threads[0];
        assert_eq!(
            (thread.root_id, thread.start_line, thread.line),
            (31, 40, 41)
        );
        assert_eq!(thread.side, "right");
        assert_eq!(thread.diff_hunk, "@@ -1 +1 @@");
        let said: Vec<(&str, &str)> = thread
            .messages
            .iter()
            .map(|m| (m.author.as_str(), m.body.as_str()))
            .collect();
        assert_eq!(said, [("alice", "Why?"), ("rohan", "Because.")]);
        assert_eq!(
            archive.events[0].text,
            "Rohan committed a1b2c3d Batch writes"
        );
        assert_eq!(archive.checks[0].outcome, CheckOutcome::Failing);
    }

    #[test]
    fn everything_fits_in_tier_order_with_no_index() {
        let archive = discussion();
        let sel = select(
            &archive,
            Some(&anchor("src/sync.rs", 210, 212)),
            None,
            MAX_DISCUSSION_CHARS,
        );
        assert_eq!(
            sel.shown,
            [
                EntryKey::Review(92),
                EntryKey::Review(90),
                // Near: 191 is 19 below 210, 230 is 18 above 212.
                EntryKey::Thread(14),
                EntryKey::Thread(10),
                // Unresolved, this file's first, newest first.
                EntryKey::Thread(15),
                EntryKey::Thread(11),
                EntryKey::Thread(12),
                EntryKey::Comment(502),
                EntryKey::Comment(500),
                EntryKey::Thread(13),
            ]
        );
        assert!(sel.omitted.is_empty());
        assert!(!sel.text.contains("Not shown above"));
        // Bots are not candidates at all.
        assert!(!sel.text.contains("codecov"));
        assert!(!sel.text.contains("Copilot"));
        // The comments read oldest first, after the threads; the checks
        // come after the reviews.
        let pos = |needle: &str| {
            sel.text
                .find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{}", sel.text))
        };
        assert!(
            pos("review by alice, changes requested")
                < pos("Checks: 1 failing (clippy), 1 passing")
        );
        assert!(pos("Checks:") < pos("thread by alice on src/sync.rs:230\u{2013}232, resolved"));
        assert!(pos("thread by alice on src/other.rs:9, resolved") < pos("comment by carol"));
        assert!(pos("comment by carol") < pos("comment by dave"));
    }

    #[test]
    fn the_near_window_is_twenty_lines_either_side_on_the_same_path_and_side() {
        let a = anchor("src/sync.rs", 210, 212);
        let when = "2026-09-15T10:00:00Z";
        let left = ThreadEntry {
            side: "left".into(),
            ..thread(7, "src/sync.rs", (211, 211), false, when)
        };
        for (t, expected) in [
            (thread(1, "src/sync.rs", (189, 190), false, when), true),
            (thread(2, "src/sync.rs", (188, 189), false, when), false),
            (thread(3, "src/sync.rs", (232, 240), false, when), true),
            (thread(4, "src/sync.rs", (233, 240), false, when), false),
            (thread(5, "src/sync.rs", (100, 300), false, when), true),
            (thread(6, "src/other.rs", (211, 211), false, when), false),
            (left, false),
        ] {
            let archive = DiscussionArchive {
                threads: vec![t],
                ..Default::default()
            };
            let c = candidates(&archive, Some(&a), None, |_| true);
            assert_eq!(c[0].rank.0 == 3, expected, "{:?}", c[0].key);
        }
    }

    #[test]
    fn the_thread_asked_in_is_its_history_and_not_repeated() {
        let archive = discussion();
        let sel = select(
            &archive,
            Some(&anchor("src/sync.rs", 190, 190)),
            Some(10),
            MAX_DISCUSSION_CHARS,
        );
        assert!(!sel.shown.contains(&EntryKey::Thread(10)));
        assert!(!sel.omitted.contains(&EntryKey::Thread(10)));
    }

    #[test]
    fn a_long_comment_is_cut_with_where_the_rest_is() {
        let long = "word ".repeat(1_000);
        let archive = DiscussionArchive {
            comments: vec![post(7, "carol", &long, "2026-09-16T09:00:00Z")],
            ..Default::default()
        };
        let sel = select(&archive, None, None, MAX_DISCUSSION_CHARS);
        assert!(sel
            .text
            .contains("word word\n(continues in conversation.md#c7)"));
        assert!(sel.text.len() < 2_300, "{}", sel.text.len());
    }

    #[test]
    fn what_does_not_fit_is_indexed_and_the_budget_holds_to_the_character() {
        let mut archive = DiscussionArchive::default();
        for i in 0..200u64 {
            archive.comments.push(post(
                1_000 + i,
                "carol",
                &format!("Comment {i}: {}", "blah ".repeat(60)),
                &format!("2026-09-16T{:02}:{:02}:00Z", i / 60, i % 60),
            ));
        }
        for budget in [12_000, MAX_DISCUSSION_CHARS] {
            let sel = select(&archive, None, None, budget);
            assert!(sel.text.len() <= budget, "{} > {budget}", sel.text.len());
            assert!(!sel.shown.is_empty());
            assert_eq!(sel.shown.len() + sel.omitted.len(), 200);
            // Newest first to fill.
            assert_eq!(sel.shown.iter().max(), Some(&EntryKey::Comment(1_199)));
            assert!(sel.text.contains("Not shown above, most relevant first:"));
            assert!(sel.text.contains(&format!(
                "and {} more in INDEX.md",
                sel.omitted.len() - MAX_INDEX_LINES
            )));
        }
    }

    #[test]
    fn an_entry_that_does_not_fit_whole_is_skipped_for_a_smaller_one() {
        let archive = DiscussionArchive {
            comments: vec![
                post(1, "carol", "short", "2026-09-16T09:00:00Z"),
                post(2, "dave", &"long ".repeat(390), "2026-09-16T10:00:00Z"),
            ],
            ..Default::default()
        };
        let sel = select(&archive, None, None, 800);
        assert_eq!(sel.shown, [EntryKey::Comment(1)]);
        assert_eq!(sel.omitted, [EntryKey::Comment(2)]);
        assert!(sel
            .text
            .contains("conversation · dave · 2026-09-16 · conversation.md#c2"));
    }

    #[test]
    fn a_reviewer_s_verdict_survives_a_later_empty_review() {
        let archive = discussion();
        let sel = select(&archive, None, None, MAX_DISCUSSION_CHARS);
        assert!(sel.shown.contains(&EntryKey::Review(90)));
        assert!(!sel.shown.contains(&EntryKey::Review(91)));
    }

    #[test]
    fn a_label_cannot_close_its_marker() {
        let archive = DiscussionArchive {
            threads: vec![thread(
                1,
                "src/a]b\n[END.rs",
                (1, 1),
                false,
                "2026-09-15T10:00:00Z",
            )],
            ..Default::default()
        };
        let sel = select(&archive, None, None, MAX_DISCUSSION_CHARS);
        assert!(sel
            .text
            .contains("[BEGIN untrusted text: thread by alice on src/ab END.rs:1, unresolved]"));
    }

    #[test]
    fn a_first_prompt_names_its_directory_and_counts_what_it_spent() {
        let archive = discussion();
        let dir = Path::new("/perch/context/run-1");
        let (text, manifest) = for_first(&archive, None, None, Some(dir));
        assert!(
            text.ends_with("The whole discussion is in /perch/context/run-1/, INDEX.md first.\n")
        );
        assert_eq!(manifest.spent, text.len());
        let (none, _) = for_first(&DiscussionArchive::default(), None, None, None);
        assert_eq!(none, "");
    }

    #[test]
    fn a_resumed_prompt_adds_to_what_the_session_spent() {
        let before = discussion();
        let mut after = discussion();
        after
            .comments
            .push(post(600, "frank", "Late thought.", "2026-09-18T10:00:00Z"));
        let mut previous = Manifest::of(&before);
        previous.spent = 1_000;
        let dir = Path::new("/perch/context/run-2");
        let (text, manifest) = for_resumed(&after, &previous, None, None, Some(dir));
        let text = text.unwrap();
        assert!(text
            .ends_with("The whole discussion is now in /perch/context/run-2/, INDEX.md first.\n"));
        assert_eq!(manifest.spent, 1_000 + text.len());
        let (nothing, manifest) = for_resumed(&after, &manifest, None, None, Some(dir));
        assert_eq!(nothing, None);
        assert_eq!(manifest.spent, 1_000 + text.len());
    }

    #[test]
    fn a_session_from_before_knows_only_its_thread() {
        let archive = discussion();
        let previous = Manifest::of_replies(&archive, &[10]);
        let text = since(&archive, &previous, None, Some(10)).unwrap();
        assert!(!text.contains("reply by"));
        assert!(text.contains("review by alice, changes requested"));
        assert!(text.contains("Checks: clippy failing, test passing"));
    }

    #[test]
    fn the_sweep_keeps_what_runs_point_at_and_what_is_new() {
        let root = tempfile::tempdir().unwrap();
        let kept = root.path().join("kept");
        let orphan = root.path().join("orphan");
        for dir in [&kept, &orphan] {
            std::fs::create_dir_all(dir.join("threads")).unwrap();
        }
        std::fs::write(root.path().join("stray.txt"), "x").unwrap();
        let keep = [kept.clone()].into_iter().collect();
        sweep(root.path(), &keep, std::time::Duration::from_secs(3600));
        assert!(orphan.exists(), "too new to sweep");
        sweep(root.path(), &keep, std::time::Duration::ZERO);
        assert!(kept.exists());
        assert!(!orphan.exists());
        assert!(root.path().join("stray.txt").exists());

        let outside = tempfile::tempdir().unwrap();
        let before = [kept.clone(), outside.path().to_path_buf()]
            .into_iter()
            .collect();
        drop_unreferenced(root.path(), &before, &Default::default());
        assert!(!kept.exists());
        assert!(outside.path().exists(), "never outside the root");
    }

    fn resumed(
        before: &DiscussionArchive,
        after: &DiscussionArchive,
        asked_in: Option<u64>,
    ) -> Option<String> {
        since(after, &Manifest::of(before), None, asked_in)
    }

    #[test]
    fn nothing_changed_says_nothing() {
        assert_eq!(resumed(&discussion(), &discussion(), Some(10)), None);
    }

    #[test]
    fn a_reply_in_the_thread_is_sent_whole() {
        let before = discussion();
        let mut after = discussion();
        let long = format!("Fair, but {}", "really ".repeat(500));
        after.threads[0]
            .messages
            .push(post(77, "alice", &long, "2026-09-18T09:00:00Z"));
        after.threads[0]
            .messages
            .push(post(78, "dependabot[bot]", "bump", "2026-09-18T09:01:00Z"));
        let text = resumed(&before, &after, Some(10)).unwrap();
        assert!(text.starts_with("Since your last answer:\n[BEGIN untrusted text: reply by alice in this thread]\nFair, but"));
        assert!(text.contains(long.trim()));
        assert!(!text.contains("bump"));
        assert!(!text.contains("New or changed elsewhere"));
    }

    #[test]
    fn a_new_review_an_edit_and_a_check_are_each_said() {
        let before = discussion();
        let mut after = discussion();
        after.reviews.push(ReviewEntry {
            post: post(95, "erin", "Needs tests.", "2026-09-18T10:00:00Z"),
            verdict: Verdict::ChangesRequested,
        });
        after.comments[0].body = "Rebased again.".into();
        after.checks[0].outcome = CheckOutcome::Passing;
        let text = resumed(&before, &after, Some(10)).unwrap();
        assert!(
            text.contains(
                "[BEGIN untrusted text: review by erin, changes requested]\nNeeds tests."
            ),
            "{text}"
        );
        assert!(
            text.contains("(edited)\n[BEGIN untrusted text: comment by carol]\nRebased again."),
            "{text}"
        );
        assert!(
            text.contains("New or changed elsewhere: 1 review, 1 comment.\n"),
            "{text}"
        );
        assert!(text.contains("Checks: clippy now passing"), "{text}");
        assert!(!text.contains("Please batch."), "{text}");
    }

    #[test]
    fn what_was_spent_before_counts_against_the_budget() {
        let before = discussion();
        let mut after = discussion();
        after
            .comments
            .push(post(600, "frank", "Late thought.", "2026-09-18T10:00:00Z"));
        let mut manifest = Manifest::of(&before);
        manifest.spent = MAX_DISCUSSION_CHARS;
        let text = since(&after, &manifest, None, None).unwrap();
        assert!(!text.contains("Late thought."));
        assert!(text.contains("New or changed elsewhere: 1 comment."));
    }
}
