//! What the review view holds about a pull request and its files.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::Cursor;
use floem::views::editor::text::Document;
use perch_git::{
    ChangeStatus, ChangedFile, Commit, Dirt, DisplayBuffer, LineStatus, Row, Scope, TreeEntry,
};
use perch_github::{
    Anchor, Checks, CommentThread, Discussion, Issue, IssueComment, Item, PullRequest,
    PullRequestOverview, PullReviewEvent, ReactionKind, Reactions, Review,
};
use perch_syntax::{Language, LineSpans};

use crate::review::doc::{self, Card, CardId, Doc, Reveals};
use crate::review::picture::Picture;

/// Where a file is from: the worktree, which is what the review is of, or
/// outside it, a dependency or the standard library reached from a
/// definition, which is a place to read and no part of the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Worktree,
    Outside {
        /// What the tab's chip says: `tokio 1.40.0`, `std`, `git: floem`.
        label: String,
    },
}

impl Origin {
    /// The origin a path says: a worktree file's path is relative to the
    /// worktree, and an outside file's is absolute. The two cannot
    /// collide, which is why `files` and the tabs are keyed by the path
    /// alone.
    pub fn of_path(path: &str) -> Origin {
        let path = std::path::Path::new(path);
        if path.is_absolute() {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            Origin::Outside {
                label: outside_label(path, home.as_deref()),
            }
        } else {
            Origin::Worktree
        }
    }

    pub fn is_outside(&self) -> bool {
        matches!(self, Origin::Outside { .. })
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Origin::Worktree => None,
            Origin::Outside { label } => Some(label),
        }
    }
}

/// A file prepared for the editor: text to render, and everything needed to
/// colour it and map a rendered line back to a real one.
pub struct LoadedFile {
    pub path: String,
    pub origin: Origin,
    pub language: Option<Language>,
    /// Head text with the removed lines spliced in.
    pub buffer: DisplayBuffer,
    /// Highlight spans for the head file, by head line.
    pub head_spans: LineSpans,
    /// Highlight spans for the base file, by base line. Used for ghost lines.
    pub base_spans: LineSpans,
    /// `None` when the file is not part of the pull request.
    pub status: Option<ChangeStatus>,
    /// True when the file was too large or not valid text.
    pub truncated: bool,
    /// The scope the two sides came from. A file loaded under another
    /// scope is stale, and reloads rather than repaints.
    pub scope: Scope,
    /// The base side on its own, for the left of a split. `None` when the
    /// file is not in the changed list, or was added.
    pub base_buffer: Option<DisplayBuffer>,
    /// Which base line sits beside which head line, both 1-based, for a
    /// split: `(Some, Some)` is an unchanged or modified pair, `(Some,
    /// None)` a removed line, `(None, Some)` an added one. Empty for a
    /// file outside the changed list.
    pub alignment: Vec<(Option<usize>, Option<usize>)>,
    /// The head blob, which the reveals are keyed by, as Viewed is: a new
    /// blob is new content and starts folded again. Empty for a file that
    /// has none: one outside the changed list, a deletion, or a working
    /// tree file git has not hashed. Such a file's reveals are kept for
    /// the window's life and never stored.
    pub blob: String,
    /// The file as it draws, when it is an SVG: its sides parsed into the
    /// trees the renderer takes, shown in place of the markup unless the
    /// reader asks for the markup. `None` for every other file.
    pub picture: Option<Picture>,
}

impl std::fmt::Debug for LoadedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The buffer can be megabytes; a log line wants the shape, not the text.
        f.debug_struct("LoadedFile")
            .field("path", &self.path)
            .field("language", &self.language.map(|l| l.name()))
            .field("lines", &self.buffer.len())
            .field("status", &self.status)
            .field("picture", &self.picture.is_some())
            .finish()
    }
}

impl LoadedFile {
    pub fn line_count(&self) -> usize {
        self.buffer.len()
    }

    pub fn change_count(&self) -> usize {
        self.buffer.change_starts.len()
    }
}

/// The item view's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemTab {
    #[default]
    Conversation,
    Files,
}

/// How Files draws a changed file: head with the removed lines spliced in,
/// or base and head side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Stacked,
    Split,
}

impl Layout {
    /// The word `meta` stores.
    pub fn name(self) -> &'static str {
        match self {
            Layout::Stacked => "stacked",
            Layout::Split => "split",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "stacked" => Some(Layout::Stacked),
            "split" => Some(Layout::Split),
            _ => None,
        }
    }
}

/// An identifier at a head position: what the pointer is over, and what
/// the caret is on. `column` is where it starts on the line and `end`
/// where it ends, both byte columns as the protocol counts them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Symbol {
    pub path: String,
    /// 0-based head line.
    pub line: usize,
    pub column: usize,
    pub end: usize,
    pub text: String,
}

/// What rust-analyzer said about a symbol, kept beside the symbol it
/// answers. `markdown` is empty from the ask until the answer lands, and
/// the tooltip draws it only while `symbol` is the pointer's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverAnswer {
    pub symbol: Symbol,
    pub markdown: String,
    /// The request's token, which the answer is matched against.
    pub token: u64,
}

/// One place rust-analyzer named: a use of a symbol, a definition, an
/// impl. `path` is relative to the worktree for a file in it and absolute
/// for one outside, in a dependency or the standard library, which then
/// carries the label its tab's chip shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Located {
    pub path: String,
    /// The chip's label when `path` is outside the worktree.
    pub outside: Option<String>,
    /// 1-based.
    pub line: usize,
    /// 0-based column of the symbol on that line.
    pub character: usize,
    /// The line's text, trimmed of its indent.
    pub text: String,
}

/// What a list of locations is a list of: the panel's heading and its
/// summary read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locations {
    References,
    Implementations,
    Definitions,
}

impl Locations {
    /// "References to", "Implementations of", "Definitions of": what the
    /// heading says before the symbol.
    pub fn heading(self) -> &'static str {
        match self {
            Locations::References => "References to",
            Locations::Implementations => "Implementations of",
            Locations::Definitions => "Definitions of",
        }
    }
}

/// The answer to a question with a list for an answer: what was asked
/// about and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct References {
    pub kind: Locations,
    pub symbol: String,
    /// Every location, those outside the worktree among them.
    pub locations: Vec<Located>,
    /// True until the answer lands, so an empty answer can say so rather
    /// than look like one still on its way.
    pub pending: bool,
    /// The request's token. The answer is matched against this, not the
    /// shared counter, which every hover moves on.
    pub token: u64,
}

/// What the chip on an outside file's tab says about where the file is
/// from, read off its path. A crate under cargo's registry is its name and
/// version, `tokio 1.40.0`; a checkout under cargo's git directory is
/// `git:` and the repository, `git: floem`; a file of the toolchain's
/// `rust-src` is its library crate, `std`, `core` or `alloc`. Anywhere
/// else under the home directory reads as its path relative to the home
/// directory, and anywhere else as the path.
pub fn outside_label(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let after = |marker: &[&str]| -> Option<&str> {
        let at = parts.windows(marker.len()).position(|w| w == marker)?;
        parts.get(at + marker.len()).copied()
    };

    // ~/.cargo/registry/src/<index>/<name>-<version>/...
    if let Some(at) = parts.windows(3).position(|w| w == [".cargo", "registry", "src"]) {
        if let Some(dir) = parts.get(at + 4) {
            return match crate_name_and_version(dir) {
                Some((name, version)) => format!("{name} {version}"),
                None => dir.to_string(),
            };
        }
    }
    // ~/.cargo/git/checkouts/<repo>-<hash>/<rev>/...
    if let Some(dir) = after(&[".cargo", "git", "checkouts"]) {
        let repo = dir.rsplit_once('-').map(|(repo, _)| repo).unwrap_or(dir);
        return format!("git: {repo}");
    }
    // <toolchain>/lib/rustlib/src/rust/library/<crate>/...
    if let Some(krate) = after(&["lib", "rustlib", "src", "rust", "library"]) {
        return krate.to_string();
    }
    if let Some(relative) = home.and_then(|home| path.strip_prefix(home).ok()) {
        return relative.display().to_string();
    }
    path.display().to_string()
}

/// `tokio-util-0.7.12` as `tokio-util` and `0.7.12`: the version starts
/// at the last dash a digit follows.
fn crate_name_and_version(dir: &str) -> Option<(&str, &str)> {
    let at = dir
        .char_indices()
        .filter(|(i, c)| *c == '-' && dir[i + 1..].starts_with(|c: char| c.is_ascii_digit()))
        .map(|(i, _)| i)
        .next_back()?;
    Some((&dir[..at], &dir[at + 1..]))
}

/// Which way the call hierarchy reads: who calls the function, or what it
/// calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Callers,
    Callees,
}

impl Direction {
    /// "Callers of", "Callees of": what the heading says before the symbol.
    pub fn heading(self) -> &'static str {
        match self {
            Direction::Callers => "Callers of",
            Direction::Callees => "Callees of",
        }
    }

    /// "No callers.", "No callees.": the row under a function nothing
    /// calls, or that calls nothing.
    pub fn none(self) -> &'static str {
        match self {
            Direction::Callers => "No callers.",
            Direction::Callees => "No callees.",
        }
    }
}

/// One function the server named in the call hierarchy: a caller or a
/// callee of the row above it, or the root. The item goes back to the
/// server as it came when the row is expanded.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub item: perch_lsp::lsp_types::CallHierarchyItem,
    /// Where the call is made: the file and line of the call, with the
    /// line's text. In the caller's file for a caller, and in the file of
    /// the function above for a callee.
    pub site: Located,
}

/// One row of the call hierarchy panel's tree, which is flat with depths
/// the way the sidebar's lists are, so `dyn_stack` draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct HierarchyRow {
    pub item: perch_lsp::lsp_types::CallHierarchyItem,
    pub depth: usize,
    /// The call sites: none for the root, whose place is its definition.
    pub site: Option<Located>,
    /// Which name the call's line emphasises: the callee's.
    pub emphasis: String,
    pub expanded: bool,
    /// The token of the ask for this row's children, while one is out.
    pub pending: Option<u64>,
    /// True when the same function is open above this row: a recursive
    /// call, which expands to nothing.
    pub repeats: bool,
}

impl HierarchyRow {
    /// Whether two items are the same function: the server's items carry
    /// no identity but where they are.
    fn same(a: &perch_lsp::lsp_types::CallHierarchyItem, b: &perch_lsp::lsp_types::CallHierarchyItem) -> bool {
        a.uri == b.uri && a.selection_range == b.selection_range && a.name == b.name
    }
}

/// The call hierarchy panel's content: a root function and the tree under
/// it, one way or the other.
#[derive(Debug, Clone, PartialEq)]
pub struct Hierarchy {
    pub root: perch_lsp::lsp_types::CallHierarchyItem,
    /// Where the root is, for its row.
    pub root_place: Located,
    pub direction: Direction,
    pub rows: Vec<HierarchyRow>,
}

impl Hierarchy {
    /// A fresh tree of the root alone, with its children on their way
    /// under `token`.
    pub fn new(
        root: perch_lsp::lsp_types::CallHierarchyItem,
        root_place: Located,
        direction: Direction,
        token: u64,
    ) -> Self {
        let rows = vec![HierarchyRow {
            item: root.clone(),
            depth: 0,
            site: None,
            emphasis: root.name.clone(),
            expanded: false,
            pending: Some(token),
            repeats: false,
        }];
        Self {
            root,
            root_place,
            direction,
            rows,
        }
    }

    /// The children asked for under `token` arrived: they go under the row
    /// that asked, one deeper, each marked when it repeats a function open
    /// above it. `None` when no row was waiting on the token.
    pub fn fill(&mut self, token: u64, calls: Vec<Call>) -> Option<usize> {
        let at = self.rows.iter().position(|r| r.pending == Some(token))?;
        let depth = self.rows[at].depth;
        let ancestors: Vec<perch_lsp::lsp_types::CallHierarchyItem> = self.ancestors(at);
        let parent_name = self.rows[at].item.name.clone();
        let direction = self.direction;
        let children: Vec<HierarchyRow> = calls
            .into_iter()
            .map(|call| {
                let repeats = ancestors.iter().any(|a| HierarchyRow::same(a, &call.item));
                // The call's line emphasises the callee: the function above
                // for a caller, the row's own for a callee.
                let emphasis = match direction {
                    Direction::Callers => parent_name.clone(),
                    Direction::Callees => call.item.name.clone(),
                };
                HierarchyRow {
                    item: call.item,
                    depth: depth + 1,
                    site: Some(call.site),
                    emphasis,
                    expanded: false,
                    pending: None,
                    repeats,
                }
            })
            .collect();
        let row = &mut self.rows[at];
        row.pending = None;
        row.expanded = true;
        let count = children.len();
        self.rows.splice(at + 1..at + 1, children);
        Some(count)
    }

    /// The row and every ancestor of it, root first.
    fn ancestors(&self, at: usize) -> Vec<perch_lsp::lsp_types::CallHierarchyItem> {
        let mut out = Vec::new();
        let mut depth = self.rows[at].depth;
        out.push(self.rows[at].item.clone());
        for row in self.rows[..at].iter().rev() {
            if row.depth < depth {
                out.push(row.item.clone());
                depth = row.depth;
            }
        }
        out.reverse();
        out
    }

    /// Fold a row: its subtree goes. Nothing for one that is not open.
    pub fn collapse(&mut self, at: usize) {
        let Some(row) = self.rows.get_mut(at) else { return };
        if !row.expanded {
            return;
        }
        row.expanded = false;
        let depth = row.depth;
        let end = self.rows[at + 1..]
            .iter()
            .position(|r| r.depth <= depth)
            .map(|n| at + 1 + n)
            .unwrap_or(self.rows.len());
        self.rows.drain(at + 1..end);
    }

    /// Mark a row's children as asked for under `token`.
    pub fn expanding(&mut self, at: usize, token: u64) {
        if let Some(row) = self.rows.get_mut(at) {
            row.pending = Some(token);
        }
    }

    /// The same root, the other way: the tree starts again.
    pub fn turned(&self, direction: Direction, token: u64) -> Self {
        Self::new(self.root.clone(), self.root_place.clone(), direction, token)
    }
}

/// A call hierarchy asked for and not yet prepared: the panel opens when
/// the server names a function, and does not when it names nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HierarchyAsk {
    pub token: u64,
    pub symbol: String,
}

/// A goto in flight: a definition, a type definition or an implementation
/// asked for and not yet answered. One answer opens; several list under
/// `kind`; none does nothing visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoAsk {
    pub token: u64,
    pub symbol: String,
    pub kind: Locations,
}

/// Somewhere the reader was: a file and a head line, for the jump keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    pub line: usize,
}

/// How many places the jump keys remember.
pub const HISTORY_CAP: usize = 50;

/// Where the jumps came from and went. A jump pushes the place it left
/// onto `back` and clears `forward`; back pops one and pushes the current
/// place onto `forward`, and forward the other way. The oldest of `back`
/// goes when there are more than the cap.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct History {
    back: Vec<Place>,
    forward: Vec<Place>,
}

impl History {
    /// A jump is about to leave `from`.
    pub fn push(&mut self, from: Place) {
        // Jumping from where the last jump left is one place, not two.
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        if self.back.len() > HISTORY_CAP {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// The place before this one, with `current` kept for forward.
    pub fn back(&mut self, current: Place) -> Option<Place> {
        let place = self.back.pop()?;
        self.forward.push(current);
        Some(place)
    }

    /// The place after this one, with `current` kept for back.
    pub fn forward(&mut self, current: Place) -> Option<Place> {
        let place = self.forward.pop()?;
        self.back.push(current);
        Some(place)
    }

}

/// What the review view has on screen. One signal, written once per
/// event after every field it depends on, so the view rebuilds when the
/// shape changes and not once per field that changes on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Failed,
    /// A checkout and its files, with or without a pull request.
    Ready,
    /// An issue, which has no files.
    Issue,
}

/// How far along preparing a checkout is.
#[derive(Debug, Clone, PartialEq)]
pub enum Preparing {
    Idle,
    Working {
        message: String,
        percent: Option<u8>,
    },
    Failed {
        message: String,
    },
}

/// How a question runs: which agent, which of its models, and how hard
/// it thinks.
pub type AgentChoice = (perch_agent::Backend, String, perch_agent::Effort);

/// Where what is typed in the composer goes: to an agent, which runs
/// and answers, or to GitHub, where it waits as a draft until the review
/// is submitted.
#[derive(Debug, Clone, PartialEq)]
pub enum Destination {
    Agent {
        backend: perch_agent::Backend,
        model: String,
        effort: perch_agent::Effort,
    },
    GitHub,
}

impl Destination {
    pub fn agent(choice: AgentChoice) -> Self {
        let (backend, model, effort) = choice;
        Destination::Agent {
            backend,
            model,
            effort,
        }
    }

    /// The agent choice, when the destination is one.
    pub fn agent_choice(&self) -> Option<AgentChoice> {
        match self {
            Destination::Agent {
                backend,
                model,
                effort,
            } => Some((*backend, model.clone(), *effort)),
            Destination::GitHub => None,
        }
    }

    pub fn is_github(&self) -> bool {
        matches!(self, Destination::GitHub)
    }
}

/// A message to GitHub that has not been posted: what the review form
/// lists, the button counts and the gutter marks. `in_reply_to` is the
/// thread's root comment on GitHub, when it has one; `reply` says the
/// draft posts as a reply, under that root or under the draft that opens
/// its thread; `commit_sha` is the anchor's, the head the lines were read
/// at, which only a draft that opens a thread posts against.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub thread_id: String,
    pub message_id: String,
    pub anchor: Option<Anchor>,
    pub in_reply_to: Option<u64>,
    pub reply: bool,
    pub body: String,
    pub commit_sha: String,
}

/// An issue comment the backend is posting: the text, so the composer
/// can be shown still while it goes; the token the answer names, so an
/// answer to an older send is not taken for this one's; and which
/// opening of the composer sent it, so the answer settles that composer
/// and not one opened since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub token: u64,
    pub opening: u64,
    pub body: String,
}

/// Where a composer opens on, by the rules of the follow-up. Editing a
/// draft is GitHub. A follow-up follows the last message in its thread:
/// under a GitHub user, or under a draft or a posted comment of the
/// reader's, GitHub; under an agent, that agent at the effort its run
/// took, or the nearest the model takes now, and the last agent used
/// when config no longer offers the model at all; under a question of
/// the reader's still with an agent, whatever agent was last used. A new
/// composer on lines is what the reader last sent from that face, else
/// the last agent; one about the item as a whole is always an agent.
pub fn default_destination(
    thread: Option<&CommentThread>,
    anchor: Option<&Anchor>,
    last: Option<Destination>,
    agent_last: Option<AgentChoice>,
    defaults: AgentChoice,
    editing: bool,
    agent: &perch_github::AgentConfig,
) -> Destination {
    use perch_github::{Author, ThreadOrigin};

    let last_agent = || Destination::agent(agent_last.clone().unwrap_or(defaults.clone()));
    if editing {
        return Destination::GitHub;
    }
    if let Some(thread) = thread {
        let Some(message) = thread.messages.last() else {
            return last_agent();
        };
        return match (&message.author, message.origin) {
            (Author::GitHubUser(_), _) => Destination::GitHub,
            (Author::User, ThreadOrigin::GitHub) => Destination::GitHub,
            (Author::Agent { backend, model }, _) => {
                let offered = agent
                    .models
                    .iter()
                    .chain(agent.codex_models())
                    .any(|m| m == model);
                let Some(backend) = perch_agent::Backend::parse(backend).filter(|_| offered)
                else {
                    return last_agent();
                };
                let effort = message
                    .run
                    .as_ref()
                    .and_then(|r| perch_agent::Effort::parse(&r.effort))
                    .unwrap_or(defaults.2);
                Destination::Agent {
                    backend,
                    model: model.clone(),
                    effort: crate::review::threads::nearest_effort(agent, model, effort),
                }
            }
            (Author::User, ThreadOrigin::Local) => last_agent(),
        };
    }
    match (anchor, last) {
        (Some(_), Some(last)) => last,
        _ => last_agent(),
    }
}

/// The drafts among `threads`, each with its thread's anchor and root
/// comment on GitHub.
pub fn drafts_of(threads: &[CommentThread]) -> Vec<Draft> {
    threads
        .iter()
        .flat_map(|t| {
            t.messages
                .iter()
                .filter(|m| m.is_draft())
                .enumerate()
                .map(move |(nth, m)| Draft {
                    thread_id: t.id.clone(),
                    message_id: m.id.clone(),
                    anchor: t.anchor.clone(),
                    in_reply_to: t.github_id,
                    reply: t.github_id.is_some() || nth > 0,
                    body: m.body.clone(),
                    commit_sha: t
                        .anchor
                        .as_ref()
                        .map(|a| a.commit_sha.clone())
                        .unwrap_or_default(),
                })
        })
        .collect()
}

/// `3 comments`, `1 reply, 2 comments`, `1 comment`: what the review form
/// says of its drafts. A reply is a draft under a thread on GitHub, or
/// after the draft that opens its thread. Nothing for none.
pub fn drafts_summary(drafts: &[Draft]) -> String {
    let replies = drafts.iter().filter(|d| d.reply).count();
    let comments = drafts.len() - replies;
    let mut parts = Vec::new();
    if replies > 0 {
        parts.push(count_of(replies, "reply", "replies"));
    }
    if comments > 0 {
        parts.push(count_of(comments, "comment", "comments"));
    }
    parts.join(", ")
}

fn count_of(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What the review form says once the submission is done and not every
/// draft posted, from the counts the backend answered with: nothing when
/// they all did. `review` says a review posted; `posted` counts the
/// drafts confirmed on GitHub. `drafts` are the drafts still kept, which
/// say whether a failed one was a reply or a comment.
pub fn submit_review_outcome(
    review: bool,
    posted: usize,
    unmatched: &[String],
    failed: &[(String, String)],
    drafts: &[Draft],
) -> Option<String> {
    if unmatched.is_empty() && failed.is_empty() {
        return None;
    }
    let mut text = String::new();
    if review {
        text.push_str("Posted the review");
        if posted > 0 {
            text.push_str(&format!(" and {}", count_of(posted, "comment", "comments")));
        }
        text.push('.');
    } else if posted > 0 {
        text.push_str(&format!("Posted {}.", count_of(posted, "comment", "comments")));
    }
    if !failed.is_empty() {
        let is_reply = |id: &str| drafts.iter().any(|d| d.message_id == id && d.reply);
        let replies = failed.iter().filter(|(id, _)| is_reply(id)).count();
        let comments = failed.len() - replies;
        let mut parts = Vec::new();
        if replies > 0 {
            parts.push(count_of(replies, "reply", "replies"));
        }
        if comments > 0 {
            parts.push(count_of(comments, "comment", "comments"));
        }
        let error = failed[0].1.trim_end_matches('.');
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!("{} did not post: {error}.", parts.join(" and ")));
    }
    if !unmatched.is_empty() {
        let n = unmatched.len();
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!(
            "{} could not be matched to what GitHub returned and {} still {}.",
            count_of(n, "comment", "comments"),
            if n == 1 { "is" } else { "are" },
            if n == 1 { "a draft" } else { "drafts" },
        ));
    }
    Some(text)
}

/// One effective reviewer captured by a launch. A disabled (`None`) row is
/// omitted from `ReviewLaunchRequest::reviewers`; it is a UI preference, not
/// an effort understood by the native CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLaunchChoice {
    pub backend: perch_agent::Backend,
    pub model: String,
    pub effort: perch_agent::ReviewEffort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLaunchRequest {
    pub item_id: String,
    pub base_ref: String,
    pub reviewers: Vec<ReviewLaunchChoice>,
}

/// The launcher's editable, durable preference for one backend. An
/// unavailable backend can retain its model/effort while `enabled` is false
/// in the effective draft shown by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPreferenceChoice {
    pub backend: perch_agent::Backend,
    pub model: Option<String>,
    pub effort: Option<perch_agent::ReviewEffort>,
    pub enabled: bool,
}

/// Everything needed to draw one immutable review in Conversation/history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewGroupSummary {
    pub group: perch_github::ReviewGroup,
    pub snapshot: perch_github::ReviewSnapshot,
    pub attempts: Vec<perch_github::ReviewerAttempt>,
}

/// Durable finding state plus the compact provenance needed to identify it
/// in the ordinary conversation and Files thread surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFindingSummary {
    pub finding: perch_github::Finding,
    pub severity: perch_github::FindingSeverity,
    /// Every reviewer/model which has reported or verified this identity.
    pub attributions: Vec<(String, String)>,
    /// Review numbers containing an observation of this finding.
    pub seen_in_reviews: Vec<u64>,
    /// The latest observation's reviewer, retained for compact callers.
    pub backend: Option<String>,
    pub model: Option<String>,
    pub original_review_number: Option<u64>,
}

/// The last stable file context while one or more historical selections are
/// optimistic. A later click keeps this original baseline so failure of the
/// eventual winner can restore the view the reader actually came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoricalReviewRollback {
    pub navigation_id: Option<u64>,
    pub review: Option<ReviewGroupSummary>,
    pub request_id: Option<u64>,
    pub findings: Option<Arc<Vec<ReviewFindingSummary>>>,
}

impl ReviewFindingSummary {
    /// Resolved and explicitly linked findings stay in review history, but
    /// do not crowd the default list of actionable threads.
    pub fn is_actionable(&self) -> bool {
        self.finding.disposition == perch_github::FindingDisposition::Open
            && self.finding.verification != perch_github::FindingVerification::Fixed
            && self.finding.duplicate_of.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewFindingFilter {
    #[default]
    Open,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLaunchDraft {
    pub item_id: String,
    pub base_ref: Option<String>,
    pub choices: Vec<ReviewPreferenceChoice>,
}

/// A composer that is open: where the question will be anchored and how
/// it will be run.
#[derive(Debug, Clone, PartialEq)]
pub struct Composer {
    /// `None` starts a thread; `Some` is a follow-up in that thread.
    pub thread_id: Option<String>,
    pub anchor: Option<Anchor>,
    /// Where the text goes: an agent, its model and effort together,
    /// since the picker lists models and config says whose each is, or
    /// GitHub.
    pub destination: Destination,
    /// A question to start with, when a failed run is retried.
    pub draft: String,
    /// The draft message this composer replaces, when one is being
    /// edited in place; sending writes its body rather than a new one.
    pub editing: Option<String>,
    /// Which opening this is, so a view keyed on what the composer is
    /// about still starts afresh when it is opened again about the same
    /// thing with a new draft. In the composer rather than beside it so
    /// opening is one write.
    pub opening: u64,
}

/// One open editor tab. The origin is here as well as on the loaded
/// file, so the strip can draw the chip before the file has loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub path: String,
    pub origin: Origin,
}

/// One editor's document, live: the handle the editor lays out from, which
/// a programmatic edit goes through, and the row table over it, which
/// everything that reads the editor's rows goes through. The table is a
/// signal so that a lookup after an edit sees the edited document rather
/// than a snapshot taken when the editor was built. With them the
/// editor's cursor, for the find bar to select a match through, and its
/// content view, the one a click gives the keyboard to, for the bar to
/// give it back to.
///
/// All of it lives with the editor's view: the signals are disposed when
/// the editor is torn down, so a reader that can run after that, on
/// another tab, reads them with the `try_` variants.
#[derive(Clone)]
pub struct LiveDoc {
    pub handle: Rc<dyn Document>,
    pub doc: RwSignal<Arc<Doc>>,
    pub cursor: RwSignal<Cursor>,
    pub content: Option<floem::ViewId>,
}

/// The documents the open editors hold, for the epoch they were built in.
/// A reader checks the epoch against `editor_epoch` first: an entry from an
/// editor since rebuilt is stale.
#[derive(Clone)]
pub struct LiveDocs {
    pub epoch: u64,
    pub layout: LiveLayout,
}

/// Which editors are open: the stacked one, or a split's two.
#[derive(Clone)]
pub enum LiveLayout {
    Stacked(LiveDoc),
    Split {
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "the view edits the base side through its own handle; only the tests read it here"
            )
        )]
        left: LiveDoc,
        right: LiveDoc,
    },
}

impl LiveLayout {
    /// The document the folds are counted from: the stacked one, or the
    /// head side of a split, whose dividers the base side mirrors.
    pub fn head(&self) -> &LiveDoc {
        match self {
            LiveLayout::Stacked(live) => live,
            LiveLayout::Split { right, .. } => right,
        }
    }
}

/// A write to the store's reveals, waiting for the backend. The state has
/// no handle to it, so a reveal queues one of these and the root view,
/// which has the handle, sends it on, as it sends a setting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevealWrite {
    /// Head lines `first` to `last` of a file's content were opened.
    Reveal {
        path: String,
        blob: String,
        first: u32,
        last: u32,
    },
    /// Every reveal overlapping `first` to `last` is forgotten: a Reset.
    Reset {
        path: String,
        blob: String,
        first: u32,
        last: u32,
    },
}

/// Everything the review view renders from. All fields are signals, so the
/// struct is `Copy` and can be handed to any view closure.
#[derive(Clone, Copy)]
pub struct ReviewState {
    /// `None` until a pull request is open: an issue and a bare branch
    /// have none.
    pub pull_request: RwSignal<Option<Arc<PullRequest>>>,
    /// The item this review is reading: its checkout, and its pull request
    /// when it has one.
    pub item: RwSignal<Option<Arc<Item>>>,
    /// True once the checkout is ready, whether or not there is a pull
    /// request to show.
    pub ready: RwSignal<bool>,
    /// The issue, when the item is one; the view is the issue view then.
    pub issue: RwSignal<Option<Arc<Issue>>>,
    pub issue_comments: RwSignal<Arc<Vec<IssueComment>>>,
    /// The head the pull request moved to while the worktree had work in
    /// it. The files on screen are the older ones.
    pub stale_head: RwSignal<Option<String>>,
    /// The pull request could not be fetched; this is the checkout as it
    /// was last left.
    pub offline: RwSignal<bool>,
    /// True from the click until the review view is left, so the progress
    /// card can show before the pull request itself has resolved.
    pub opening: RwSignal<bool>,
    pub phase: RwSignal<Phase>,
    pub preparing: RwSignal<Preparing>,
    /// The header's review launcher and the immutable history selection are
    /// view-local; durable preferences and groups live on `AppState`.
    pub review_launcher_open: RwSignal<bool>,
    pub review_launcher_draft: RwSignal<Option<ReviewLaunchDraft>>,
    pub review_launcher_opener: RwSignal<Option<floem::ViewId>>,
    /// GitHub's review submission surface is separate from the native agent
    /// review launcher above. Its draft stays with the open pull request so a
    /// cancelled or failed submission can be resumed without retyping it.
    pub submit_review_open: RwSignal<bool>,
    pub submit_review_draft: RwSignal<String>,
    pub submit_review_event: RwSignal<PullReviewEvent>,
    pub submit_review_opener: RwSignal<Option<floem::ViewId>>,
    pub submit_review_focused: RwSignal<bool>,
    pub submit_review_autofocus: RwSignal<bool>,
    /// The backend-assigned attempt currently in flight, if any.
    pub submit_review_pending: RwSignal<Option<u64>>,
    pub submit_review_error: RwSignal<Option<String>>,
    /// What the last submission left undone, once it is over: a line the
    /// form keeps until the next submission or a close, through edits.
    pub submit_review_outcome: RwSignal<Option<String>>,
    /// Counts the submissions that came back with an outcome. The form's
    /// summary box is built anew on each, so a summary that posted is
    /// gone from it.
    pub submit_review_generation: RwSignal<u64>,
    /// Discard all was pressed and is being asked about.
    pub submit_review_confirm_discard: RwSignal<bool>,
    /// Reviews accepted by GitHub in this session. They are merged into
    /// later discussion snapshots so an older in-flight GET cannot briefly
    /// erase a review that the POST already confirmed.
    pub submitted_reviews: RwSignal<Arc<Vec<Review>>>,
    pub historical_review: RwSignal<Option<ReviewGroupSummary>>,
    /// Identity of the in-flight or installed History request. A group id
    /// alone is insufficient when the same group is requested twice and
    /// its async results arrive out of order.
    pub historical_review_request: RwSignal<Option<u64>>,
    /// Finding state reconstructed at the selected group's cutoff. Current
    /// dashboard state remains in `AppState` and is restored on Back.
    pub historical_findings: RwSignal<Option<Arc<Vec<ReviewFindingSummary>>>>,
    historical_review_rollback: RwSignal<Option<HistoricalReviewRollback>>,
    /// Conversation or Files. Remembered per item in `tabs_by_item` for
    /// the life of the app.
    pub tab: RwSignal<ItemTab>,
    pub tabs_by_item: RwSignal<HashMap<String, ItemTab>>,
    /// Stacked or Split, remembered across items in `meta`.
    pub layout: RwSignal<Layout>,
    /// The files with a picture that the reader asked to see as markup,
    /// by path. A picture is drawn until they ask, and the strip's Source
    /// tile asks and unasks; the choice lives as long as the app.
    pub as_source: RwSignal<HashSet<String>>,
    /// What the diff is over. Per item; Branch when the item opens.
    pub scope: RwSignal<Scope>,
    /// Identity of the backend file context currently rendered. File and
    /// metadata events must name this exact generation as well as the item
    /// and navigation request before they can update the view.
    pub scope_gen: RwSignal<Option<u64>>,
    /// The commits over the base, newest first, for the scope menu.
    pub commits: RwSignal<Arc<Vec<Commit>>>,
    /// Whether the checkout has unstaged or staged work, for greying the
    /// scope menu's entries.
    pub dirty: RwSignal<Dirt>,
    /// The head at the previous open of this item, which "Since you last
    /// looked" ranges from. `None` before the first.
    pub since_head: RwSignal<Option<String>>,
    /// Everything said on the pull request on GitHub, once fetched or read
    /// back from the cache.
    pub discussion: RwSignal<Option<Arc<Discussion>>>,
    /// The reactions on everything said, by the subject's node id: the
    /// discussion's or the issue's when they land, then whatever the
    /// reader toggles. Kept apart from the discussion so a click redraws
    /// one row of pills, not the column.
    pub reactions: RwSignal<HashMap<String, Reactions>>,
    /// The reactions asked of GitHub and not yet answered, by subject and
    /// kind. A pill in flight takes no second click, and a discussion that
    /// lands meanwhile does not put its older count over the toggle.
    pub reacting: RwSignal<HashSet<(String, ReactionKind)>>,
    /// The workflow runs' verdict for the head, once fetched. `None` means
    /// unknown, and the summary has no Checks row.
    pub checks: RwSignal<Option<Arc<Checks>>>,
    /// Live GitHub mergeability, permissions, enabled merge methods, and
    /// detailed checks for the pull request's exact head. Unlike `checks`,
    /// this is never restored from disk: merge controls require a fresh
    /// answer from GitHub.
    pub pull_request_overview: RwSignal<Option<Arc<PullRequestOverview>>>,
    pub pull_request_overview_loading: RwSignal<bool>,
    pub pull_request_overview_error: RwSignal<Option<String>>,
    /// Latest live status cycle accepted for the open pull request. Merge UI
    /// uses this to discard consent captured against an older rules snapshot.
    pub pull_request_status_generation: RwSignal<u64>,
    /// State of the deliberate, confirmed merge mutation.
    pub merge_pending: RwSignal<bool>,
    pub merge_error: RwSignal<Option<String>>,
    /// Latest merge mutation observed for the displayed checkout. Every event
    /// also carries the item and pull-request identity, so checkout resets can
    /// clear this without letting a different subject settle the new view.
    pub merge_attempt_id: RwSignal<Option<u64>>,
    /// The (path, blob) pairs the reader has checked as viewed.
    pub viewed: RwSignal<Arc<std::collections::HashSet<(String, String)>>>,
    /// The open files the reader asked to see whole rather than folded to
    /// their changes, by path. Every file opens folded, and the strip's
    /// Show whole tile asks and unasks for the one that is open; the
    /// choice is the tab's, so it goes when the tab closes and a file
    /// opened again opens folded. Not stored: a reader unfolds a file to
    /// read it now, not to read it that way every time.
    pub whole: RwSignal<HashSet<String>>,
    /// The runs the reader asked to see, by (path, blob), in head lines,
    /// so a reveal made in one layout holds in the other. Loaded with the
    /// viewed set when the item opens; a file with no blob keeps its
    /// reveals here under `(path, "")` and nowhere else.
    pub reveals: RwSignal<HashMap<(String, String), Reveals>>,
    /// Reveals and resets not yet sent to the store, drained by the root
    /// view, which holds the backend handle.
    pub pending_reveals: RwSignal<Vec<RevealWrite>>,
    /// The references panel's content, when it is open.
    pub references: RwSignal<Option<Arc<References>>>,
    /// A goto asked for and not yet answered.
    pub goto: RwSignal<Option<GotoAsk>>,
    /// The call hierarchy panel's content, when it is open. One panel at a
    /// time: opening this one closes the references panel.
    pub hierarchy: RwSignal<Option<Arc<Hierarchy>>>,
    /// A call hierarchy asked for and not yet prepared.
    pub hierarchy_ask: RwSignal<Option<HierarchyAsk>>,
    /// Asks for the calls under an item not yet sent, drained by the panel,
    /// which holds the backend handle: the event that opens the panel has
    /// none, and asks for the root's calls this way.
    pub pending_calls: RwSignal<Vec<(u64, perch_lsp::lsp_types::CallHierarchyItem, Direction)>>,
    /// Where the jumps came from, for `Ctrl--` and `Ctrl-Shift--`. Lives
    /// as long as the item is open.
    pub history: RwSignal<History>,
    /// The panel's height. Dragged.
    pub references_height: RwSignal<f64>,
    pub changed_files: RwSignal<Arc<Vec<ChangedFile>>>,
    pub tree: RwSignal<Arc<Vec<TreeEntry>>>,
    /// Whether the rail's tree is cut down to the changed files. On by
    /// default: the review is of the changes, and the whole worktree is
    /// a click away. Comes off on its own when a file the cut hides is
    /// opened, from a definition say, so the tree can show it.
    pub changed_only: RwSignal<bool>,
    pub tabs: RwSignal<Vec<Tab>>,
    pub active: RwSignal<Option<String>>,
    pub files: RwSignal<HashMap<String, Arc<LoadedFile>>>,
    /// Paths whose load is in flight, so a tab can show a spinner and a
    /// second click does not queue the work twice.
    pub loading: RwSignal<HashMap<String, ()>>,
    /// Display line the cursor sits on, for the status line.
    pub cursor_line: RwSignal<usize>,
    /// Bumped whenever the active file's content changes, to rebuild the
    /// editor view: another file is another editor.
    pub editor_epoch: RwSignal<u64>,
    /// The documents the open editors hold, set by the view that builds
    /// them and `None` while no editor is open. What the editor lays out
    /// is edited in place through these when the document changes under
    /// it, rather than the editor being rebuilt.
    pub docs: RwSignal<Option<LiveDocs>>,

    /// Every thread in the open worktree, with messages and runs.
    pub comment_threads: RwSignal<Arc<Vec<CommentThread>>>,
    /// Finding threads hidden from ordinary Conversation/Files rendering.
    /// Their durable threads remain loaded and are revealed by the All filter.
    pub hidden_review_threads: RwSignal<HashSet<String>>,
    pub review_finding_filter: RwSignal<ReviewFindingFilter>,
    /// An open composer, and where it is anchored.
    pub composer: RwSignal<Option<Composer>>,
    /// How many composers have been opened, for `Composer::opening`.
    pub composer_opened: RwSignal<u64>,
    /// The anchor of a question sent from the composer's card in the
    /// code whose thread has not arrived yet. The card's rows stay under
    /// the anchor, blank, until the next thread list brings the thread
    /// and its card takes their place, so the code does not close over
    /// the rows and open again a moment later.
    pub sent_composer: RwSignal<Option<Anchor>>,
    /// The editor's selection as display lines, first and last inclusive.
    pub selection: RwSignal<Option<(usize, usize)>>,
    /// A gutter marker was clicked: the inline card to scroll to and flash.
    /// Left as it was set rather than taken so a second click on the same
    /// marker is still a second flash, while a card built after the click
    /// ignores what it finds here.
    pub focus_card: RwSignal<Option<CardId>>,
    /// A thread whose Delete is waiting for a yes, under its byline.
    pub confirm_thread: RwSignal<Option<String>>,
    /// Runs whose step list the reader opened.
    pub expanded_runs: RwSignal<std::collections::HashSet<String>>,
    /// The rows each card has now: the estimate until the card has been
    /// laid out, and what the layout said after. Empty for a card whose
    /// rows are the estimate.
    pub card_rows: RwSignal<HashMap<CardId, usize>>,
    /// The cards drawn as one row. Kept for the app's life and not per
    /// item, as `expanded_runs` is: a card's ids are unique across items.
    pub collapsed_cards: RwSignal<HashSet<CardId>>,
    /// The resolved threads whose cards the reader opened. A resolved
    /// thread is drawn as one row until it is here, the reverse of
    /// `collapsed_cards`, so a thread resolved on GitHub folds away
    /// without a write.
    pub opened_cards: RwSignal<HashSet<CardId>>,
    /// Every card that has arrived since launch. A thread's card starts
    /// collapsed when the thread is outdated, and that default is applied
    /// once, when the thread first arrives; after that the reader's own
    /// choice stands, through closing the item and opening it again.
    pub known_cards: RwSignal<HashSet<CardId>>,
    /// The backend, model and effort last sent, which the next composer
    /// starts on.
    pub agent_last: RwSignal<Option<AgentChoice>>,
    /// The destination last sent from each face, by `ThreadFace::index`:
    /// inline in the code, and on Conversation. A new composer on lines
    /// starts on its face's. Kept for the app's life, as `agent_last` is.
    pub last_destination: [RwSignal<Option<Destination>>; 2],
    /// The agent section of config, `AppState::agent`'s own signal once
    /// the app has wired it, so a follow-up under an agent's answer opens
    /// on a model config offers, at an effort it takes.
    pub agent: RwSignal<perch_github::AgentConfig>,
    /// The display line the pointer is over in the gutter, for the `+`,
    /// or the row of a divider, for its controls.
    pub hovered_line: RwSignal<Option<usize>>,
    /// The element that recorded `hovered_line`. A divider's row is
    /// several elements, its gutter row and the parts of the bar over
    /// the code, and floem delivers the enter to the next before the
    /// leave from the last; a leave clears the line only when it comes
    /// from the element that set it, so a move between the parts of one
    /// row keeps its controls.
    pub hovered_by: RwSignal<Option<floem::ElementId>>,
    /// True while the composer's text box has the keyboard, so `a` stands
    /// down.
    pub composer_focused: RwSignal<bool>,
    /// What is in the composer's box, so Escape can tell an empty one
    /// from one with a question in it.
    pub composer_draft: RwSignal<String>,
    /// Whether a composer takes the keyboard when it opens. On, except in
    /// the headless tests, which drive the keys through the window and
    /// want no text box taking them.
    pub composer_autofocus: RwSignal<bool>,
    /// The text box of a composer laid in place on Conversation, while one
    /// is open, so `a` can focus it and scroll to it.
    pub composer_box: RwSignal<Option<floem::ViewId>>,
    /// The composer's Comment was clicked once and the footer is asking
    /// whether to post to the pull request; the next Enter or Post does.
    pub composer_confirming: RwSignal<bool>,
    /// An issue comment in flight, with the token its answer carries. The
    /// composer stays open and still until the answer comes.
    pub posting: RwSignal<Option<Posting>>,
    /// Why the last issue comment did not post, under the composer's
    /// box until the text changes or the composer closes.
    pub composer_error: RwSignal<Option<String>>,
    pub lsp_status: RwSignal<perch_lsp::Status>,
    /// Rises with every hover and jump request. An answer carrying an older
    /// token is stale, because the pointer has moved on since it was asked.
    pub request_token: RwSignal<u64>,
    /// The identifier under the pointer, kept by the editor's pointer
    /// tracking. `None` over nothing: whitespace, punctuation, a removed
    /// line, a divider, a filler row, or outside the code.
    pub pointer_symbol: RwSignal<Option<Symbol>>,
    /// The last hover asked for, and its answer once it lands.
    pub hover: RwSignal<Option<HoverAnswer>>,
    /// Every other use in the file of the symbol the caret rests on, as
    /// head line, start column and end column, tinted by the editor.
    /// Empty is none.
    pub highlights: RwSignal<Arc<Vec<(usize, usize, usize)>>>,
    /// The document highlight asked for, with the symbol at the caret it
    /// is about, until the caret leaves the symbol.
    pub highlight_ask: RwSignal<Option<(u64, Symbol)>>,
    /// Whether the find bar is over the code. It stays up from one file
    /// to the next, searching each for the same thing, until Escape or
    /// its Close puts it away; leaving the item puts it away too.
    pub find_open: RwSignal<bool>,
    /// What is in the find bar's box. Kept for the app's life, as
    /// `agent_last` is: the bar opens on what was last looked for.
    pub find_query: RwSignal<String>,
    /// How the query is read: with regard to case, as a whole word, as a
    /// regular expression. Kept with the query.
    pub find_options: RwSignal<crate::review::find::Options>,
    /// Every match of the query in the active file, in order down it, as
    /// a display line and byte columns; the editor tints them. Empty
    /// while the bar is closed, with no query, and for a pattern that
    /// does not parse.
    pub find_matches: RwSignal<Arc<Vec<crate::review::find::Match>>>,
    /// Which of `find_matches` the bar is on: the one selected in the
    /// editor and counted up to. `None` when the bar has not walked to
    /// one, as when it has just opened or the file has just changed.
    pub find_current: RwSignal<Option<usize>>,
    /// Why a pattern matched nothing, when it did not parse, for the bar
    /// to say in place of the count.
    pub find_error: RwSignal<Option<String>>,
    /// True while the find bar's box has the keyboard, so the code's
    /// keys stand down, as `composer_focused` does for the composer.
    pub find_focused: RwSignal<bool>,
    /// The find bar's box while the bar is up, so Cmd-F can give it the
    /// keyboard again.
    pub find_box: RwSignal<Option<floem::ViewId>>,
    /// Cmd-F asked for the keyboard before the bar's box was built; the
    /// box takes it once it is in the tree, as the sidebar's search box
    /// does with `focus_search`.
    pub find_focus: RwSignal<bool>,
    /// A jump waiting to be made, as the file, the head line and the column
    /// the caret lands on. The view with the file-opening closure takes it.
    pub pending_jump: RwSignal<Option<(String, usize, usize)>>,
    /// A head line to scroll to once the file it belongs to is loaded.
    pub jump_to_line: RwSignal<Option<usize>>,
    /// Where the caret lands after `jump_to_line`, as the head line and a
    /// column, when the jump is a navigation and not a scroll: a jump to a
    /// definition puts the caret on it, so the next jump remembers leaving
    /// from there. Read by the jump, and cleared by it.
    pub jump_caret: RwSignal<Option<(usize, usize)>>,
    /// The head position the last request was made about, so the definition
    /// button asks about the same symbol the panel is describing.
    pub last_position: RwSignal<Option<(usize, usize)>>,
    /// Directories open in the rail's tree. A checkout has thousands of
    /// entries, so only the way to each changed file starts open.
    pub expanded: RwSignal<std::collections::HashSet<String>>,
    /// The row the rail's tree scrolls to once it is laid out: the file
    /// just opened, which may be far down the tree or in a part of it the
    /// filter hid until now. The row takes it on its next layout.
    pub scroll_to_row: RwSignal<Option<String>>,
}

/// How many tabs stay open before the oldest is dropped.
const MAX_TABS: usize = 20;

impl Default for ReviewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewState {
    pub fn new() -> Self {
        Self {
            pull_request: RwSignal::new(None),
            item: RwSignal::new(None),
            ready: RwSignal::new(false),
            issue: RwSignal::new(None),
            issue_comments: RwSignal::new(Arc::new(Vec::new())),
            stale_head: RwSignal::new(None),
            offline: RwSignal::new(false),
            opening: RwSignal::new(false),
            phase: RwSignal::new(Phase::Preparing),
            preparing: RwSignal::new(Preparing::Idle),
            review_launcher_open: RwSignal::new(false),
            review_launcher_draft: RwSignal::new(None),
            review_launcher_opener: RwSignal::new(None),
            submit_review_open: RwSignal::new(false),
            submit_review_draft: RwSignal::new(String::new()),
            submit_review_event: RwSignal::new(PullReviewEvent::Comment),
            submit_review_opener: RwSignal::new(None),
            submit_review_focused: RwSignal::new(false),
            submit_review_autofocus: RwSignal::new(true),
            submit_review_pending: RwSignal::new(None),
            submit_review_error: RwSignal::new(None),
            submit_review_outcome: RwSignal::new(None),
            submit_review_generation: RwSignal::new(0),
            submit_review_confirm_discard: RwSignal::new(false),
            submitted_reviews: RwSignal::new(Arc::new(Vec::new())),
            historical_review: RwSignal::new(None),
            historical_review_request: RwSignal::new(None),
            historical_findings: RwSignal::new(None),
            historical_review_rollback: RwSignal::new(None),
            tab: RwSignal::new(ItemTab::Conversation),
            tabs_by_item: RwSignal::new(HashMap::new()),
            layout: RwSignal::new(Layout::Stacked),
            as_source: RwSignal::new(HashSet::new()),
            scope: RwSignal::new(Scope::Branch),
            scope_gen: RwSignal::new(None),
            commits: RwSignal::new(Arc::new(Vec::new())),
            dirty: RwSignal::new(Dirt::default()),
            since_head: RwSignal::new(None),
            discussion: RwSignal::new(None),
            reactions: RwSignal::new(HashMap::new()),
            reacting: RwSignal::new(HashSet::new()),
            checks: RwSignal::new(None),
            pull_request_overview: RwSignal::new(None),
            pull_request_overview_loading: RwSignal::new(false),
            pull_request_overview_error: RwSignal::new(None),
            pull_request_status_generation: RwSignal::new(0),
            merge_pending: RwSignal::new(false),
            merge_error: RwSignal::new(None),
            merge_attempt_id: RwSignal::new(None),
            viewed: RwSignal::new(Arc::new(std::collections::HashSet::new())),
            whole: RwSignal::new(HashSet::new()),
            reveals: RwSignal::new(HashMap::new()),
            pending_reveals: RwSignal::new(Vec::new()),
            references: RwSignal::new(None),
            goto: RwSignal::new(None),
            hierarchy: RwSignal::new(None),
            hierarchy_ask: RwSignal::new(None),
            pending_calls: RwSignal::new(Vec::new()),
            history: RwSignal::new(History::default()),
            references_height: RwSignal::new(crate::theme::REFERENCES_HEIGHT),
            changed_files: RwSignal::new(Arc::new(Vec::new())),
            tree: RwSignal::new(Arc::new(Vec::new())),
            changed_only: RwSignal::new(true),
            tabs: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            files: RwSignal::new(HashMap::new()),
            loading: RwSignal::new(HashMap::new()),
            cursor_line: RwSignal::new(0),
            editor_epoch: RwSignal::new(0),
            docs: RwSignal::new(None),
            comment_threads: RwSignal::new(Arc::new(Vec::new())),
            hidden_review_threads: RwSignal::new(HashSet::new()),
            review_finding_filter: RwSignal::new(ReviewFindingFilter::Open),
            composer: RwSignal::new(None),
            composer_opened: RwSignal::new(0),
            sent_composer: RwSignal::new(None),
            selection: RwSignal::new(None),
            focus_card: RwSignal::new(None),
            confirm_thread: RwSignal::new(None),
            expanded_runs: RwSignal::new(std::collections::HashSet::new()),
            card_rows: RwSignal::new(HashMap::new()),
            collapsed_cards: RwSignal::new(HashSet::new()),
            opened_cards: RwSignal::new(HashSet::new()),
            known_cards: RwSignal::new(HashSet::new()),
            agent_last: RwSignal::new(None),
            last_destination: [RwSignal::new(None), RwSignal::new(None)],
            agent: RwSignal::new(perch_github::AgentConfig::default()),
            hovered_line: RwSignal::new(None),
            hovered_by: RwSignal::new(None),
            composer_focused: RwSignal::new(false),
            composer_draft: RwSignal::new(String::new()),
            composer_autofocus: RwSignal::new(true),
            composer_box: RwSignal::new(None),
            composer_confirming: RwSignal::new(false),
            posting: RwSignal::new(None),
            composer_error: RwSignal::new(None),
            lsp_status: RwSignal::new(perch_lsp::Status::Absent),
            request_token: RwSignal::new(0),
            pointer_symbol: RwSignal::new(None),
            hover: RwSignal::new(None),
            highlights: RwSignal::new(Arc::new(Vec::new())),
            highlight_ask: RwSignal::new(None),
            find_open: RwSignal::new(false),
            find_query: RwSignal::new(String::new()),
            find_options: RwSignal::new(Default::default()),
            find_matches: RwSignal::new(Arc::new(Vec::new())),
            find_current: RwSignal::new(None),
            find_error: RwSignal::new(None),
            find_focused: RwSignal::new(false),
            find_box: RwSignal::new(None),
            find_focus: RwSignal::new(false),
            pending_jump: RwSignal::new(None),
            jump_to_line: RwSignal::new(None),
            jump_caret: RwSignal::new(None),
            last_position: RwSignal::new(None),
            expanded: RwSignal::new(std::collections::HashSet::new()),
            scroll_to_row: RwSignal::new(None),
        }
    }

    pub fn is_open(&self) -> bool {
        self.ready.get()
    }

    /// Move to a phase. A write of the same value would still rebuild the
    /// view, so it is skipped.
    pub fn set_phase(&self, phase: Phase) {
        if self.phase.get_untracked() != phase {
            self.phase.set(phase);
        }
    }

    /// Start opening something: the progress card, over whatever was
    /// showing, until the next Ready or Failed. Everything that belonged
    /// to the last checkout goes, so the next one's files are not shown
    /// under its name.
    pub fn begin(&self, message: &str) {
        self.reset_checkout();
        // Whatever the last item still has on its way, a discussion or a
        // list of commits, must not land on the one being opened. Until
        // Ready says which item this is, there is none.
        self.item.set(None);
        self.preparing.set(Preparing::Working {
            message: message.into(),
            percent: None,
        });
        self.opening.set(true);
        self.set_phase(Phase::Preparing);
    }

    /// Forget the open checkout: its files, tabs, threads and the rest.
    /// The item and `opening` are the caller's to set.
    fn reset_checkout(&self) {
        self.pull_request.set(None);
        self.ready.set(false);
        self.issue.set(None);
        self.issue_comments.set(Arc::new(Vec::new()));
        self.stale_head.set(None);
        self.offline.set(false);
        self.review_launcher_open.set(false);
        self.review_launcher_draft.set(None);
        self.review_launcher_opener.set(None);
        self.submit_review_open.set(false);
        self.submit_review_draft.set(String::new());
        self.submit_review_event.set(PullReviewEvent::Comment);
        self.submit_review_opener.set(None);
        self.submit_review_focused.set(false);
        self.submit_review_pending.set(None);
        self.submit_review_error.set(None);
        self.submit_review_outcome.set(None);
        self.submit_review_confirm_discard.set(false);
        self.submitted_reviews.set(Arc::new(Vec::new()));
        self.clear_historical_review();
        self.tab.set(ItemTab::Conversation);
        self.scope.set(Scope::Branch);
        self.scope_gen.set(None);
        self.commits.set(Arc::new(Vec::new()));
        self.dirty.set(Dirt::default());
        self.since_head.set(None);
        self.discussion.set(None);
        self.reactions.set(HashMap::new());
        self.reacting.set(HashSet::new());
        self.checks.set(None);
        self.pull_request_overview.set(None);
        self.pull_request_overview_loading.set(false);
        self.pull_request_overview_error.set(None);
        self.pull_request_status_generation.set(0);
        self.merge_pending.set(false);
        self.merge_error.set(None);
        self.merge_attempt_id.set(None);
        self.viewed.set(Arc::new(std::collections::HashSet::new()));
        // Only written when they change: the open editor rebuilds its
        // document on either, and it is about to go.
        if !self.whole.with_untracked(HashSet::is_empty) {
            self.whole.set(HashSet::new());
        }
        if !self.reveals.with_untracked(HashMap::is_empty) {
            self.reveals.set(HashMap::new());
        }
        self.pending_reveals.set(Vec::new());
        self.references.set(None);
        self.goto.set(None);
        self.hierarchy.set(None);
        self.hierarchy_ask.set(None);
        self.pending_calls.set(Vec::new());
        self.history.set(History::default());
        self.pending_jump.set(None);
        self.changed_files.set(Arc::new(Vec::new()));
        self.tree.set(Arc::new(Vec::new()));
        self.tabs.set(Vec::new());
        self.active.set(None);
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        self.changed_only.set(true);
        self.lsp_status.set(perch_lsp::Status::Absent);
        self.comment_threads.set(Arc::new(Vec::new()));
        self.hidden_review_threads.set(HashSet::new());
        self.review_finding_filter.set(ReviewFindingFilter::Open);
        self.composer.set(None);
        self.sent_composer.set(None);
        self.selection.set(None);
        self.focus_card.set(None);
        self.confirm_thread.set(None);
        // The rows are the next item's cards' to fill; whether a card is
        // open is the reader's and stays, as `collapsed_cards` says.
        self.card_rows.set(HashMap::new());
        self.hovered_line.set(None);
        self.hovered_by.set(None);
        self.composer_focused.set(false);
        self.composer_draft.set(String::new());
        self.composer_confirming.set(false);
        self.posting.set(None);
        self.composer_error.set(None);
        self.expanded.set(std::collections::HashSet::new());
        self.scroll_to_row.set(None);
        self.editor_epoch.update(|e| *e += 1);
        self.docs.set(None);
        self.pointer_symbol.set(None);
        self.hover.set(None);
        self.clear_highlights();
        self.last_position.set(None);
        // The bar goes with the files it was over; what it was looking
        // for stays, for the next item.
        self.find_open.set(false);
        self.find_focused.set(false);
        self.find_focus.set(false);
        self.find_current.set(None);
        self.find_error.set(None);
        if !self.find_matches.with_untracked(|m| m.is_empty()) {
            self.find_matches.set(Arc::new(Vec::new()));
        }
    }

    /// Prepare to install an accepted live result. Agent history navigation
    /// keeps the prior owner visible while its new owner is resolving, so a
    /// cross-item result must clear that owner's checkout at commit time.
    pub(crate) fn prepare_ready_for(&self, item_id: &str) {
        if self
            .item
            .with_untracked(|item| item.as_ref().is_some_and(|item| item.id != item_id))
        {
            self.reset_checkout();
        } else {
            // A fresh live checkout always gets a fresh language server. Its
            // status event follows Ready and names the same navigation.
            self.lsp_status.set(perch_lsp::Status::Absent);
        }
    }

    /// Optimistically select an immutable review while retaining the last
    /// stable live or historical view. Repeated clicks do not replace the
    /// original rollback point with another optimistic selection.
    pub fn begin_historical_review(
        &self,
        navigation_id: Option<u64>,
        summary: ReviewGroupSummary,
        request_id: u64,
    ) {
        self.begin_historical_review_rollback(navigation_id);
        self.historical_review.set(Some(summary));
        self.historical_review_request.set(Some(request_id));
        self.historical_findings.set(None);
    }

    /// Capture the stable view before a history target whose summary has not
    /// been resolved yet, as with navigation from a reviewer child.
    pub(crate) fn begin_historical_review_rollback(&self, navigation_id: Option<u64>) {
        if self
            .historical_review_rollback
            .with_untracked(Option::is_none)
        {
            self.historical_review_rollback
                .set(Some(HistoricalReviewRollback {
                    navigation_id,
                    review: self.historical_review.get_untracked(),
                    request_id: self.historical_review_request.get_untracked(),
                    findings: self.historical_findings.get_untracked(),
                }));
        }
    }

    /// The selected request is now the stable baseline for any later switch.
    pub fn finish_historical_review(&self, findings: Arc<Vec<ReviewFindingSummary>>) {
        self.historical_findings.set(Some(findings));
        self.historical_review_rollback.set(None);
    }

    /// Restore the stable view captured by the first optimistic selection.
    /// With no captured baseline, failure falls back to the live view.
    pub(crate) fn rollback_historical_review(&self) -> Option<HistoricalReviewRollback> {
        let rollback = self.historical_review_rollback.get_untracked();
        if let Some(rollback) = rollback.as_ref() {
            self.historical_review.set(rollback.review.clone());
            self.historical_review_request.set(rollback.request_id);
            self.historical_findings.set(rollback.findings.clone());
        } else {
            self.historical_review.set(None);
            self.historical_review_request.set(None);
            self.historical_findings.set(None);
        }
        self.historical_review_rollback.set(None);
        rollback
    }

    /// Return to a live/current context and discard any optimistic rollback.
    pub fn clear_historical_review(&self) {
        self.historical_review.set(None);
        self.historical_review_request.set(None);
        self.historical_findings.set(None);
        self.historical_review_rollback.set(None);
    }

    /// Show a tab, and remember it for the item.
    pub fn show_tab(&self, tab: ItemTab) {
        self.tab.set(tab);
        if let Some(id) = self.item.with(|i| i.as_ref().map(|i| i.id.clone())) {
            self.tabs_by_item.update(|m| {
                m.insert(id, tab);
            });
        }
    }

    /// The tab the item was on when it was last open, else Conversation.
    pub fn restore_tab(&self) {
        let tab = self
            .item
            .with(|i| {
                i.as_ref()
                    .and_then(|i| self.tabs_by_item.with(|m| m.get(&i.id).copied()))
            })
            .unwrap_or_default();
        self.tab.set(tab);
    }

    /// Whether the open item is this one.
    pub fn is_item(&self, id: &str) -> bool {
        self.item.with(|i| i.as_ref().is_some_and(|i| i.id == id))
    }

    /// Record the backend attempt that owns the review submission UI. An
    /// older result can still enter the timeline, but must not settle this
    /// surface once a newer attempt owns it.
    pub fn begin_submit_review(&self, attempt_id: u64) {
        self.submit_review_pending.set(Some(attempt_id));
        self.submit_review_error.set(None);
        self.submit_review_outcome.set(None);
        self.submit_review_confirm_discard.set(false);
    }

    /// Install a review GitHub accepted before the follow-up discussion GET
    /// completes. Keeping it in `submitted_reviews` also protects the UI from
    /// an older discussion request that was already in flight. `None` is a
    /// submission that needed no review: only replies and comments on
    /// their own. With `outcome`, what did not post: the form stays open
    /// to say so, its summary cleared and built anew since that posted,
    /// and the drafts that stayed drafts listed as before. Answers whether
    /// the form that sent the attempt was still there to be told.
    pub fn finish_submit_review(
        &self,
        attempt_id: u64,
        review: Option<Review>,
        outcome: Option<String>,
    ) -> bool {
        let pending = self.submit_review_pending.get_untracked();
        let owns_surface = pending == Some(attempt_id);
        // `None` means the item was left and reopened while the backend kept
        // this attempt alive. A different id means a newer submission now
        // owns the surface. In both cases the durable review still belongs in
        // the timeline, without disturbing that fresh draft or attempt.
        if owns_surface {
            self.submit_review_pending.set(None);
            self.submit_review_draft.set(String::new());
            self.submit_review_event.set(PullReviewEvent::Comment);
            self.submit_review_confirm_discard.set(false);
            self.submit_review_error.set(None);
            match outcome {
                Some(message) => {
                    self.submit_review_outcome.set(Some(message));
                    self.submit_review_generation.update(|n| *n += 1);
                }
                None => {
                    self.submit_review_outcome.set(None);
                    self.submit_review_open.set(false);
                    self.submit_review_focused.set(false);
                }
            }
        }

        if let Some(review) = review {
            self.submitted_reviews.update(|reviews| {
                let reviews = Arc::make_mut(reviews);
                match reviews.iter_mut().find(|known| known.id == review.id) {
                    Some(known) => *known = review.clone(),
                    None => reviews.push(review.clone()),
                }
            });
            self.discussion.update(|discussion| {
                let Some(current) = discussion.as_ref() else {
                    return;
                };
                let mut current = (**current).clone();
                match current.reviews.iter_mut().find(|known| known.id == review.id) {
                    Some(known) => *known = review.clone(),
                    None => current.reviews.push(review.clone()),
                }
                *discussion = Some(Arc::new(current));
            });
        }

        if owns_surface && !self.submit_review_open.get_untracked() {
            if let Some(opener) = self.submit_review_opener.get_untracked() {
                opener.request_focus();
            }
            self.submit_review_opener.set(None);
        }
        owns_surface
    }

    pub fn fail_submit_review(&self, attempt_id: u64, message: String) -> bool {
        if self
            .submit_review_pending
            .get_untracked()
            .is_some_and(|current| current != attempt_id)
        {
            return false;
        }
        self.submit_review_pending.set(None);
        self.submit_review_error.set(Some(message));
        true
    }

    /// Merge locally confirmed reviews into a fetched discussion only when
    /// that snapshot does not contain their id. A newer server-side state
    /// (for example a dismissed review) wins when it is present.
    pub fn with_submitted_reviews(&self, discussion: Arc<Discussion>) -> Arc<Discussion> {
        let submitted = self.submitted_reviews.get_untracked();
        if submitted.is_empty()
            || submitted
                .iter()
                .all(|review| discussion.reviews.iter().any(|known| known.id == review.id))
        {
            return discussion;
        }
        let mut merged = (*discussion).clone();
        for review in submitted.iter() {
            if !merged.reviews.iter().any(|known| known.id == review.id) {
                merged.reviews.push(review.clone());
            }
        }
        Arc::new(merged)
    }

    /// The reactions on a subject, as the pills draw them. Tracked.
    pub fn reactions_of(&self, node_id: &str) -> Reactions {
        self.reactions
            .with(|m| m.get(node_id).cloned())
            .unwrap_or_default()
    }

    /// Whether a reaction of this kind on this subject is with GitHub.
    pub fn is_reacting(&self, node_id: &str, kind: ReactionKind) -> bool {
        self.reacting
            .with(|r| r.contains(&(node_id.to_string(), kind)))
    }

    /// Take the reactions a discussion or an issue landed with. A subject
    /// with a toggle in flight keeps what the reader sees: the answer to
    /// the toggle settles it, and an older read must not.
    pub fn install_reactions<'a>(&self, read: impl IntoIterator<Item = (&'a str, &'a Reactions)>) {
        let in_flight: HashSet<String> = self
            .reacting
            .with_untracked(|r| r.iter().map(|(id, _)| id.clone()).collect());
        self.reactions.update(|m| {
            for (id, reactions) in read {
                if id.is_empty() || in_flight.contains(id) {
                    continue;
                }
                m.insert(id.to_string(), reactions.clone());
            }
        });
    }

    /// An issue's reactions: the body's and each comment's.
    pub fn install_issue_reactions(&self, issue: &Issue, comments: &[IssueComment]) {
        self.install_reactions(
            std::iter::once((issue.node_id.as_str(), &issue.reactions))
                .chain(comments.iter().map(|c| (c.node_id.as_str(), &c.reactions))),
        );
    }

    /// The reader's reaction, toggled where it is shown before GitHub is
    /// asked: what to ask, or `None` while an earlier ask on the same pill
    /// is unanswered.
    pub fn toggle_reaction(&self, node_id: &str, kind: ReactionKind) -> Option<bool> {
        let busy = self
            .reacting
            .with_untracked(|r| r.contains(&(node_id.to_string(), kind)));
        if node_id.is_empty() || busy {
            return None;
        }
        let add = !self
            .reactions
            .with_untracked(|m| m.get(node_id).is_some_and(|r| r.viewer_reacted(kind)));
        self.reacting.update(|r| {
            r.insert((node_id.to_string(), kind));
        });
        self.reactions.update(|m| {
            m.entry(node_id.to_string()).or_default().toggle(kind, add);
        });
        Some(add)
    }

    /// GitHub's answer to a toggle: the subject's reactions as it has them.
    pub fn settle_reaction(&self, node_id: &str, kind: ReactionKind, reactions: Reactions) {
        self.reacting.update(|r| {
            r.remove(&(node_id.to_string(), kind));
        });
        self.reactions.update(|m| {
            m.insert(node_id.to_string(), reactions);
        });
    }

    /// GitHub's refusal of a toggle: the pill goes back to what it showed.
    pub fn revert_reaction(&self, node_id: &str, kind: ReactionKind, add: bool) {
        self.reacting.update(|r| {
            r.remove(&(node_id.to_string(), kind));
        });
        self.reactions.update(|m| {
            if let Some(reactions) = m.get_mut(node_id) {
                reactions.toggle(kind, !add);
            }
        });
    }

    /// Whether the review view should be on screen at all.
    pub fn is_showing(&self) -> bool {
        self.opening.get() || self.is_open()
    }

    /// Leave the review view for the sidebar's empty main area.
    pub fn close(&self) {
        self.reset_checkout();
        self.opening.set(false);
        self.item.set(None);
        self.preparing.set(Preparing::Idle);
        self.set_phase(Phase::Preparing);
    }

    /// Start a new request, invalidating any answer still in flight.
    pub fn next_token(&self) -> u64 {
        self.request_token.update(|t| *t += 1);
        self.request_token.get()
    }

    /// Ask about the symbol under the pointer. The answer's slot is taken
    /// at once, empty, so the tooltip can fill it in place when the answer
    /// lands with the same token; asking about a symbol whose answer is
    /// already here, or on its way, asks nothing.
    pub fn ask_hover(&self, backend: &crate::backend::Backend, symbol: Symbol) {
        if self
            .hover
            .with_untracked(|h| h.as_ref().is_some_and(|h| h.symbol == symbol))
        {
            return;
        }
        let token = self.next_token();
        backend.hover(token, &symbol.path, symbol.line as u32, symbol.column as u32);
        self.hover.set(Some(HoverAnswer {
            symbol,
            markdown: String::new(),
            token,
        }));
    }

    /// The caret left the symbol whose uses were tinted, or nothing was:
    /// no tint, and no answer taken.
    pub fn clear_highlights(&self) {
        if !self.highlights.with_untracked(|h| h.is_empty()) {
            self.highlights.set(Arc::new(Vec::new()));
        }
        if self.highlight_ask.with_untracked(Option::is_some) {
            self.highlight_ask.set(None);
        }
    }

    /// Ask for every use in the file of the symbol the caret rests on.
    pub fn ask_highlights(&self, backend: &crate::backend::Backend, symbol: Symbol) {
        if !self.lsp_status.with_untracked(|s| s.is_usable()) {
            return;
        }
        let token = self.next_token();
        backend.document_highlight(token, &symbol.path, symbol.line as u32, symbol.column as u32);
        self.highlight_ask.set(Some((token, symbol)));
    }

    /// The uses arrived. Kept only for the ask still waiting: the caret
    /// may have moved on since.
    pub fn finish_highlights(&self, token: u64, ranges: Vec<(usize, usize, usize)>) {
        if self
            .highlight_ask
            .with_untracked(|a| a.as_ref().is_some_and(|(t, _)| *t == token))
        {
            self.highlights.set(Arc::new(ranges));
        }
    }

    /// The hover's answer, when it is about the symbol under the pointer
    /// and has landed. What the tooltip draws; `None` draws nothing.
    pub fn hover_for_pointer(&self) -> Option<String> {
        let pointer = self.pointer_symbol.get()?;
        self.hover.with(|h| {
            h.as_ref()
                .filter(|h| h.symbol == pointer && !h.markdown.is_empty())
                .map(|h| h.markdown.clone())
        })
    }

    /// Where the reader is: the active file and the caret's head line,
    /// which the jump keys remember. `None` with no file open.
    pub fn current_place(&self) -> Option<Place> {
        let file = self.active_file()?;
        let line = self
            .last_position
            .get_untracked()
            .map(|(line, _)| line)
            .or_else(|| file.buffer.head_line(self.cursor_line.get_untracked()))
            .unwrap_or(0);
        Some(Place {
            path: file.path.clone(),
            line,
        })
    }

    /// Jump to a head line of a file, remembering where the jump left. The
    /// caret lands at `column` on the line.
    pub fn navigate_to(&self, path: &str, line: usize, column: usize) {
        if let Some(from) = self.current_place() {
            self.history.update(|h| h.push(from));
        }
        self.pending_jump.set(Some((path.to_string(), line, column)));
    }

    /// `Ctrl--`: back to where the last jump left.
    pub fn go_back(&self) {
        let Some(current) = self.current_place() else { return };
        let place = self.history.try_update(|h| h.back(current)).flatten();
        if let Some(place) = place {
            self.pending_jump.set(Some((place.path, place.line, 0)));
        }
    }

    /// `Ctrl-Shift--`: forward again.
    pub fn go_forward(&self) {
        let Some(current) = self.current_place() else { return };
        let place = self.history.try_update(|h| h.forward(current)).flatten();
        if let Some(place) = place {
            self.pending_jump.set(Some((place.path, place.line, 0)));
        }
    }

    /// Open a location rust-analyzer named: the file, at the line, with
    /// the caret on the symbol.
    pub fn open_located(&self, located: &Located) {
        self.navigate_to(&located.path, located.line.saturating_sub(1), located.character);
    }

    /// A goto's answer landed. One location opens; several open the panel
    /// under the ask's kind, and the reader picks; none does nothing, since
    /// a symbol without a definition is an ordinary thing to click on.
    pub fn finish_goto(&self, token: u64, locations: Vec<Located>) {
        let ask = self.goto.try_update(|g| g.take_if(|g| g.token == token)).flatten();
        let Some(ask) = ask else { return };
        match locations.as_slice() {
            [] => {}
            [one] => self.open_located(one),
            _ => self.open_references(References {
                kind: ask.kind,
                symbol: ask.symbol,
                locations,
                pending: false,
                token,
            }),
        }
    }

    /// Open the references panel with an answer, or an ask. One panel at a
    /// time: the call hierarchy's closes.
    pub fn open_references(&self, references: References) {
        self.hierarchy.set(None);
        self.references.set(Some(Arc::new(references)));
    }

    /// Open the call hierarchy panel. The references panel closes.
    pub fn open_hierarchy(&self, hierarchy: Hierarchy) {
        self.references.set(None);
        self.hierarchy.set(Some(Arc::new(hierarchy)));
    }

    /// Ask for the calls under an item, one way or the other: queued for
    /// the panel to send, under a fresh token, which the answer carries.
    pub fn ask_calls(&self, item: perch_lsp::lsp_types::CallHierarchyItem, direction: Direction) -> u64 {
        let token = self.next_token();
        self.pending_calls.update(|p| p.push((token, item, direction)));
        token
    }

    /// The server named what a call hierarchy can start from. Nothing
    /// named opens nothing; the first item opens the panel as its callers,
    /// with the calls on their way.
    pub fn finish_prepare_hierarchy(
        &self,
        token: u64,
        items: Vec<perch_lsp::lsp_types::CallHierarchyItem>,
        place: Option<Located>,
    ) {
        let ask = self
            .hierarchy_ask
            .try_update(|a| a.take_if(|a| a.token == token))
            .flatten();
        let Some(_) = ask else { return };
        let Some(root) = items.into_iter().next() else { return };
        let place = place.unwrap_or_else(|| Located {
            path: root.uri.as_str().to_string(),
            outside: None,
            line: root.selection_range.start.line as usize + 1,
            character: root.selection_range.start.character as usize,
            text: String::new(),
        });
        let token = self.ask_calls(root.clone(), Direction::Callers);
        self.open_hierarchy(Hierarchy::new(root, place, Direction::Callers, token));
    }

    /// Expand a row of the call hierarchy: its calls are asked for. A row
    /// that repeats a function open above it, a recursive call, expands
    /// to nothing and says so, and asks nothing.
    pub fn expand_hierarchy_row(&self, at: usize) {
        let Some(hierarchy) = self.hierarchy.get_untracked() else { return };
        let Some(row) = hierarchy.rows.get(at) else { return };
        if row.expanded || row.pending.is_some() {
            return;
        }
        let token = if row.repeats {
            None
        } else {
            Some(self.ask_calls(row.item.clone(), hierarchy.direction))
        };
        let mut open = (*hierarchy).clone();
        match token {
            Some(token) => open.expanding(at, token),
            None => {
                open.rows[at].expanded = true;
            }
        }
        self.hierarchy.set(Some(Arc::new(open)));
    }

    /// Fold a row of the call hierarchy: its subtree goes.
    pub fn collapse_hierarchy_row(&self, at: usize) {
        let Some(hierarchy) = self.hierarchy.get_untracked() else { return };
        let mut open = (*hierarchy).clone();
        open.collapse(at);
        self.hierarchy.set(Some(Arc::new(open)));
    }

    /// Turn the call hierarchy the other way: the same root, its tree
    /// started again.
    pub fn turn_hierarchy(&self, direction: Direction) {
        let Some(hierarchy) = self.hierarchy.get_untracked() else { return };
        if hierarchy.direction == direction {
            return;
        }
        let token = self.ask_calls(hierarchy.root.clone(), direction);
        self.hierarchy.set(Some(Arc::new(hierarchy.turned(direction, token))));
    }

    /// The calls under a row arrived.
    pub fn finish_calls(&self, token: u64, calls: Vec<Call>) {
        self.hierarchy.update(|h| {
            if let Some(h) = h.as_mut() {
                let mut open = (**h).clone();
                if open.fill(token, calls).is_some() {
                    *h = Arc::new(open);
                }
            }
        });
    }

    /// Whether a panel is open under the editor, for Escape.
    pub fn panel_open(&self) -> bool {
        self.references.with(Option::is_some) || self.hierarchy.with(Option::is_some)
    }

    /// Escape: whichever panel is open closes.
    pub fn close_panel(&self) {
        self.references.set(None);
        self.hierarchy.set(None);
    }

    pub fn file(&self, path: &str) -> Option<Arc<LoadedFile>> {
        self.files.with(|m| m.get(path).cloned())
    }

    pub fn active_file(&self) -> Option<Arc<LoadedFile>> {
        let path = self.active.get()?;
        self.file(&path)
    }

    /// Whether the file at `path` is shown as its markup rather than
    /// drawn, which only means anything for a file with a picture.
    pub fn shows_source(&self, path: &str) -> bool {
        self.as_source.with(|paths| paths.contains(path))
    }

    /// The active file's picture, when it has one and is drawn: what the
    /// pane shows in place of the editor, and what the status line and
    /// the strip's tile describe.
    pub fn drawn_picture(&self) -> Option<Picture> {
        let file = self.active_file()?;
        if self.shows_source(&file.path) {
            return None;
        }
        file.picture.clone()
    }

    /// Flip the active file between its picture and its markup.
    pub fn toggle_source(&self) {
        let Some(path) = self.active.get_untracked() else {
            return;
        };
        self.as_source.update(|paths| {
            if !paths.remove(&path) {
                paths.insert(path);
            }
        });
    }

    /// Whether the file at `path` is folded to its changes, which it is
    /// unless the reader asked to see it whole. Tracked, so a document
    /// built from it is rebuilt when the answer changes.
    pub fn folded(&self, path: &str) -> bool {
        self.whole.with(|paths| !paths.contains(path))
    }

    /// Show whole for the active file: fold it to its changes, or show it
    /// whole. The folds come back with the reveals as they were; showing
    /// the file whole does not throw the reveals away. The other tabs
    /// stay as they are.
    pub fn toggle_whole(&self) {
        let Some(path) = self.active.get_untracked() else {
            return;
        };
        self.whole.update(|paths| {
            if !paths.remove(&path) {
                paths.insert(path);
            }
        });
    }

    pub fn is_loading(&self, path: &str) -> bool {
        self.loading.with(|m| m.contains_key(path))
    }

    /// Focus a tab, opening one if the file is not already open.
    ///
    /// Returns true when the caller should ask the backend to load the file.
    pub fn open_tab(&self, path: &str) -> bool {
        let composer_was_in_code = self.composer_in_code_untracked();
        let already_open = self.tabs.with(|t| t.iter().any(|x| x.path == path));
        if !already_open {
            let mut dropped = None;
            self.tabs.update(|tabs| {
                tabs.push(Tab {
                    path: path.to_string(),
                    origin: Origin::of_path(path),
                });
                // Oldest first, so the cap drops the least recently opened.
                if tabs.len() > MAX_TABS {
                    dropped = Some(tabs.remove(0));
                }
            });
            if let Some(dropped) = dropped {
                self.forget_whole(&dropped.path);
            }
        }
        self.active.set(Some(path.to_string()));
        self.cursor_line.set(0);
        // The tint was another file's, or another caret's.
        self.clear_highlights();
        self.editor_epoch.update(|e| *e += 1);
        self.keep_composer_in_view(composer_was_in_code);

        let needs_load = self.file(path).is_none() && !self.is_loading(path);
        if needs_load {
            self.loading.update(|m| {
                m.insert(path.to_string(), ());
            });
        }
        needs_load
    }

    /// The open file changed under an open composer. One that was drawn in
    /// the code of the file that was open, `was_in_code`, would now be
    /// hidden, so show Conversation, where new questions and follow-ups
    /// always have a home. One that was not, a question about the item as
    /// a whole say, or a card in a file other than the open one, is not
    /// hidden by the change: the reader chose Files over it, and keeps it.
    fn keep_composer_in_view(&self, was_in_code: bool) {
        let hidden = was_in_code && !self.composer_in_code_untracked();
        if hidden && self.tab.get_untracked() != ItemTab::Conversation {
            self.show_tab(ItemTab::Conversation);
        }
    }

    /// `composer_in_code` for a caller that may be an effect, which is not
    /// to depend on the file or the composer merely for having checked.
    fn composer_in_code_untracked(&self) -> bool {
        floem::reactive::Effect::untrack(|| self.composer_in_code())
    }

    /// Show a file just opened in the rail's tree: open the directories
    /// on the way to it, turn the filter off when it is not one of the
    /// changed files, since the filtered tree has no row for it, and
    /// scroll its row into view. The row is the active one, so it is
    /// drawn selected. A file outside the worktree has no row and is left
    /// alone, as is one the listing lacks, in an ignored directory say:
    /// the filter would come off for a tree that still does not show it.
    pub fn show_in_tree(&self, path: &str) {
        if std::path::Path::new(path).is_absolute() {
            return;
        }
        let changed = self
            .changed_files
            .with_untracked(|files| files.iter().any(|f| f.path == path));
        // A deleted file is in the changed list and not the listing, and
        // the rail gives it a row all the same.
        let has_row = changed
            || self
                .tree
                .with_untracked(|tree| tree.iter().any(|e| e.path == path));
        if !has_row {
            return;
        }
        // One rebuild of the rows, not one per write.
        floem::reactive::Effect::batch(|| {
            self.expanded
                .update(|expanded| crate::review::tree::reveal(expanded, path));
            // With nothing changed the tree is whole whatever the filter
            // says, so the reader's setting stands.
            let cut = self.changed_only.get_untracked()
                && !self.changed_files.with_untracked(|files| files.is_empty());
            if cut && !changed {
                self.changed_only.set(false);
            }
            self.scroll_to_row.set(Some(path.to_string()));
        });
    }

    pub fn close_tab(&self, path: &str) {
        let was_active = self.active.with(|a| a.as_deref() == Some(path));
        let composer_was_in_code = self.composer_in_code_untracked();
        let mut neighbour = None;

        self.tabs.update(|tabs| {
            if let Some(i) = tabs.iter().position(|t| t.path == path) {
                tabs.remove(i);
                neighbour = tabs
                    .get(i)
                    .or_else(|| tabs.get(i.saturating_sub(1)))
                    .map(|t| t.path.clone());
            }
        });
        // Show whole was the tab's: opened again, the file opens folded.
        self.forget_whole(path);

        // Keep the loaded content: reopening a closed tab should not refetch.
        if was_active {
            self.active.set(neighbour);
            self.cursor_line.set(0);
            self.editor_epoch.update(|e| *e += 1);
            self.keep_composer_in_view(composer_was_in_code);
        }
    }

    /// A tab is gone: its file is folded again next time. Only written
    /// when the file was whole, since the open editor rebuilds its
    /// document on every write.
    fn forget_whole(&self, path: &str) {
        if self.whole.with_untracked(|paths| paths.contains(path)) {
            self.whole.update(|paths| {
                paths.remove(path);
            });
        }
    }

    pub fn finish_load(&self, path: &str, file: Arc<LoadedFile>) {
        self.loading.update(|m| {
            m.remove(path);
        });
        // A load from a scope since left is thrown away; the reload for
        // the current one is on its way.
        if self.scope.with(|s| *s != file.scope) {
            return;
        }
        self.files.update(|m| {
            m.insert(path.to_string(), file);
        });
        if self.active.with(|a| a.as_deref() == Some(path)) {
            self.editor_epoch.update(|e| *e += 1);
        }
    }

    /// A new changed list, from an open, a scope change, or the poll
    /// finding the head moved.
    ///
    /// Kept in the tree's order rather than git's, which puts a file
    /// beside a directory before the directory's contents: `}` and `{`
    /// walk this list, and the rail shows the tree, so the two have to
    /// agree on what comes next.
    ///
    /// The path to each file the list did not have before is opened in
    /// the tree, so the filtered rail shows every changed file rather
    /// than a row of shut directories, and the whole tree, when the
    /// reader turns to it, is open at the changes. Only the new ones: a
    /// poll that rereads the same list must not spring open what the
    /// reader shut.
    pub fn set_changed_files(&self, files: Arc<Vec<ChangedFile>>) {
        let mut sorted = files.as_ref().clone();
        sorted.sort_by(|a, b| perch_git::compare_paths(&a.path, false, &b.path, false));
        let known: std::collections::HashSet<String> = self
            .changed_files
            .with_untracked(|old| old.iter().map(|f| f.path.clone()).collect());
        let new: Vec<&str> = sorted
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| !known.contains(*p))
            .collect();
        if !new.is_empty() {
            self.expanded.update(|expanded| {
                for path in new {
                    crate::review::tree::reveal(expanded, path);
                }
            });
        }
        self.changed_files.set(Arc::new(sorted));
    }

    /// The scope changed: every loaded file was diffed under the old one.
    /// Forget them all and reload the open tabs, active one first, so the
    /// editor repaints from the new sides rather than the stale ones.
    /// Returns the paths to ask the backend for.
    pub fn change_scope(&self, scope: Scope, files: Arc<Vec<ChangedFile>>) -> Vec<String> {
        self.scope.set(scope);
        self.set_changed_files(files);
        // Every loaded file, not only the tabs: a thread card on
        // Conversation shows lines from a file no tab has, and would go
        // blank until something loaded it again.
        let mut paths: Vec<String> = self.files.with(|m| m.keys().cloned().collect());
        self.files.set(HashMap::new());
        self.loading.set(HashMap::new());
        for tab in self
            .tabs
            .with(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>())
        {
            if !paths.contains(&tab) {
                paths.push(tab);
            }
        }
        if let Some(active) = self.active.get() {
            paths.retain(|p| *p != active);
            paths.insert(0, active);
        }
        self.loading.update(|m| {
            for p in &paths {
                m.insert(p.clone(), ());
            }
        });
        self.editor_epoch.update(|e| *e += 1);
        paths
    }

    /// Whether a changed file's content has been marked viewed.
    pub fn is_viewed(&self, file: &ChangedFile) -> bool {
        match &file.head_blob {
            Some(blob) => self
                .viewed
                .with(|v| v.contains(&(file.path.clone(), blob.clone()))),
            None => false,
        }
    }

    /// How many of the changed files are viewed, and how many there are.
    ///
    /// A deletion has no content to view, so it is in neither number.
    pub fn viewed_count(&self) -> (usize, usize) {
        self.changed_files.with(|files| {
            let viewable = files.iter().filter(|f| f.head_blob.is_some());
            let viewed = viewable.clone().filter(|f| self.is_viewed(f)).count();
            (viewed, viewable.count())
        })
    }

    /// The next changed file after `from` in rail order that is not viewed,
    /// wrapping round; `None` when every file is viewed.
    pub fn next_unviewed(&self, from: Option<&str>, forward: bool) -> Option<String> {
        self.changed_files.with(|files| {
            if files.is_empty() {
                return None;
            }
            let n = files.len();
            let start = from.and_then(|p| files.iter().position(|f| f.path == p));
            let order: Vec<usize> = (1..=n)
                .map(|k| {
                    let base =
                        start
                            .map(|s| s as isize)
                            .unwrap_or(if forward { -1 } else { n as isize });
                    let k = k as isize;
                    ((base + if forward { k } else { -k }).rem_euclid(n as isize)) as usize
                })
                .collect();
            order
                .into_iter()
                .map(|i| &files[i])
                .find(|f| f.head_blob.is_some() && !self.is_viewed(f))
                .map(|f| f.path.clone())
        })
    }

    /// The head lines a file's current threads are anchored on, for the folds to
    /// keep in view, and the composer card's while it has one: the thread
    /// being written keeps its lines in view as a written one does, so a
    /// run folded again cannot close over the card. Tracked, so a document
    /// built from them follows the threads, the discussion and the
    /// composer.
    pub fn anchors_in(&self, path: &str) -> Vec<(usize, usize)> {
        let file = self.files.with(|files| files.get(path).cloned());
        let mut out = self.comment_threads.with(|threads| {
            let visible = threads
                .iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .cloned()
                .collect::<Vec<_>>();
            let mut out = anchors_of(&visible, path);
            if let Some(file) = file.as_deref() {
                out.extend(
                    visible
                        .iter()
                        .filter_map(|thread| thread.anchor.as_ref())
                        .filter(|anchor| {
                            anchor.path == path && anchor.side == perch_github::Side::Left
                        })
                        .flat_map(|anchor| aligned_head_ranges(file, anchor)),
                );
            }
            out
        });
        if let Some(a) = self.composer_card_anchor(path) {
            match a.side {
                perch_github::Side::Right => {
                    let (first, last) = (a.start_line.min(a.line), a.start_line.max(a.line));
                    out.push((
                        first.saturating_sub(1) as usize,
                        last.saturating_sub(1) as usize,
                    ));
                }
                perch_github::Side::Left => {
                    if let Some(file) = file.as_deref() {
                        out.extend(aligned_head_ranges(file, &a));
                    }
                }
            }
        }
        out
    }

    /// The cards of a file: the current threads anchored in it, the
    /// reader's own and GitHub's, and the composer when it is a card in
    /// it, each with the rows it has now. Tracked on the threads, the
    /// composer, the rows and which cards are collapsed, so a document
    /// built from them follows them all; the file itself is read as it
    /// is, since a file's content is fixed for the editor built over it.
    pub fn cards_in(&self, path: &str) -> Vec<Card> {
        let Some(file) = self.files.with_untracked(|m| m.get(path).cloned()) else {
            return Vec::new();
        };
        let composer = self.composer_card_anchor(path);
        self.comment_threads.with(|threads| {
            let visible = threads
                .iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .cloned()
                .collect::<Vec<_>>();
            let collapsed: HashSet<CardId> = visible
                .iter()
                .map(|thread| CardId::Thread(thread.id.clone()))
                .filter(|card| self.is_collapsed(card))
                .collect();
            self.card_rows
                .with(|rows| cards_of(&visible, composer.as_ref(), &file, &collapsed, rows))
        })
    }

    /// The anchor the composer's card in a file sits under, if it has
    /// one there: the open composer's, when it starts a thread on lines
    /// of the file, or else the one a sent question's card keeps until
    /// its thread arrives. Tracked on both.
    pub fn composer_card_anchor(&self, path: &str) -> Option<Anchor> {
        self.composer
            .with(|c| {
                c.as_ref()
                    .filter(|c| c.thread_id.is_none())
                    .and_then(|c| c.anchor.clone())
            })
            .or_else(|| self.sent_composer.get())
            .filter(|a| a.path == path)
    }

    /// Whether the open composer is drawn as a card in the code: it starts
    /// a thread on lines of the active file, which is loaded and has the
    /// lines. Tracked.
    pub fn composer_is_inline(&self) -> bool {
        let Some(file) = self.active_file() else {
            return false;
        };
        self.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.thread_id.is_none())
                .and_then(|c| c.anchor.as_ref())
                .is_some_and(|a| a.path == file.path && card_line(&file, a).is_some())
        })
    }

    /// Whether a follow-up on a thread is drawn inside the thread's card
    /// in the code: the thread has a card in the active file, which is
    /// loaded and has its lines. Tracked.
    pub fn follow_up_is_inline(&self, thread_id: &str) -> bool {
        let Some(file) = self.active_file() else {
            return false;
        };
        self.comment_threads.with(|threads| {
            threads
                .iter()
                .find(|t| t.id == thread_id && !t.outdated && self.thread_visible(&t.id))
                .and_then(|t| t.anchor.as_ref())
                .is_some_and(|a| a.path == file.path && card_line(&file, a).is_some())
        })
    }

    /// Whether the open composer is drawn in the code: a new thread's as a
    /// card under its lines, a follow-up's inside its thread's card. Tracked.
    pub fn composer_in_code(&self) -> bool {
        match self
            .composer
            .with(|c| c.as_ref().map(|c| c.thread_id.clone()))
        {
            Some(Some(thread_id)) => self.follow_up_is_inline(&thread_id),
            Some(None) => self.composer_is_inline(),
            None => false,
        }
    }

    /// Open a card drawn as one row, so what is inside it can be seen.
    /// Nothing is written for a card that is open already.
    pub fn expand_card(&self, card: &CardId) {
        if self.thread_resolved(card) {
            if !self.opened_cards.with_untracked(|c| c.contains(card)) {
                self.opened_cards.update(|c| {
                    c.insert(card.clone());
                });
            }
            self.forget_collapsed(card);
            return;
        }
        self.forget_collapsed(card);
    }

    /// A card the reader opened is no longer one they collapsed: a
    /// resolved thread opened and then unresolved on GitHub stays open.
    fn forget_collapsed(&self, card: &CardId) {
        if self.collapsed_cards.with_untracked(|c| c.contains(card)) {
            self.collapsed_cards.update(|c| {
                c.remove(card);
            });
        }
    }

    /// Whether a card is drawn as one row. A resolved thread's is, unless
    /// the reader opened it; any other's is when the reader collapsed it.
    pub fn is_collapsed(&self, card: &CardId) -> bool {
        if self.thread_resolved(card) {
            return !self.opened_cards.with(|c| c.contains(card));
        }
        self.collapsed_cards.with(|c| c.contains(card))
    }

    /// Whether the card is a thread's that GitHub says is resolved.
    /// Tracked on the threads.
    fn thread_resolved(&self, card: &CardId) -> bool {
        let CardId::Thread(id) = card else {
            return false;
        };
        self.comment_threads
            .with(|threads| threads.iter().any(|t| &t.id == id && t.resolved))
    }

    /// A card has been laid out, and its height takes `rows`: what the
    /// document built from the cards gives it from here on, in place of
    /// the estimate. Nothing is written when that is what it already
    /// says, since the write rebuilds the open file's document.
    pub fn set_card_rows(&self, card: &CardId, rows: usize) {
        if self
            .card_rows
            .with_untracked(|r| r.get(card) == Some(&rows))
        {
            return;
        }
        self.card_rows.update(|r| {
            r.insert(card.clone(), rows);
        });
    }

    /// The chevron was clicked: a collapsed card opens, an open one
    /// collapses.
    pub fn toggle_card(&self, card: &CardId) {
        if self.thread_resolved(card) {
            let opened = self.opened_cards.with_untracked(|c| !c.contains(card));
            self.opened_cards.update(|c| {
                if !c.remove(card) {
                    c.insert(card.clone());
                }
            });
            if opened {
                self.forget_collapsed(card);
            }
            return;
        }
        self.collapsed_cards.update(|c| {
            if !c.remove(card) {
                c.insert(card.clone());
            }
        });
    }

    /// The worktree's threads, as the backend sent them. A thread arriving
    /// for the first time since launch starts its card collapsed when it
    /// is outdated, and open otherwise; one seen before keeps whatever the
    /// reader left it as. The cards are settled before the threads are
    /// written, so the document built from the threads is built once.
    ///
    /// A sent question's card gives up its rows here, in the same pass
    /// as the threads land, so the thread's card takes the rows in the
    /// one edit rather than the code closing over them and opening
    /// again. Only a thread new since launch can be the question's, and
    /// the threads are sent after every store of the discussion too, so
    /// a batch with nothing new leaves the rows where they are.
    pub fn set_comment_threads(&self, threads: Arc<Vec<CommentThread>>) {
        let new: Vec<(CardId, bool)> = self.known_cards.with_untracked(|known| {
            threads
                .iter()
                .filter(|t| t.anchor.is_some())
                .map(|t| (CardId::Thread(t.id.clone()), t.outdated))
                .filter(|(id, _)| !known.contains(id))
                .collect()
        });
        let arrived = !new.is_empty();
        if arrived {
            let collapsed: Vec<CardId> = new
                .iter()
                .filter(|(_, outdated)| *outdated)
                .map(|(id, _)| id.clone())
                .collect();
            if !collapsed.is_empty() {
                self.collapsed_cards.update(|c| c.extend(collapsed));
            }
            self.known_cards
                .update(|k| k.extend(new.into_iter().map(|(id, _)| id)));
        }
        floem::reactive::Effect::batch(|| {
            if arrived && self.sent_composer.with_untracked(Option::is_some) {
                self.sent_composer.set(None);
                self.forget_composer_rows();
            }
            self.comment_threads.set(threads);
        });
    }

    /// The composer's laid-out rows are the card's that was, and the next
    /// composer starts on the estimate.
    fn forget_composer_rows(&self) {
        if self
            .card_rows
            .with_untracked(|r| r.contains_key(&CardId::Composer))
        {
            self.card_rows.update(|r| {
                r.remove(&CardId::Composer);
            });
        }
    }

    /// The reveals for a file's content, tracked, so a document built from
    /// them is rebuilt when they change.
    pub fn reveals_for(&self, path: &str, blob: &str) -> Reveals {
        self.reveals.with(|m| {
            m.get(&(path.to_string(), blob.to_string()))
                .cloned()
                .unwrap_or_default()
        })
    }

    /// Show head lines `first` to `last` of a file, inclusive, whatever
    /// fold they are in, and remember it for the file's content.
    pub fn reveal(&self, path: &str, blob: &str, first: usize, last: usize) {
        self.reveals.update(|m| {
            m.entry((path.to_string(), blob.to_string()))
                .or_default()
                .add(first, last);
        });
        self.queue_reveal_write(
            blob,
            RevealWrite::Reveal {
                path: path.to_string(),
                blob: blob.to_string(),
                first: first as u32,
                last: last as u32,
            },
        );
    }

    /// Reveal the lines around a head line of a folded file, for a jump
    /// into a fold: a few of context on either side, as a divider shows
    /// around a change. Nothing for a file shown whole, or one whose line
    /// is not in it. The reveal reaches the editor before this returns:
    /// the effect that rebuilds the document runs on the write.
    pub fn reveal_around(&self, file: &LoadedFile, head_line: usize) {
        let folded = floem::reactive::Effect::untrack(|| self.folded(&file.path));
        if !folded || file.buffer.display_line(head_line).is_none() {
            return;
        }
        self.reveal(
            &file.path,
            &file.blob,
            head_line.saturating_sub(doc::CONTEXT_LINES),
            head_line + doc::CONTEXT_LINES,
        );
    }

    /// Fold runs back to their default: forget every reveal overlapping
    /// each of `ranges`, the head lines of a run as the folds have it by
    /// default, here and in the store. A divider's Reset gives its own
    /// run's and those of the runs it carries; see `doc::reset_ranges`.
    pub fn reset(&self, path: &str, blob: &str, ranges: &[(usize, usize)]) {
        self.reveals.update(|m| {
            if let Some(reveals) = m.get_mut(&(path.to_string(), blob.to_string())) {
                for &(first, last) in ranges {
                    reveals.remove_overlapping(first, last);
                }
                if reveals.is_empty() {
                    m.remove(&(path.to_string(), blob.to_string()));
                }
            }
        });
        for &(first, last) in ranges {
            self.queue_reveal_write(
                blob,
                RevealWrite::Reset {
                    path: path.to_string(),
                    blob: blob.to_string(),
                    first: first as u32,
                    last: last as u32,
                },
            );
        }
    }

    /// Hand a write to the root view for the backend. A file with no blob
    /// has nothing to key a row by, so its reveals stay in memory.
    fn queue_reveal_write(&self, blob: &str, write: RevealWrite) {
        if blob.is_empty() {
            return;
        }
        self.pending_reveals.update(|w| w.push(write));
    }

    /// How many lines the open file's dividers hide between them, from the
    /// document the editor is laying out: the stacked one, or the head
    /// side of a split. Nothing when no editor is open, when the file is
    /// shown whole, or when the entry is an editor's since rebuilt.
    pub fn hidden_lines(&self) -> usize {
        self.docs
            .with(|docs| {
                let docs = docs
                    .as_ref()
                    .filter(|d| d.epoch == self.editor_epoch.get())?;
                // The document's signal lives with its editor; see `LiveDoc`.
                docs.layout
                    .head()
                    .doc
                    .try_with(|doc| doc.map(|d| d.hidden_lines()))
            })
            .unwrap_or(0)
    }

    pub fn fail_load(&self, path: &str) {
        self.loading.update(|m| {
            m.remove(path);
        });
    }

    /// Status of a path in the pull request, for the tree's change markers.
    /// Fold a run event into the thread list in place, so the running block
    /// updates without a reload.
    pub fn apply_run_event(&self, run_id: &str, event: &perch_agent::RunEvent) {
        self.comment_threads.update(|threads| {
            let mut all: Vec<CommentThread> = threads.as_ref().clone();
            for thread in &mut all {
                for message in &mut thread.messages {
                    if let Some(run) = message.run.as_mut().filter(|r| r.id == run_id) {
                        crate::ask::apply_run_event(run, event);
                    }
                }
            }
            *threads = Arc::new(all);
        });
    }

    /// The pointer came onto `row`, on the element `by`.
    pub fn enter_row(&self, by: floem::ElementId, row: usize) {
        self.hovered_line.set(Some(row));
        self.hovered_by.set(Some(by));
    }

    /// The pointer left the element `by`. The row goes when `by` is the
    /// element that recorded it, and stays when another has since: see
    /// `hovered_by`.
    pub fn leave_row(&self, by: floem::ElementId) {
        if self.hovered_by.get_untracked() == Some(by) {
            self.hovered_line.set(None);
            self.hovered_by.set(None);
        }
    }

    /// The head lines under the editor's selection, as an anchor. `None`
    /// when nothing is selected or the selection is all removed lines.
    pub fn anchor_from_selection(&self) -> Option<Anchor> {
        let (first, last) = self.selection.get()?;
        let file = self.active_file()?;
        // A file outside the worktree is a place to read, not part of the
        // review: nothing is anchored in it.
        if file.origin.is_outside() {
            return None;
        }
        self.anchor_for_display_range(&file, first, last)
    }

    /// An anchor for a display range in a file.
    pub fn anchor_for_display_range(
        &self,
        file: &LoadedFile,
        first: usize,
        last: usize,
    ) -> Option<Anchor> {
        let (first, last) = (first.min(last), first.max(last));
        let mut heads = (first..=last).filter_map(|d| file.buffer.head_line(d));
        let start = heads.next()?;
        let end = heads.next_back().unwrap_or(start);
        // The commit the lines are of. Under a commit scope that is the
        // commit, not the branch head: the numbers are its, and the quote
        // and the outdated check read from it.
        let head = self.item.with(|i| {
            i.as_ref()
                .and_then(|i| i.checkout.as_ref())
                .map(|c| c.head_sha.clone())
        })?;
        let commit_sha = match self.scope.get_untracked() {
            Scope::Commit { sha } => sha,
            _ => head,
        };
        Some(Anchor {
            path: file.path.clone(),
            start_line: start as u32 + 1,
            line: end as u32 + 1,
            side: perch_github::Side::Right,
            commit_sha,
        })
    }

    /// Open a composer, for a new thread on `anchor` or the item as a whole,
    /// or a follow-up in `thread_id`.
    ///
    /// A composer on lines of the open file is a card under them, and a
    /// follow-up on a thread with a card in the open file sits inside that
    /// card. Anything that cannot be drawn inline moves to Conversation,
    /// where every item-level thread is available. A card the follow-up
    /// sits inside is opened if it was collapsed, so the box is seen. A
    /// sent question's card gives way to the new composer, since there is
    /// one composer card at a time, and whatever rows the last composer's
    /// card had are forgotten: they were that card's, and this one starts
    /// on the estimate.
    pub fn open_composer(
        &self,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        draft: String,
        defaults: AgentChoice,
    ) {
        let thread = thread_id.as_ref().and_then(|id| {
            self.comment_threads
                .with_untracked(|t| t.iter().find(|t| &t.id == id).cloned())
        });
        let face = floem::reactive::Effect::untrack(|| {
            self.face_for(thread_id.as_deref(), anchor.as_ref())
        });
        let destination = self.agent.with_untracked(|agent| {
            default_destination(
                thread.as_ref(),
                anchor.as_ref(),
                self.last_destination[face.index()].get_untracked(),
                self.agent_last.get_untracked(),
                defaults.clone(),
                false,
                agent,
            )
        });
        // The rules say GitHub where a draft could go; where none can, on
        // a branch or a pull request no longer open, the last agent.
        let destination = if destination.is_github()
            && !self.github_offered_for(thread_id.as_deref(), anchor.as_ref())
        {
            Destination::agent(self.agent_last.get_untracked().unwrap_or(defaults))
        } else {
            destination
        };
        self.open_composer_with(thread_id, anchor, draft, destination, None);
    }

    /// The face a composer about `thread_id` or `anchor` would take: inline
    /// when Files is showing and it has a card in the code, else a card on
    /// Conversation. Tracked, as the placement checks are.
    fn face_for(
        &self,
        thread_id: Option<&str>,
        anchor: Option<&Anchor>,
    ) -> crate::review::threads::ThreadFace {
        use crate::review::threads::ThreadFace;
        if self.tab.get() == ItemTab::Conversation {
            return ThreadFace::Card;
        }
        let in_code = match thread_id {
            Some(id) => self.follow_up_is_inline(id),
            None => match (self.active_file(), anchor) {
                (Some(file), Some(a)) => a.path == file.path && card_line(&file, a).is_some(),
                _ => false,
            },
        };
        if in_code {
            ThreadFace::Inline
        } else {
            ThreadFace::Card
        }
    }

    /// Open a composer on a destination the caller chose: Ask about this
    /// goes to an agent whatever the thread's last message was, and a
    /// draft's edit goes back to GitHub with `editing` set to it.
    pub fn open_composer_with(
        &self,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        draft: String,
        destination: Destination,
        editing: Option<String>,
    ) {
        self.composer_draft.set(draft.clone());
        // A confirm or a refusal was about the composer this replaces.
        self.composer_confirming.set(false);
        self.composer_error.set(None);
        let opening = self.composer_opened.get_untracked() + 1;
        self.composer_opened.set(opening);
        floem::reactive::Effect::batch(|| {
            if self.sent_composer.with_untracked(Option::is_some) {
                self.sent_composer.set(None);
            }
            self.forget_composer_rows();
            self.composer.set(Some(Composer {
                thread_id,
                anchor,
                destination,
                draft,
                opening,
                editing,
            }));
        });
        let in_code = self.composer_in_code_untracked();
        if !in_code && self.tab.get_untracked() != ItemTab::Conversation {
            self.show_tab(ItemTab::Conversation);
        } else if let Some(id) = self
            .composer
            .with_untracked(|c| c.as_ref().and_then(|c| c.thread_id.clone()))
        {
            self.expand_card(&CardId::Thread(id));
        }
    }

    /// Close the composer and forget its draft. A composer card's rows go
    /// with it.
    pub fn close_composer(&self) {
        floem::reactive::Effect::batch(|| {
            self.composer.set(None);
            self.forget_composer_rows();
        });
        self.composer_draft.set(String::new());
        self.composer_confirming.set(false);
        self.composer_error.set(None);
        // The box goes with the composer and is never told it lost focus,
        // so the flag that stands the keys down is cleared here.
        self.composer_focused.set(false);
    }

    /// The composer's question went to the backend, and the composer is
    /// closed. A composer that was a card in the code leaves its rows
    /// where they are, under its anchor, for the thread's card to take
    /// when the thread arrives; see `sent_composer`.
    pub fn composer_sent(&self) {
        let card = floem::reactive::Effect::untrack(|| self.composer_is_inline())
            .then(|| {
                self.composer
                    .with_untracked(|c| c.as_ref().and_then(|c| c.anchor.clone()))
            })
            .flatten();
        floem::reactive::Effect::batch(|| {
            self.composer.set(None);
            match card {
                Some(anchor) => self.sent_composer.set(Some(anchor)),
                None => self.forget_composer_rows(),
            }
        });
        self.composer_draft.set(String::new());
        self.composer_confirming.set(false);
        self.composer_error.set(None);
        self.composer_focused.set(false);
    }

    /// Change where the composer's text goes. The confirm, if one was up,
    /// was about the old destination and goes.
    pub fn set_destination(&self, destination: Destination) {
        self.composer.update(|c| {
            if let Some(c) = c {
                c.destination = destination;
            }
        });
        self.composer_confirming.set(false);
    }

    /// The open composer's text is on its way to the pull request's
    /// conversation as an issue comment. That composer stays open and
    /// still until `settle_issue_comment` says how it went.
    pub fn start_posting(&self, token: u64, body: String) {
        let opening = self
            .composer
            .with_untracked(|c| c.as_ref().map(|c| c.opening))
            .unwrap_or_default();
        self.composer_confirming.set(false);
        self.composer_error.set(None);
        self.posting.set(Some(Posting {
            token,
            opening,
            body,
        }));
    }

    /// Whether the composer opened as `opening` has an issue comment in
    /// flight, and so is still. Tracked.
    pub fn posting_from(&self, opening: u64) -> bool {
        self.posting
            .with(|p| p.as_ref().is_some_and(|p| p.opening == opening))
    }

    /// The backend answered the issue comment in flight. Posted, the
    /// composer that sent it closes and the comment is in the timeline
    /// through the discussion the backend already sent; refused, the
    /// text stays in its box with the reason under it. An answer to a
    /// send that is not the one in flight is ignored, and one whose
    /// composer was put away since only clears the flight.
    pub fn settle_issue_comment(&self, token: u64, result: Result<(), String>) {
        let sent_from = self.posting.with_untracked(|p| {
            p.as_ref()
                .filter(|p| p.token == token)
                .map(|p| p.opening)
        });
        let Some(sent_from) = sent_from else {
            return;
        };
        self.posting.set(None);
        let still_open = self
            .composer
            .with_untracked(|c| c.as_ref().is_some_and(|c| c.opening == sent_from));
        if !still_open {
            return;
        }
        match result {
            Ok(()) => self.composer_sent(),
            Err(message) => self.composer_error.set(Some(message)),
        }
    }

    /// Whether the composer's text, sent to GitHub, posts at once as an
    /// issue comment: it is about the item as a whole, at the foot of the
    /// conversation or under a thread that has neither lines nor a place
    /// on GitHub, so there is nothing to reply under and nothing to draft
    /// for the review. Untracked.
    pub fn posts_issue_comment(&self, composer: &Composer) -> bool {
        self.posts_issue_comment_about(composer.thread_id.as_deref(), composer.anchor.as_ref())
    }

    /// `posts_issue_comment` for a composer about `thread_id` or `anchor`.
    fn posts_issue_comment_about(&self, thread_id: Option<&str>, anchor: Option<&Anchor>) -> bool {
        if anchor.is_some() {
            return false;
        }
        match thread_id {
            None => true,
            Some(id) => self.comment_threads.with_untracked(|t| {
                t.iter()
                    .find(|t| t.id == id)
                    .is_some_and(|t| t.anchor.is_none() && t.github_id.is_none())
            }),
        }
    }

    /// The backend complained about something, which may be the send
    /// whose thread a sent question's card is waiting for. The card's rows
    /// are given up, so a failed send does not leave blank rows under the
    /// lines; a thread that does arrive after all brings its own card.
    pub fn drop_sent_placeholder(&self) {
        if self.sent_composer.with_untracked(Option::is_none) {
            return;
        }
        floem::reactive::Effect::batch(|| {
            self.sent_composer.set(None);
            self.forget_composer_rows();
        });
    }

    /// Whether GitHub is a destination for `composer`: it has lines to
    /// land on or its thread is on GitHub, and the pull request is open
    /// to take the review the draft would ride in; or it is about a pull
    /// request as a whole, where it posts as an issue comment. About an
    /// issue, or lines of a branch, there is nowhere to go. Untracked.
    pub fn github_offered(&self, composer: &Composer) -> bool {
        self.github_offered_for(composer.thread_id.as_deref(), composer.anchor.as_ref())
    }

    /// `github_offered` for a composer about `thread_id` or `anchor`.
    fn github_offered_for(&self, thread_id: Option<&str>, anchor: Option<&Anchor>) -> bool {
        let open_pull = self
            .pull_request
            .with_untracked(|pull| pull.as_ref().is_some_and(|pull| pull.is_open()));
        if anchor.is_some() {
            return open_pull;
        }
        let on_github = thread_id.is_some_and(|id| {
            self.comment_threads.with_untracked(|t| {
                t.iter()
                    .any(|t| t.id == id && t.github_id.is_some())
            })
        });
        if on_github {
            return open_pull;
        }
        self.pull_request.with_untracked(Option::is_some)
            && self.posts_issue_comment_about(thread_id, anchor)
    }

    /// Whether `thread` can take a draft: it has lines to land on or is
    /// on GitHub, and the item is a pull request that is still open. What
    /// Post to GitHub on an agent's message is offered on. A tracked read
    /// of the pull request, which lands after the threads do.
    pub fn takes_draft(&self, thread: &CommentThread) -> bool {
        (thread.anchor.is_some() || thread.github_id.is_some())
            && self
                .pull_request
                .with(|pull| pull.as_ref().is_some_and(|pull| pull.is_open()))
    }

    /// Every draft in the open item, in thread order and time order within
    /// a thread: what the review form lists and the gutter marks. A
    /// tracked read over the threads.
    pub fn drafts(&self) -> Vec<Draft> {
        self.comment_threads.with(|threads| drafts_of(threads))
    }

    /// The thread whose anchor starts on a display line of the open file.
    /// A left-side anchor starts on its base row: an exact ghost when the
    /// line was changed or removed, otherwise the head row paired with it.
    pub fn thread_starting_at(
        &self,
        file: &LoadedFile,
        display_line: usize,
    ) -> Option<CommentThread> {
        self.comment_threads.with(|t| {
            t.iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .find(|t| {
                    t.anchor.as_ref().is_some_and(|a| {
                        a.path == file.path
                            && display_line_of_anchor_line(file, a.side, a.start_line)
                                == Some(display_line)
                    })
                })
                .cloned()
        })
    }

    /// The current thread whose anchor starts on a zero-based line of one
    /// particular side. Split gutters use this so a paired row does not show
    /// a left-side marker in the head gutter, or vice versa.
    pub fn thread_starting_on(
        &self,
        file: &LoadedFile,
        side: perch_github::Side,
        line: usize,
    ) -> Option<CommentThread> {
        self.comment_threads.with(|threads| {
            threads
                .iter()
                .filter(|thread| !thread.outdated && self.thread_visible(&thread.id))
                .find(|thread| {
                    thread.anchor.as_ref().is_some_and(|anchor| {
                        anchor.path == file.path
                            && anchor.side == side
                            && anchor.start_line.checked_sub(1).map(|line| line as usize)
                                == Some(line)
                    })
                })
                .cloned()
        })
    }

    pub fn thread_visible(&self, thread_id: &str) -> bool {
        !self
            .hidden_review_threads
            .with(|hidden| hidden.contains(thread_id))
    }

    pub fn apply_review_finding_filter(
        &self,
        filter: ReviewFindingFilter,
        findings: &[ReviewFindingSummary],
    ) {
        let hidden = match filter {
            ReviewFindingFilter::Open => findings
                .iter()
                .filter(|summary| !summary.is_actionable())
                .map(|summary| summary.finding.thread_id.clone())
                .collect(),
            ReviewFindingFilter::All => HashSet::new(),
        };
        self.review_finding_filter.set(filter);
        self.hidden_review_threads.set(hidden);
    }

    pub fn status_of(&self, path: &str) -> Option<ChangeStatus> {
        self.changed_files
            .with(|files| files.iter().find(|f| f.path == path).map(|f| f.status))
    }
}

/// A file's cards: the current threads anchored in it, the reader's own
/// and GitHub's alike, oldest first, so two under one line stack in the
/// order they were made; then the composer, when `composer` is the anchor
/// of its card in the file. A card sits under the last line of its
/// anchor, after any ghost spliced under that line, and takes the rows in
/// `rows` when the card has been laid out, else the estimate, and one row
/// when collapsed. A thread whose last line the file does not have,
/// because its anchor has gone stale past the end, has nowhere to sit and
/// gets no card.
pub fn cards_of(
    threads: &[CommentThread],
    composer: Option<&Anchor>,
    file: &LoadedFile,
    collapsed: &HashSet<CardId>,
    rows: &HashMap<CardId, usize>,
) -> Vec<Card> {
    let rows_for = |id: &CardId, estimate: &dyn Fn(bool) -> usize| {
        let is_collapsed = collapsed.contains(id);
        match rows.get(id) {
            Some(&rows) if !is_collapsed => rows,
            _ => estimate(is_collapsed),
        }
    };

    let mut anchored: Vec<&CommentThread> = threads
        .iter()
        .filter(|t| !t.outdated)
        .filter(|t| t.anchor.as_ref().is_some_and(|a| a.path == file.path))
        .collect();
    anchored.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut out: Vec<Card> = anchored
        .into_iter()
        .filter_map(|t| {
            let after_display_line = card_line(file, t.anchor.as_ref()?)?;
            let id = CardId::Thread(t.id.clone());
            let rows = rows_for(&id, &|collapsed| doc::estimate_rows(t, collapsed));
            Some(Card {
                id,
                after_display_line,
                rows,
            })
        })
        .collect();

    if let Some(after_display_line) = composer.and_then(|a| card_line(file, a)) {
        let rows = rows
            .get(&CardId::Composer)
            .copied()
            .unwrap_or(doc::COMPOSER_ROWS);
        out.push(Card {
            id: CardId::Composer,
            after_display_line,
            rows,
        });
    }
    out
}

/// The display line a card for an anchor goes after. A right-side card goes
/// after its head line and any deletion belonging under that line. A
/// left-side card goes after the exact base row: its ghost when changed or
/// removed, or the aligned head row when unchanged. `None` when the file
/// does not have the line on that side.
fn card_line(file: &LoadedFile, anchor: &Anchor) -> Option<usize> {
    let last = anchor.start_line.max(anchor.line);
    match anchor.side {
        perch_github::Side::Left => {
            let display = display_line_of_anchor_line(file, anchor.side, last)?;
            match file.buffer.row(display)? {
                Row::Deleted { .. } => Some(display),
                Row::Head { head_line, .. } => display_line_with_ghosts(&file.buffer, head_line),
            }
        }
        perch_github::Side::Right => {
            let last = last.checked_sub(1)? as usize;
            display_line_with_ghosts(&file.buffer, last)
        }
    }
}

/// Where a one-based line on one side of the diff appears in the unified
/// display. Head lines map directly. A base line prefers its ghost because a
/// rewritten line also has a paired head row; an unchanged base line has no
/// ghost and maps through the split alignment instead.
fn display_line_of_anchor_line(
    file: &LoadedFile,
    side: perch_github::Side,
    line: u32,
) -> Option<usize> {
    let line = line.checked_sub(1)? as usize;
    match side {
        perch_github::Side::Right => file.buffer.display_line(line),
        perch_github::Side::Left => {
            if let Some(display) = file
                .buffer
                .rows
                .iter()
                .position(|row| matches!(row, Row::Deleted { base_line } if *base_line == line))
            {
                return Some(display);
            }
            let base_line = line + 1;
            let head_line = file
                .alignment
                .iter()
                .find_map(|&(base, head)| (base == Some(base_line)).then_some(head).flatten())?;
            file.buffer.display_line(head_line.checked_sub(1)?)
        }
    }
}

/// The head ranges corresponding to a left-side anchor, for folds. Removed
/// base lines have no head range and need none: ghosts are changed rows and
/// are never folded. Consecutive aligned head lines stay one range.
fn aligned_head_ranges(file: &LoadedFile, anchor: &Anchor) -> Vec<(usize, usize)> {
    let first = anchor.start_line.min(anchor.line) as usize;
    let last = anchor.start_line.max(anchor.line) as usize;
    let mut out: Vec<(usize, usize)> = Vec::new();
    for head in file.alignment.iter().filter_map(|&(base, head)| {
        let base = base?;
        (first..=last)
            .contains(&base)
            .then_some(head)
            .flatten()?
            .checked_sub(1)
    }) {
        match out.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(head) => *end = head,
            _ => out.push((head, head)),
        }
    }
    out
}

/// The display line a card under a head line goes after: the line's own,
/// or the last of the ghosts spliced under it. The buffer splices a run
/// of removed lines above the head line that follows them, so the run
/// after a line is under it when that next line is unchanged, or there
/// is none: lines removed and nothing put in their place. When the next
/// line is changed, the run is its rewrite's old side, which the split
/// pairs with it, and the card goes before the run on both layouts.
fn display_line_with_ghosts(buffer: &DisplayBuffer, head_line: usize) -> Option<usize> {
    let display = buffer.display_line(head_line)?;
    let ghosts = (display + 1..)
        .take_while(|&d| matches!(buffer.row(d), Some(Row::Deleted { .. })))
        .count();
    let next = buffer.row(display + 1 + ghosts);
    let under = next.is_none_or(|r| r.status() == Some(LineStatus::Unchanged));
    Some(if under { display + ghosts } else { display })
}

/// The head lines current threads are anchored on in a file, as inclusive
/// zero-based ranges, which the folds keep in view: each from its first
/// line to its last, the reader's own and GitHub's alike. One on the base
/// side has nowhere in the head to be and does not count.
pub fn anchors_of(threads: &[CommentThread], path: &str) -> Vec<(usize, usize)> {
    threads
        .iter()
        .filter(|t| !t.outdated)
        .filter_map(|t| t.anchor.as_ref())
        .filter(|a| a.path == path && a.side == perch_github::Side::Right)
        .map(|a| {
            let (first, last) = (a.start_line.min(a.line), a.start_line.max(a.line));
            (
                first.saturating_sub(1) as usize,
                last.saturating_sub(1) as usize,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, blob: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: if blob.is_some() {
                ChangeStatus::Modified
            } else {
                ChangeStatus::Deleted
            },
            additions: 1,
            deletions: 1,
            binary: false,
            head_blob: blob.map(String::from),
        }
    }

    /// Three files with content and one deletion, with `b` viewed.
    fn state() -> ReviewState {
        let state = ReviewState::new();
        state.changed_files.set(Arc::new(vec![
            changed("a.rs", Some("blob-a")),
            changed("b.rs", Some("blob-b")),
            changed("gone.rs", None),
            changed("c.rs", Some("blob-c")),
        ]));
        state.viewed.set(Arc::new(
            [("b.rs".to_string(), "blob-b".to_string())]
                .into_iter()
                .collect(),
        ));
        state
    }

    #[test]
    fn viewed_is_by_path_and_blob_and_a_deletion_never_is() {
        let state = state();
        let files = state.changed_files.get();
        assert!(state.is_viewed(&files[1]));
        assert!(!state.is_viewed(&files[0]));
        assert!(
            !state.is_viewed(&changed("b.rs", Some("blob-b2"))),
            "a new blob is new content"
        );
        assert!(!state.is_viewed(&files[2]));
        // The deletion has nothing to view, so it is not counted.
        assert_eq!(state.viewed_count(), (1, 3));
    }

    #[test]
    fn next_unviewed_skips_viewed_files_and_wraps_both_ways() {
        let state = state();
        // b is viewed and the deletion has nothing to view, so both are
        // stepped over.
        assert_eq!(
            state.next_unviewed(Some("a.rs"), true).as_deref(),
            Some("c.rs")
        );
        assert_eq!(
            state.next_unviewed(Some("c.rs"), true).as_deref(),
            Some("a.rs"),
            "wraps forward"
        );
        assert_eq!(
            state.next_unviewed(Some("a.rs"), false).as_deref(),
            Some("c.rs"),
            "wraps back"
        );
        assert_eq!(
            state.next_unviewed(Some("gone.rs"), false).as_deref(),
            Some("a.rs")
        );
        // No open file: the first from either end.
        assert_eq!(state.next_unviewed(None, true).as_deref(), Some("a.rs"));
        assert_eq!(state.next_unviewed(None, false).as_deref(), Some("c.rs"));
        // A file outside the list counts as none open.
        assert_eq!(
            state.next_unviewed(Some("elsewhere.rs"), true).as_deref(),
            Some("a.rs")
        );
    }

    #[test]
    fn a_new_changed_list_is_in_tree_order_and_opens_the_way_to_what_is_new() {
        let state = ReviewState::new();
        // Git's order: bytewise, so src/main.rs comes before src/ui/.
        state.set_changed_files(Arc::new(vec![
            changed("src/main.rs", Some("blob-m")),
            changed("src/ui/row.rs", Some("blob-r")),
            changed("README.md", Some("blob-d")),
        ]));
        let order: Vec<String> = state
            .changed_files
            .with(|f| f.iter().map(|f| f.path.clone()).collect());
        assert_eq!(
            order,
            ["src/ui/row.rs", "src/main.rs", "README.md"],
            "the tree's order: ui/ before main.rs"
        );
        assert_eq!(
            state.next_unviewed(None, true).as_deref(),
            Some("src/ui/row.rs"),
            "the next key starts at the top row"
        );
        let expanded = state.expanded.get();
        assert!(expanded.contains("src") && expanded.contains("src/ui"));

        // The reader shuts src/ui, and the poll rereads the same list.
        state
            .expanded
            .update(|e| crate::review::tree::toggle(e, "src/ui"));
        state.set_changed_files(state.changed_files.get());
        assert!(
            !state.expanded.get().contains("src/ui"),
            "nothing new, nothing reopened"
        );

        // A file in a new directory opens the way to it, and only it.
        state.set_changed_files(Arc::new(vec![
            changed("src/main.rs", Some("blob-m")),
            changed("src/ui/row.rs", Some("blob-r")),
            changed("docs/guide.md", Some("blob-g")),
        ]));
        let expanded = state.expanded.get();
        assert!(expanded.contains("docs"));
        assert!(!expanded.contains("src/ui"));
    }

    /// Showing an opened file in the tree opens the way to it and asks
    /// the rail to scroll to it; a file the filter hides turns the filter
    /// off, and one with no row does nothing.
    #[test]
    fn showing_a_file_in_the_tree_turns_the_filter_off_when_it_hides_the_file() {
        let state = ReviewState::new();
        state.tree.set(Arc::new(
            [
                "src",
                "src/a.rs",
                "src/untouched.rs",
                "docs",
                "docs/guide.md",
            ]
            .into_iter()
            .map(|path| perch_git::TreeEntry::new(path, !path.contains('.')))
            .collect(),
        ));
        state.set_changed_files(Arc::new(vec![
            changed("src/a.rs", Some("blob-a")),
            changed("src/gone.rs", None),
        ]));
        state
            .expanded
            .update(|e| crate::review::tree::toggle(e, "src"));
        assert!(state.changed_only.get(), "the filter starts on");

        // A changed file is in the filtered tree: the filter stays.
        state.show_in_tree("src/a.rs");
        assert!(state.changed_only.get());
        assert!(state.expanded.get().contains("src"), "the way to it opened");
        assert_eq!(state.scroll_to_row.get().as_deref(), Some("src/a.rs"));
        // So is a deleted one, though the worktree lacks it.
        state.scroll_to_row.set(None);
        state.show_in_tree("src/gone.rs");
        assert!(state.changed_only.get());
        assert_eq!(state.scroll_to_row.get().as_deref(), Some("src/gone.rs"));

        // A file the branch left alone is not, so the whole worktree shows.
        state.show_in_tree("docs/guide.md");
        assert!(!state.changed_only.get(), "the filter came off");
        assert!(state.expanded.get().contains("docs"));
        assert_eq!(state.scroll_to_row.get().as_deref(), Some("docs/guide.md"));

        // Nothing to show for a file with no row: one the listing lacks,
        // or one outside the worktree.
        state.changed_only.set(true);
        state.scroll_to_row.set(None);
        state.show_in_tree("target/out.rs");
        state.show_in_tree("/Users/me/.cargo/registry/src/index/tokio-1.40.0/src/lib.rs");
        assert!(state.changed_only.get(), "the filter stands");
        assert_eq!(state.scroll_to_row.get(), None);
        assert!(!state.expanded.get().contains("target"));

        // With nothing changed the tree is whole already, and the flag is
        // the reader's to keep.
        state.set_changed_files(Arc::new(Vec::new()));
        state.show_in_tree("src/untouched.rs");
        assert!(state.changed_only.get());
        assert_eq!(state.scroll_to_row.get().as_deref(), Some("src/untouched.rs"));
    }

    #[test]
    fn the_anchors_in_a_file_are_its_threads_head_side_lines() {
        use perch_github::{CommentThread, Side, ThreadOrigin};
        let at = "2026-09-18T09:59:00Z".parse().unwrap();
        let thread = |id: &str, path: &str, start_line: u32, line: u32, side: Side, outdated: bool| CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: if id.starts_with("gh-") { ThreadOrigin::GitHub } else { ThreadOrigin::Local },
            github_id: id.strip_prefix("gh-").and_then(|n| n.parse().ok()),
            anchor: Some(Anchor {
                path: path.into(),
                start_line,
                line,
                side,
                commit_sha: "h".into(),
            }),
            outdated,
            resolved: false,
            created_at: at,
            messages: Vec::new(),
        };
        let mut worktree = thread("w", "", 0, 0, Side::Right, false);
        worktree.anchor = None;
        let threads = vec![
            thread("a", "src/x.rs", 41, 58, Side::Right, false),
            thread("b", "src/x.rs", 7, 7, Side::Right, true),
            thread("c", "src/y.rs", 1, 3, Side::Right, false),
            thread("gh-1", "src/x.rs", 100, 100, Side::Right, false),
            thread("gh-2", "src/x.rs", 200, 200, Side::Left, false),
            thread("gh-3", "src/x.rs", 300, 300, Side::Right, true),
            thread("gh-4", "src/y.rs", 9, 9, Side::Right, false),
            worktree,
        ];
        // Zero based and inclusive: the current thread over 41 to 58 and
        // GitHub's on 100. Not the outdated threads, the base side, or
        // anything from the other file or the worktree.
        assert_eq!(anchors_of(&threads, "src/x.rs"), vec![(40, 57), (99, 99)]);
        assert_eq!(anchors_of(&threads, "src/z.rs"), Vec::<(usize, usize)>::new());

        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(threads));
        assert_eq!(state.anchors_in("src/y.rs"), vec![(0, 2), (8, 8)]);
    }

    /// A thread of the reader's own anchored on head lines `start` to
    /// `line` of `src/x.rs`, one-based, asked at `at`.
    fn anchored(id: &str, start: u32, line: u32, at: &str, outdated: bool) -> CommentThread {
        use perch_github::{Author, CommentMessage, Side, ThreadOrigin};
        let at = at.parse().unwrap();
        CommentThread {
            id: id.into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: start,
                line,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated,
            resolved: false,
            created_at: at,
            messages: vec![CommentMessage {
                id: format!("{id}-m1"),
                thread_id: id.into(),
                origin: ThreadOrigin::Local,
                author: Author::User,
                body: "why?".into(),
                created_at: at,
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            }],
        }
    }

    #[test]
    fn a_files_cards_sit_under_their_anchors_last_line_past_its_ghosts_oldest_first() {
        // base a b c d e f g; head a B c e F g: b and f rewritten, d
        // removed. Display: a, ghost(b), B, c, ghost(d), e, ghost(f), F, g.
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nB\nc\ne\nF\ng\n".into(),
            "a\nb\nc\nd\ne\nf\ng\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let threads = vec![
            // Asked later, on c, whose ghost of d is spliced under it:
            // nothing took d's place, so the card goes after the ghost.
            anchored("later", 3, 3, "2026-09-18T10:00:00Z", false),
            // Asked first, over a to B; B's ghost is above it, not under.
            anchored("first", 1, 2, "2026-09-18T09:00:00Z", false),
            // Asked last, on e: the ghost after it is F's old side, which
            // the split pairs with F, so the card goes before it.
            anchored("last", 4, 4, "2026-09-18T11:00:00Z", false),
            // Past the end of the file: nowhere to sit.
            anchored("gone", 9, 9, "2026-09-18T09:30:00Z", true),
        ];
        let mut elsewhere = anchored("elsewhere", 1, 1, "2026-09-18T08:00:00Z", false);
        elsewhere.anchor.as_mut().unwrap().path = "src/y.rs".into();
        let mut worktree = anchored("worktree", 1, 1, "2026-09-18T08:00:00Z", false);
        worktree.anchor = None;
        let threads = [threads, vec![elsewhere, worktree]].concat();

        let cards = cards_of(&threads, None, &file, &HashSet::new(), &HashMap::new());
        let estimate = doc::estimate_rows(&threads[0], false);
        assert_eq!(
            cards,
            vec![
                Card {
                    id: CardId::Thread("first".into()),
                    after_display_line: 2,
                    rows: estimate
                },
                Card {
                    id: CardId::Thread("later".into()),
                    after_display_line: 4,
                    rows: estimate
                },
                Card {
                    id: CardId::Thread("last".into()),
                    after_display_line: 5,
                    rows: estimate
                },
            ]
        );
        // A run removed from the end of the file is under the last line.
        let cut = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\n".into(),
            "a\nb\nc\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let on_a = [anchored("end", 1, 1, "2026-09-18T10:00:00Z", false)];
        assert_eq!(
            cards_of(&on_a, None, &cut, &HashSet::new(), &HashMap::new())[0]
                .after_display_line,
            2
        );

        // Collapsed, a card is one row; laid out, it has the rows the
        // layout gave it, unless it is collapsed.
        let collapsed: HashSet<CardId> = [CardId::Thread("first".into())].into_iter().collect();
        let rows: HashMap<CardId, usize> = [
            (CardId::Thread("first".into()), 9),
            (CardId::Thread("later".into()), 7),
        ]
        .into_iter()
        .collect();
        let cards = cards_of(&threads, None, &file, &collapsed, &rows);
        assert_eq!(cards[0].rows, doc::COLLAPSED_ROWS);
        assert_eq!(cards[1].rows, 7);

        let state = ReviewState::new();
        assert!(state.cards_in("src/x.rs").is_empty(), "no file, no cards");
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        state.comment_threads.set(Arc::new(threads));
        assert_eq!(state.cards_in("src/x.rs").len(), 3);
        state.toggle_card(&CardId::Thread("later".into()));
        assert!(state.is_collapsed(&CardId::Thread("later".into())));
        assert_eq!(state.cards_in("src/x.rs")[1].rows, 1);
        state.toggle_card(&CardId::Thread("later".into()));
        assert_eq!(state.cards_in("src/x.rs")[1].rows, estimate);
    }

    #[test]
    fn base_side_anchors_use_ghosts_and_alignment_for_markers_and_cards() {
        use perch_github::Side;

        // base a b c d e f g; head a B c e F g. The old b and f are
        // rewritten ghosts, d is a deleted ghost, and unchanged e and g
        // have shifted head line numbers.
        let file = spliced_file();
        let on_side = |id: &str, line: u32, side: Side, at: &str| {
            let mut thread = anchored(id, line, line, at, false);
            thread.anchor.as_mut().unwrap().side = side;
            thread
        };
        let threads = vec![
            on_side("left-rewrite", 2, Side::Left, "2026-09-18T08:00:00Z"),
            on_side("right-rewrite", 2, Side::Right, "2026-09-18T09:00:00Z"),
            on_side("left-deleted", 4, Side::Left, "2026-09-18T10:00:00Z"),
            on_side("left-unchanged", 5, Side::Left, "2026-09-18T11:00:00Z"),
            on_side("left-shifted", 7, Side::Left, "2026-09-18T12:00:00Z"),
        ];

        let cards = cards_of(&threads, None, &file, &HashSet::new(), &HashMap::new());
        let placed: Vec<(CardId, usize)> = cards
            .iter()
            .map(|card| (card.id.clone(), card.after_display_line))
            .collect();
        assert_eq!(
            placed,
            vec![
                (CardId::Thread("left-rewrite".into()), 1),
                (CardId::Thread("right-rewrite".into()), 2),
                (CardId::Thread("left-deleted".into()), 4),
                (CardId::Thread("left-unchanged".into()), 5),
                (CardId::Thread("left-shifted".into()), 8),
            ]
        );

        // A base line that is unchanged still owns a pure-deletion run
        // immediately below it, just as the corresponding head line does.
        let cut = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\n".into(),
            "a\nb\nc\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let left_on_a = [on_side(
            "left-before-cut",
            1,
            Side::Left,
            "2026-09-18T13:00:00Z",
        )];
        assert_eq!(
            cards_of(
                &left_on_a,
                None,
                &cut,
                &HashSet::new(),
                &HashMap::new()
            )[0]
            .after_display_line,
            2
        );

        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(threads));
        let id_at = |display| {
            state
                .thread_starting_at(&file, display)
                .map(|thread| thread.id)
        };
        assert_eq!(id_at(1).as_deref(), Some("left-rewrite"));
        assert_eq!(id_at(2).as_deref(), Some("right-rewrite"));
        assert_eq!(id_at(4).as_deref(), Some("left-deleted"));
        assert_eq!(id_at(5).as_deref(), Some("left-unchanged"));
        assert_eq!(id_at(8).as_deref(), Some("left-shifted"));
        assert_eq!(
            state
                .thread_starting_on(&file, Side::Left, 1)
                .map(|thread| thread.id),
            Some("left-rewrite".into())
        );
        assert_eq!(
            state
                .thread_starting_on(&file, Side::Right, 1)
                .map(|thread| thread.id),
            Some("right-rewrite".into())
        );
    }

    #[test]
    fn base_side_anchor_ranges_use_aligned_head_lines_for_folds() {
        use perch_github::Side;

        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "new\na\nb\nc\n".into(),
            "a\nb\nc\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        );
        let mut on_base_b = anchored("left", 2, 2, "2026-09-18T10:00:00Z", false);
        on_base_b.anchor.as_mut().unwrap().side = Side::Left;

        let state = ReviewState::new();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), Arc::new(file));
        });
        state.comment_threads.set(Arc::new(vec![on_base_b]));

        assert_eq!(state.anchors_in("src/x.rs"), vec![(2, 2)]);
    }

    #[test]
    fn outdated_reader_threads_stay_off_inline_surfaces() {
        let file = spliced_file();
        let threads = vec![
            anchored("outdated", 1, 1, "2026-09-18T09:00:00Z", true),
            anchored("current", 3, 3, "2026-09-18T10:00:00Z", false),
        ];

        let cards = cards_of(&threads, None, &file, &HashSet::new(), &HashMap::new());
        let card_ids: Vec<CardId> = cards.iter().map(|card| card.id.clone()).collect();
        assert_eq!(card_ids, vec![CardId::Thread("current".into())]);
        assert_eq!(anchors_of(&threads, "src/x.rs"), vec![(2, 2)]);

        let state = ReviewState::new();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), Arc::new(file));
        });
        state.active.set(Some("src/x.rs".into()));
        state.comment_threads.set(Arc::new(threads));
        let active = state.active_file().unwrap();

        assert!(state.thread_starting_at(&active, 0).is_none());
        assert_eq!(
            state.thread_starting_at(&active, 3).map(|thread| thread.id),
            Some("current".into())
        );
        let card_ids: Vec<CardId> = state
            .cards_in("src/x.rs")
            .iter()
            .map(|card| card.id.clone())
            .collect();
        assert_eq!(card_ids, vec![CardId::Thread("current".into())]);
        assert_eq!(state.anchors_in("src/x.rs"), vec![(2, 2)]);
        assert!(!state.follow_up_is_inline("outdated"));

        // A reply to an outdated thread has no inline card, so it moves to
        // Conversation without losing what was typed.
        state.show_tab(ItemTab::Files);
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        let outdated_anchor = state
            .comment_threads
            .with(|threads| threads[0].anchor.clone());
        state.open_composer(
            Some("outdated".into()),
            outdated_anchor,
            "reply".into(),
            choice,
        );
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "reply");
    }

    /// One of GitHub's threads on head line `line` of `src/x.rs`,
    /// one-based, with `replies` comments under its first.
    fn github_thread(id: u64, line: u32, side: perch_github::Side, at: &str, replies: usize) -> CommentThread {
        use perch_github::{Author, CommentMessage, ThreadOrigin};
        let at = at.parse().unwrap();
        let thread_id = format!("gh-{id}");
        CommentThread {
            id: thread_id.clone(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::GitHub,
            github_id: Some(id),
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: line,
                line,
                side,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            created_at: at,
            messages: (0..=replies)
                .map(|i| CommentMessage {
                    id: format!("gh-{}", id + i as u64),
                    thread_id: thread_id.clone(),
                    origin: ThreadOrigin::GitHub,
                    author: Author::GitHubUser("octocat".into()),
                    body: "nit".into(),
                    created_at: at,
                    run: None,
                    github_id: Some(id + i as u64),
                    node_id: None,
                    html_url: None,
                    diff_hunk: None,
                    reactions: Default::default(),
                })
                .collect(),
        }
    }

    /// The file of `a_files_cards_sit_under_their_anchors_last_line_past_its_ghosts_oldest_first`.
    fn spliced_file() -> LoadedFile {
        crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nB\nc\ne\nF\ng\n".into(),
            "a\nb\nc\nd\ne\nf\ng\n".into(),
            Some(ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        )
    }

    #[test]
    fn githubs_threads_and_the_composer_are_cards_with_the_readers_under_the_same_line() {
        use perch_github::Side;
        let file = spliced_file();
        // The reader's on c, two of GitHub's on c around it in time, one
        // on e, and one on the base side, which sits on the ghost.
        let threads = vec![
            anchored("mine", 3, 3, "2026-09-18T10:00:00Z", false),
            github_thread(20, 3, Side::Right, "2026-09-18T11:00:00Z", 0),
            github_thread(10, 3, Side::Right, "2026-09-18T09:00:00Z", 1),
            github_thread(30, 4, Side::Right, "2026-09-18T09:30:00Z", 0),
            github_thread(40, 2, Side::Left, "2026-09-18T09:40:00Z", 0),
        ];
        let composer = Anchor {
            path: "src/x.rs".into(),
            start_line: 1,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let cards = cards_of(
            &threads,
            Some(&composer),
            &file,
            &HashSet::new(),
            &HashMap::new(),
        );
        let ids: Vec<(CardId, usize)> = cards
            .iter()
            .map(|c| (c.id.clone(), c.after_display_line))
            .collect();
        // Under c, past the ghost of d at display line 4, in time order
        // whoever made them, then the composer on the last selected line;
        // GitHub's on e is under e alone, and the base side's on b's ghost.
        assert_eq!(
            ids,
            vec![
                (CardId::Thread("gh-10".into()), 4),
                (CardId::Thread("gh-30".into()), 5),
                (CardId::Thread("gh-40".into()), 1),
                (CardId::Thread("mine".into()), 4),
                (CardId::Thread("gh-20".into()), 4),
                (CardId::Composer, 4),
            ]
        );
        assert_eq!(cards[0].rows, doc::estimate_rows(&threads[2], false));
        assert!(
            cards[4].rows < cards[0].rows,
            "a comment without a reply is shorter"
        );
        assert_eq!(
            cards[5].rows,
            doc::COMPOSER_ROWS,
            "the composer's estimate until it is laid out"
        );

        // Laid out and collapsed, as the reader's cards are.
        let collapsed: HashSet<CardId> = [CardId::Thread("gh-10".into())].into_iter().collect();
        let rows: HashMap<CardId, usize> = [(CardId::Thread("gh-20".into()), 6), (CardId::Composer, 12)]
            .into_iter()
            .collect();
        let cards = cards_of(&threads, Some(&composer), &file, &collapsed, &rows);
        assert_eq!(cards[0].rows, doc::COLLAPSED_ROWS);
        assert_eq!(cards[4].rows, 6);
        assert_eq!(cards[5].rows, 12);

        // Through the state: the composer is read with the threads.
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        state.active.set(Some("src/x.rs".into()));
        state.comment_threads.set(Arc::new(threads));
        assert_eq!(state.cards_in("src/x.rs").len(), 5);
        assert!(!state.composer_is_inline());
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.show_tab(ItemTab::Files);
        state.open_composer(None, Some(composer.clone()), String::new(), choice.clone());
        assert!(state.composer_is_inline());
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert_eq!(
            state.composer_card_anchor("src/x.rs"),
            Some(composer.clone())
        );
        assert_eq!(state.composer_card_anchor("src/y.rs"), None);
        assert_eq!(state.cards_in("src/x.rs").len(), 6);
        // A follow-up is inside its thread's card, not a card of its own,
        // and stays on Files when that card is in the open file. A question
        // about the item as a whole moves to Conversation.
        state.open_composer(
            Some("mine".into()),
            Some(composer.clone()),
            String::new(),
            choice.clone(),
        );
        assert!(!state.composer_is_inline());
        assert!(state.follow_up_is_inline("mine"));
        assert!(
            !state.follow_up_is_inline("elsewhere"),
            "no such thread in the file"
        );
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert_eq!(state.cards_in("src/x.rs").len(), 5);
        state.open_composer(None, None, "whole item".into(), choice.clone());
        assert!(!state.composer_is_inline());
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "whole item");
        assert_eq!(state.cards_in("src/x.rs").len(), 5);

        // A follow-up on a thread whose card is collapsed opens the card,
        // so the box it sits inside is seen.
        state.show_tab(ItemTab::Files);
        state.toggle_card(&CardId::Thread("mine".into()));
        state.open_composer(Some("mine".into()), Some(composer), String::new(), choice);
        assert!(!state.is_collapsed(&CardId::Thread("mine".into())));
        assert_eq!(state.tab.get(), ItemTab::Files);
    }

    #[test]
    fn a_resolved_thread_starts_collapsed_and_opens_when_the_reader_asks() {
        use perch_github::Side;
        let file = spliced_file();
        let mut resolved = github_thread(10, 3, Side::Right, "2026-09-18T09:00:00Z", 1);
        resolved.resolved = true;
        let open = github_thread(20, 4, Side::Right, "2026-09-18T09:30:00Z", 0);
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        state.active.set(Some("src/x.rs".into()));
        state.comment_threads.set(Arc::new(vec![resolved, open]));
        let card = CardId::Thread("gh-10".into());
        let other = CardId::Thread("gh-20".into());
        assert!(state.is_collapsed(&card));
        assert!(!state.is_collapsed(&other));
        assert_eq!(state.cards_in("src/x.rs")[0].rows, doc::COLLAPSED_ROWS);

        state.toggle_card(&card);
        assert!(!state.is_collapsed(&card));
        assert!(state.collapsed_cards.get().is_empty(), "opening a resolved thread is its own set");
        assert_ne!(state.cards_in("src/x.rs")[0].rows, doc::COLLAPSED_ROWS);
        state.toggle_card(&card);
        assert!(state.is_collapsed(&card));
        state.expand_card(&card);
        assert!(!state.is_collapsed(&card));

        // Any other thread collapses the way it always has.
        state.toggle_card(&other);
        assert!(state.is_collapsed(&other));
        assert!(state.collapsed_cards.get().contains(&other));
    }

    #[test]
    fn a_thread_collapsed_then_resolved_and_opened_stays_open_when_unresolved() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let thread = github_thread(10, 3, Side::Right, "2026-09-18T09:00:00Z", 1);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let card = CardId::Thread("gh-10".into());

        state.toggle_card(&card);
        assert!(state.is_collapsed(&card), "collapsed by the reader");
        let mut resolved = thread.clone();
        resolved.resolved = true;
        state.comment_threads.set(Arc::new(vec![resolved]));
        assert!(state.is_collapsed(&card));
        state.toggle_card(&card);
        assert!(!state.is_collapsed(&card), "opened by the reader");

        // Unresolved on GitHub, it is as the reader last left it.
        state.comment_threads.set(Arc::new(vec![thread]));
        assert!(!state.is_collapsed(&card));
    }

    #[test]
    fn the_composers_card_anchor_counts_among_the_files_anchors_until_its_thread_arrives() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 2,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        assert!(state.anchors_in("src/x.rs").is_empty());

        // Open on lines 2 to 3, the composer's card keeps them out of the
        // folds as a thread's would; the other file has nothing.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        assert!(state.anchors_in("src/y.rs").is_empty());

        // Sent, the placeholder keeps them until the thread's own anchor
        // takes over, and the thread is then the one anchor.
        state.composer_sent();
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        state.set_comment_threads(Arc::new(vec![anchored(
            "new",
            2,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);

        // Escape gives the lines back; a follow-up is no card and adds
        // none; a composer in Conversation adds none.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2), (1, 2)]);
        state.close_composer();
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        state.open_composer(
            Some("new".into()),
            Some(anchor),
            String::new(),
            choice.clone(),
        );
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
        state.open_composer(None, None, String::new(), choice);
        assert_eq!(state.anchors_in("src/x.rs"), vec![(1, 2)]);
    }

    #[test]
    fn a_composer_replacing_an_open_one_starts_on_the_estimate() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        let on = |start: u32, line: u32| Anchor {
            path: "src/x.rs".into(),
            start_line: start,
            line,
            side: Side::Right,
            commit_sha: "h".into(),
        };

        // A composer typed into grows to fourteen rows; `a` on other
        // lines replaces it with an empty one, which is the estimate's
        // height, not the last box's.
        state.open_composer(None, Some(on(3, 3)), String::new(), choice.clone());
        state.set_card_rows(&CardId::Composer, 14);
        assert_eq!(state.cards_in("src/x.rs")[0].rows, 14);
        state.open_composer(None, Some(on(1, 2)), String::new(), choice.clone());
        assert!(
            !state.card_rows.get().contains_key(&CardId::Composer),
            "the last card's rows are not this one's"
        );
        assert_eq!(
            state.cards_in("src/x.rs"),
            vec![Card {
                id: CardId::Composer,
                after_display_line: 2,
                rows: doc::COMPOSER_ROWS
            }]
        );

        // A follow-up replacing it leaves nothing behind for the next
        // new composer either.
        state.set_card_rows(&CardId::Composer, 12);
        state.set_comment_threads(Arc::new(vec![anchored(
            "t",
            3,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        state.open_composer(
            Some("t".into()),
            Some(on(3, 3)),
            String::new(),
            choice.clone(),
        );
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));
        state.open_composer(None, Some(on(1, 2)), String::new(), choice);
        assert_eq!(
            state
                .cards_in("src/x.rs")
                .iter()
                .find(|c| c.id == CardId::Composer)
                .map(|c| c.rows),
            Some(doc::COMPOSER_ROWS)
        );
    }

    #[test]
    fn a_composer_moves_to_conversation_when_the_open_file_changes_under_it() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.open_tab("src/x.rs");
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // Opened on lines of the open file, the composer is a card there
        // and Files stays visible. Another file opened over it has no card
        // for it, so Conversation takes over without losing the draft.
        state.show_tab(ItemTab::Files);
        state.open_composer(None, Some(anchor.clone()), "why?".into(), choice.clone());
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        state.open_tab("src/y.rs");
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "why?");
        state.open_tab("src/x.rs");
        assert!(state.composer_in_code());
        state.show_tab(ItemTab::Files);
        assert_eq!(state.tab.get(), ItemTab::Files);

        // Closing the composer's file for its neighbour does the same.
        state.close_tab("src/x.rs");
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "why?");

        // A follow-up inside a card of the open file likewise.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Files);
        state.set_comment_threads(Arc::new(vec![anchored(
            "t",
            3,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        state.open_composer(
            Some("t".into()),
            Some(anchor),
            String::new(),
            choice.clone(),
        );
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Conversation);

        // Without a composer, changing files leaves Files selected.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Files);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Files);
    }

    #[test]
    fn a_composer_that_was_not_in_the_code_stays_put_when_the_open_file_changes() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.open_tab("src/x.rs");
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // A question about the item as a whole opens on Conversation. The
        // reader who then picks Files and opens a file there was not shown
        // the composer in the code, so nothing is hidden: Files stays, and
        // the draft with it.
        state.open_composer(None, None, "what does this do?".into(), choice.clone());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        state.show_tab(ItemTab::Files);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Files);
        state.open_tab("src/x.rs");
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert_eq!(state.composer_draft.get(), "what does this do?");

        // Closing a tab likewise.
        state.close_tab("src/x.rs");
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(state.tab.get(), ItemTab::Files);

        // A composer on lines of a file other than the open one is not in
        // the code either, so opening a third file over it leaves Files
        // too; opening its own file draws it in the code again.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Files);
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        state.open_composer(None, Some(anchor), "why?".into(), choice);
        assert!(state.composer_in_code());
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        state.show_tab(ItemTab::Files);
        state.open_tab("src/z.rs");
        assert_eq!(state.tab.get(), ItemTab::Files);
        state.open_tab("src/x.rs");
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert_eq!(state.composer_draft.get(), "why?");
    }

    #[test]
    fn a_notice_gives_up_a_sent_questions_rows() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // Nothing sent: nothing to give up.
        state.drop_sent_placeholder();
        assert!(state.sent_composer.get().is_none());

        // Sent and laid out at ten rows, the send fails: the rows go, and
        // the composer that was already closed stays closed.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice);
        state.set_card_rows(&CardId::Composer, 10);
        state.composer_sent();
        assert_eq!(state.sent_composer.get(), Some(anchor));
        state.drop_sent_placeholder();
        assert!(state.sent_composer.get().is_none());
        assert!(
            state.cards_in("src/x.rs").is_empty(),
            "the placeholder's rows went with it"
        );
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));
        assert!(state.composer.get().is_none());
    }

    #[test]
    fn a_sent_question_keeps_its_cards_rows_until_the_threads_arrive_and_escape_gives_them_up() {
        use perch_github::Side;
        let state = ReviewState::new();
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(spliced_file()));
        });
        state.active.set(Some("src/x.rs".into()));
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );

        // Opened, laid out at ten rows, and sent: the composer is gone,
        // and the card's rows stay under its lines.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        state.set_card_rows(&CardId::Composer, 10);
        assert_eq!(
            state.cards_in("src/x.rs"),
            vec![Card {
                id: CardId::Composer,
                after_display_line: 4,
                rows: 10
            }]
        );
        state.composer_sent();
        assert!(state.composer.get().is_none());
        assert_eq!(state.sent_composer.get(), Some(anchor.clone()));
        assert_eq!(
            state.cards_in("src/x.rs"),
            vec![Card {
                id: CardId::Composer,
                after_display_line: 4,
                rows: 10
            }]
        );

        // Threads with nothing new, as a reaction's store sends them,
        // leave the rows waiting.
        state.set_comment_threads(Arc::new(vec![]));
        assert_eq!(state.sent_composer.get(), Some(anchor.clone()));

        // The thread arrives, and its card takes the place of the rows.
        state.set_comment_threads(Arc::new(vec![anchored(
            "new",
            3,
            3,
            "2026-09-18T10:00:00Z",
            false,
        )]));
        assert_eq!(state.sent_composer.get(), None);
        let cards = state.cards_in("src/x.rs");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, CardId::Thread("new".into()));
        assert!(
            !state.card_rows.get().contains_key(&CardId::Composer),
            "the next composer starts on the estimate"
        );

        // Escape gives the rows up at once.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        state.set_card_rows(&CardId::Composer, 10);
        state.close_composer();
        assert_eq!(state.cards_in("src/x.rs").len(), 1);
        assert!(!state.card_rows.get().contains_key(&CardId::Composer));

        // A question sent from Conversation keeps no rows: it was no card.
        state.open_composer(None, None, String::new(), choice.clone());
        state.composer_sent();
        assert_eq!(state.sent_composer.get(), None);

        // A new composer opened before the thread lands takes the
        // placeholder's place: there is one composer card at a time.
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        state.composer_sent();
        assert!(state.sent_composer.get().is_some());
        state.open_composer(None, Some(anchor), String::new(), choice);
        assert_eq!(state.sent_composer.get(), None);
        assert_eq!(
            state
                .cards_in("src/x.rs")
                .iter()
                .filter(|c| c.id == CardId::Composer)
                .count(),
            1
        );
    }

    #[test]
    fn a_cards_laid_out_rows_are_written_when_they_change_and_not_otherwise() {
        use floem::reactive::{Effect, SignalTrack};
        let state = ReviewState::new();
        let card = CardId::Thread("t1".into());
        // Every write rebuilds the open file's document, so the writes
        // are counted through what a write runs.
        let writes = RwSignal::new(0);
        Effect::new(move |_| {
            state.card_rows.track();
            writes.update(|w| *w += 1);
        });
        assert_eq!(writes.get(), 1, "the effect's first run");

        state.set_card_rows(&card, 8);
        assert_eq!(state.card_rows.get().get(&card), Some(&8));
        assert_eq!(writes.get(), 2);
        state.set_card_rows(&card, 8);
        assert_eq!(writes.get(), 2, "the same rows again is no write");
        state.set_card_rows(&card, 5);
        assert_eq!(state.card_rows.get().get(&card), Some(&5));
        assert_eq!(writes.get(), 3);
        // Another card's rows are its own.
        state.set_card_rows(&CardId::Thread("t2".into()), 5);
        assert_eq!(writes.get(), 4);
        assert_eq!(state.card_rows.get().len(), 2);
    }

    #[test]
    fn a_thread_arriving_outdated_starts_collapsed_once_and_the_readers_choice_stands_after() {
        let state = ReviewState::new();
        let stale = CardId::Thread("stale".into());
        let fresh = CardId::Thread("fresh".into());
        state.set_comment_threads(Arc::new(vec![
            anchored("stale", 1, 1, "2026-09-18T09:00:00Z", true),
            anchored("fresh", 2, 2, "2026-09-18T09:00:00Z", false),
        ]));
        assert!(state.is_collapsed(&stale));
        assert!(!state.is_collapsed(&fresh));
        assert_eq!(state.comment_threads.get().len(), 2);

        // Opened by the reader, it stays open when the threads come again,
        // as they do every time the item is opened.
        state.toggle_card(&stale);
        state.begin("Reading the pull request");
        assert!(state.comment_threads.get().is_empty());
        assert!(
            !state.is_collapsed(&stale),
            "the reader's choice outlives the item"
        );
        state.set_comment_threads(Arc::new(vec![anchored(
            "stale",
            1,
            1,
            "2026-09-18T09:00:00Z",
            true,
        )]));
        assert!(!state.is_collapsed(&stale));

        // A thread with no anchor is no card, and a new outdated one is
        // collapsed as the first was.
        let mut worktree = anchored("worktree", 1, 1, "2026-09-18T09:00:00Z", true);
        worktree.anchor = None;
        state.set_comment_threads(Arc::new(vec![
            worktree,
            anchored("another", 3, 3, "2026-09-18T09:00:00Z", true),
        ]));
        assert!(!state.is_collapsed(&CardId::Thread("worktree".into())));
        assert!(state.is_collapsed(&CardId::Thread("another".into())));
    }

    #[test]
    fn a_reveal_is_kept_by_path_and_blob_and_reset_forgets_the_run_it_is_in() {
        let state = ReviewState::new();
        assert!(state.reveals_for("src/x.rs", "b1").is_empty());
        state.reveal("src/x.rs", "b1", 4, 23);
        state.reveal("src/x.rs", "b2", 50, 60);
        assert!(state.reveals_for("src/x.rs", "b1").overlaps(4, 37));
        assert!(
            !state.reveals_for("src/x.rs", "b1").overlaps(50, 60),
            "another blob's reveal is another file's"
        );
        // Reset on a piece of the run folds the whole run: the piece
        // remembers the run it was cut from.
        let piece = crate::review::doc::Run {
            first: 25,
            last: 38,
            first_head: 24,
            default_heads: (4, 37),
        };
        state.reset("src/x.rs", "b1", &[piece.default_heads]);
        assert!(state.reveals_for("src/x.rs", "b1").is_empty());
        assert!(
            state.reveals_for("src/x.rs", "b2").overlaps(50, 60),
            "and no other"
        );
        let key = ("src/x.rs".to_string(), "b1".to_string());
        assert!(
            !state.reveals.get().contains_key(&key),
            "an empty set is dropped"
        );

        // Each went to the store as it happened, the reset over the whole
        // run the piece was cut from.
        assert_eq!(
            state.pending_reveals.get(),
            vec![
                RevealWrite::Reveal {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 4,
                    last: 23
                },
                RevealWrite::Reveal {
                    path: "src/x.rs".into(),
                    blob: "b2".into(),
                    first: 50,
                    last: 60
                },
                RevealWrite::Reset {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 4,
                    last: 37
                },
            ]
        );

        // A file with no blob keeps its reveals here and sends nothing.
        state.pending_reveals.set(Vec::new());
        state.reveal("untracked.rs", "", 0, 9);
        state.reset("untracked.rs", "", &[piece.default_heads]);
        assert!(state.pending_reveals.get().is_empty());
        assert!(
            state.reveals_for("untracked.rs", "").is_empty(),
            "the reset still took in memory"
        );

        // A divider carrying a run taken whole resets both, one write each.
        state.reveal("src/x.rs", "b1", 4, 23);
        state.reveal("src/x.rs", "b1", 43, 58);
        state.reveal("src/x.rs", "b1", 70, 79);
        state.pending_reveals.set(Vec::new());
        state.reset("src/x.rs", "b1", &[(4, 37), (43, 58)]);
        assert_eq!(
            state
                .reveals_for("src/x.rs", "b1")
                .iter()
                .collect::<Vec<_>>(),
            vec![(70, 79)]
        );
        assert_eq!(
            state.pending_reveals.get(),
            vec![
                RevealWrite::Reset {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 4,
                    last: 37
                },
                RevealWrite::Reset {
                    path: "src/x.rs".into(),
                    blob: "b1".into(),
                    first: 43,
                    last: 58
                },
            ]
        );
    }

    #[test]
    fn next_unviewed_is_none_once_everything_is_viewed() {
        let state = state();
        state.viewed.set(Arc::new(
            [("a.rs", "blob-a"), ("b.rs", "blob-b"), ("c.rs", "blob-c")]
                .into_iter()
                .map(|(p, b)| (p.to_string(), b.to_string()))
                .collect(),
        ));
        // The deletion has no blob and is not in the round, so a review
        // that deletes a file can still be viewed whole.
        assert_eq!(state.next_unviewed(Some("a.rs"), true), None);
        assert_eq!(state.viewed_count(), (3, 3));

        state
            .changed_files
            .set(Arc::new(vec![changed("a.rs", Some("blob-a"))]));
        assert_eq!(state.next_unviewed(Some("a.rs"), true), None);
        assert_eq!(state.next_unviewed(None, false), None);
        assert_eq!(state.viewed_count(), (1, 1));

        state.changed_files.set(Arc::new(Vec::new()));
        assert_eq!(state.next_unviewed(None, true), None);
        assert_eq!(state.viewed_count(), (0, 0));
    }

    #[test]
    fn an_outside_path_is_labelled_by_where_it_is_from() {
        use std::path::Path;
        let home = Path::new("/Users/me");
        let label = |p: &str| outside_label(Path::new(p), Some(home));
        assert_eq!(
            label("/Users/me/.cargo/registry/src/index.crates.io-6f17d22bba15001f/tokio-1.40.0/src/io/mod.rs"),
            "tokio 1.40.0"
        );
        assert_eq!(
            label("/Users/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-util-0.7.12/src/lib.rs"),
            "tokio-util 0.7.12"
        );
        assert_eq!(
            label("/Users/me/.cargo/git/checkouts/floem-785aa715ca28b34b/d1184f2/src/views/tooltip.rs"),
            "git: floem"
        );
        assert_eq!(
            label("/Users/me/.rustup/toolchains/1.91.1-aarch64-apple-darwin/lib/rustlib/src/rust/library/core/src/result.rs"),
            "core"
        );
        assert_eq!(
            label("/Users/me/.rustup/toolchains/stable/lib/rustlib/src/rust/library/std/src/io/mod.rs"),
            "std"
        );
        assert_eq!(label("/Users/me/Code/other/src/lib.rs"), "Code/other/src/lib.rs");
        assert_eq!(label("/opt/vendor/src/lib.rs"), "/opt/vendor/src/lib.rs");
        assert_eq!(
            outside_label(Path::new("/Users/me/Code/other/src/lib.rs"), None),
            "/Users/me/Code/other/src/lib.rs"
        );
    }

    #[test]
    fn the_history_goes_back_and_forward_and_drops_the_oldest() {
        let place = |line: usize| Place { path: "src/a.rs".into(), line };
        let mut history = History::default();
        assert_eq!(history.back(place(0)), None);
        assert_eq!(history.forward(place(0)), None);

        // Two jumps: from 1 to 5, then from 5 to 9.
        history.push(place(1));
        history.push(place(5));

        // Back lands on 5 with 9 kept ahead, then on 1 with 5 ahead.
        assert_eq!(history.back(place(9)), Some(place(5)));
        assert_eq!(history.back(place(5)), Some(place(1)));
        assert_eq!(history.back(place(1)), None, "nothing before the first");
        assert_eq!(history.forward(place(1)), Some(place(5)));
        assert_eq!(history.forward(place(5)), Some(place(9)));
        assert_eq!(history.forward(place(9)), None);

        // A jump from the middle of the history clears what was ahead.
        assert_eq!(history.back(place(9)), Some(place(5)));
        history.push(place(5));
        assert_eq!(history.forward(place(20)), None);
        // And jumping twice from one place is one place, not two.
        history.push(place(5));
        assert_eq!(history.back(place(20)), Some(place(5)));
        assert_eq!(history.back(place(5)), Some(place(1)));

        let mut long = History::default();
        for line in 0..(HISTORY_CAP + 10) {
            long.push(place(line));
        }
        let mut back = 0;
        while long.back(place(999)).is_some() {
            back += 1;
        }
        assert_eq!(back, HISTORY_CAP, "the oldest were dropped");
    }

    #[test]
    fn a_jump_remembers_where_it_left_and_the_keys_walk_the_history() {
        let state = ReviewState::new();
        let file = |path: &str| {
            Arc::new(crate::backend::build_loaded_file(
                path.into(),
                "fn a() {}\nfn b() {}\nfn c() {}\n".into(),
                String::new(),
                None,
                false,
                Scope::Branch,
                String::new(),
            ))
        };
        state.open_tab("src/a.rs");
        state.finish_load("src/a.rs", file("src/a.rs"));
        state.last_position.set(Some((1, 3)));

        // A jump to another file: the place left is a.rs line 1.
        state.navigate_to("src/b.rs", 2, 3);
        assert_eq!(state.pending_jump.get(), Some(("src/b.rs".into(), 2, 3)));

        // The view opened b.rs and the caret landed on line 2.
        state.pending_jump.set(None);
        state.open_tab("src/b.rs");
        state.finish_load("src/b.rs", file("src/b.rs"));
        state.last_position.set(Some((2, 3)));

        state.go_back();
        assert_eq!(state.pending_jump.get(), Some(("src/a.rs".into(), 1, 0)));
        state.pending_jump.set(None);
        state.open_tab("src/a.rs");
        state.last_position.set(Some((1, 0)));

        state.go_forward();
        assert_eq!(state.pending_jump.get(), Some(("src/b.rs".into(), 2, 0)));
        state.pending_jump.set(None);
        state.go_forward();
        assert_eq!(state.pending_jump.get(), None, "nothing ahead");
    }

    #[test]
    fn a_gotos_answer_opens_one_location_lists_several_and_drops_a_stale_one() {
        let state = ReviewState::new();
        let located = |path: &str, line: usize| Located {
            path: path.into(),
            outside: None,
            line,
            character: 4,
            text: "x".into(),
        };
        state.goto.set(Some(GotoAsk { token: 7, symbol: "x".into(), kind: Locations::Definitions }));

        // A stale token answers nothing and leaves the ask waiting.
        state.finish_goto(6, vec![located("src/a.rs", 3)]);
        assert_eq!(state.pending_jump.get(), None);
        assert!(state.goto.get().is_some());

        state.finish_goto(7, vec![located("src/a.rs", 3)]);
        assert_eq!(state.pending_jump.get(), Some(("src/a.rs".into(), 2, 4)));
        assert!(state.goto.get().is_none(), "the ask is answered");

        state.pending_jump.set(None);
        state.goto.set(Some(GotoAsk { token: 8, symbol: "x".into(), kind: Locations::Definitions }));
        state.finish_goto(8, vec![located("src/a.rs", 3), located("src/b.rs", 9)]);
        assert_eq!(state.pending_jump.get(), None, "several open the panel, not a file");
        let panel = state.references.get().expect("the panel opened");
        assert_eq!(panel.kind, Locations::Definitions);
        assert_eq!(panel.symbol, "x");
        assert_eq!(panel.locations.len(), 2);
        assert!(!panel.pending);

        state.goto.set(Some(GotoAsk { token: 9, symbol: "x".into(), kind: Locations::Definitions }));
        state.finish_goto(9, vec![]);
        assert_eq!(state.pending_jump.get(), None, "none does nothing");
    }

    #[test]
    fn an_origin_is_read_off_the_path() {
        assert_eq!(Origin::of_path("src/lib.rs"), Origin::Worktree);
        let outside = Origin::of_path("/opt/vendor/src/lib.rs");
        assert_eq!(outside.label(), Some("/opt/vendor/src/lib.rs"));
        assert!(outside.is_outside());
        assert!(!Origin::Worktree.is_outside());

        let state = ReviewState::new();
        state.open_tab("/opt/vendor/src/lib.rs");
        assert_eq!(
            state.tabs.get()[0].origin,
            Origin::Outside { label: "/opt/vendor/src/lib.rs".into() },
            "the tab carries the origin before the file has loaded"
        );
    }

    /// `anchored("t1", ..)` with its last message replaced by one from
    /// `author`, of `origin`, on GitHub as `github_id`, with `run`.
    fn thread_ending_in(
        author: perch_github::Author,
        origin: perch_github::ThreadOrigin,
        github_id: Option<u64>,
        run: Option<perch_github::RunRecord>,
    ) -> CommentThread {
        let mut thread = anchored("t1", 3, 3, "2026-09-18T10:00:00Z", false);
        let mut last = thread.messages[0].clone();
        last.id = "t1-last".into();
        last.author = author;
        last.origin = origin;
        last.github_id = github_id;
        last.run = run;
        thread.messages.push(last);
        thread
    }

    fn agent_run(model: &str, effort: &str) -> perch_github::RunRecord {
        perch_github::RunRecord {
            id: "r1".into(),
            backend: "claude-code".into(),
            model: model.into(),
            effort: effort.into(),
            status: perch_github::RunStatus::Completed,
            ..Default::default()
        }
    }

    fn sonnet_medium() -> AgentChoice {
        (
            perch_agent::Backend::ClaudeCode,
            "sonnet".into(),
            perch_agent::Effort::Medium,
        )
    }

    fn opus_high() -> AgentChoice {
        (
            perch_agent::Backend::ClaudeCode,
            "opus".into(),
            perch_agent::Effort::High,
        )
    }

    #[test]
    fn editing_a_draft_opens_on_github_whatever_else_is_true() {
        use perch_github::{Author, ThreadOrigin};
        let thread = thread_ending_in(Author::User, ThreadOrigin::Local, None, None);
        let agent = perch_github::AgentConfig::default();
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            Some(Destination::agent(opus_high())),
            Some(opus_high()),
            sonnet_medium(),
            true,
            &agent,
        );
        assert_eq!(d, Destination::GitHub);
    }

    #[test]
    fn a_follow_up_under_a_github_user_or_the_readers_own_comment_opens_on_github() {
        use perch_github::{Author, ThreadOrigin};
        let agent = perch_github::AgentConfig::default();
        let under = |thread: CommentThread| {
            default_destination(
                Some(&thread),
                thread.anchor.as_ref(),
                None,
                Some(opus_high()),
                sonnet_medium(),
                false,
                &agent,
            )
        };
        let theirs = thread_ending_in(
            Author::GitHubUser("octocat".into()),
            ThreadOrigin::GitHub,
            Some(31),
            None,
        );
        assert_eq!(under(theirs), Destination::GitHub);
        let draft = thread_ending_in(Author::User, ThreadOrigin::GitHub, None, None);
        assert_eq!(under(draft), Destination::GitHub, "a draft of the reader's");
        let posted = thread_ending_in(Author::User, ThreadOrigin::GitHub, Some(32), None);
        assert_eq!(under(posted), Destination::GitHub, "a comment the reader posted");
    }

    #[test]
    fn a_follow_up_under_an_agent_opens_on_that_agent_at_its_runs_effort() {
        use perch_github::{Author, ThreadOrigin};
        let agent = perch_github::AgentConfig::default();
        let answered = |run: Option<perch_github::RunRecord>| {
            thread_ending_in(
                Author::Agent {
                    backend: "claude-code".into(),
                    model: "opus".into(),
                },
                ThreadOrigin::Local,
                None,
                run,
            )
        };
        let thread = answered(Some(agent_run("opus", "high")));
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            Some(Destination::GitHub),
            None,
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(opus_high()));

        // Without a run, the defaults' effort.
        let thread = answered(None);
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            None,
            None,
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(
            d,
            Destination::Agent {
                backend: perch_agent::Backend::ClaudeCode,
                model: "opus".into(),
                effort: perch_agent::Effort::Medium,
            }
        );
    }

    #[test]
    fn a_follow_up_under_an_agent_keeps_to_what_config_offers() {
        use perch_github::{Author, ThreadOrigin};
        // Codex's gpt-5 takes low to high; the run went at xhigh, which
        // the catalog has since dropped.
        let mut agent = perch_github::AgentConfig::default();
        agent.codex.models = vec!["gpt-5".into()];
        agent.codex.efforts.insert(
            "gpt-5".into(),
            vec!["low".into(), "medium".into(), "high".into()],
        );
        let mut run = agent_run("gpt-5", "xhigh");
        run.backend = "codex".into();
        let thread = thread_ending_in(
            Author::Agent {
                backend: "codex".into(),
                model: "gpt-5".into(),
            },
            ThreadOrigin::Local,
            None,
            Some(run),
        );
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            None,
            None,
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(
            d,
            Destination::Agent {
                backend: perch_agent::Backend::Codex,
                model: "gpt-5".into(),
                effort: perch_agent::Effort::High,
            },
            "the nearest effort the model takes"
        );

        // A model config no longer lists at all: the last agent used.
        agent.codex.models.clear();
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            None,
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(opus_high()));
    }

    #[test]
    fn a_follow_up_under_the_readers_own_question_opens_on_the_last_agent() {
        use perch_github::{Author, ThreadOrigin};
        let agent = perch_github::AgentConfig::default();
        let thread = thread_ending_in(Author::User, ThreadOrigin::Local, None, None);
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            Some(Destination::GitHub),
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(opus_high()));
        let d = default_destination(
            Some(&thread),
            thread.anchor.as_ref(),
            Some(Destination::GitHub),
            None,
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(sonnet_medium()), "else the defaults");

        // An empty thread has no last message to follow.
        let mut empty = thread.clone();
        empty.messages.clear();
        let d = default_destination(
            Some(&empty),
            empty.anchor.as_ref(),
            Some(Destination::GitHub),
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(opus_high()));
    }

    #[test]
    fn a_new_composer_on_lines_opens_where_its_face_last_sent_and_one_on_the_item_on_an_agent() {
        let agent = perch_github::AgentConfig::default();
        let anchor = anchored("t1", 3, 3, "2026-09-18T10:00:00Z", false).anchor;
        let d = default_destination(
            None,
            anchor.as_ref(),
            Some(Destination::GitHub),
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::GitHub);
        let d = default_destination(
            None,
            anchor.as_ref(),
            None,
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(opus_high()), "nothing sent from this face yet");
        let d = default_destination(
            None,
            None,
            Some(Destination::GitHub),
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(d, Destination::agent(opus_high()), "no anchor: an agent, whatever was last");
        let d = default_destination(None, None, None, None, sonnet_medium(), false, &agent);
        assert_eq!(d, Destination::agent(sonnet_medium()));
    }

    #[test]
    fn the_drafts_are_listed_with_their_threads_root_on_github_and_the_anchors_head() {
        use perch_github::{Author, Side, ThreadOrigin};
        // A draft replying under one of GitHub's, and a new thread of one.
        let mut reply = github_thread(31, 3, Side::Right, "2026-09-18T10:00:00Z", 0);
        let mut draft = reply.messages[0].clone();
        draft.id = "d1".into();
        draft.author = Author::User;
        draft.github_id = None;
        draft.body = "I will fix it".into();
        reply.messages.push(draft);
        let mut fresh = anchored("t2", 5, 6, "2026-09-18T10:01:00Z", false);
        fresh.origin = ThreadOrigin::GitHub;
        fresh.messages[0].origin = ThreadOrigin::GitHub;
        fresh.messages[0].body = "and this".into();
        let mine = anchored("t3", 7, 7, "2026-09-18T10:02:00Z", false);

        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(vec![reply, fresh, mine]));
        let drafts = state.drafts();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].thread_id, "gh-31");
        assert_eq!(drafts[0].message_id, "d1");
        assert_eq!(drafts[0].in_reply_to, Some(31));
        assert_eq!(drafts[0].commit_sha, "h");
        assert_eq!(drafts[0].body, "I will fix it");
        assert_eq!(drafts[1].thread_id, "t2");
        assert_eq!(drafts[1].in_reply_to, None, "a new thread has no root there yet");
        assert_eq!(drafts[1].anchor.as_ref().map(|a| (a.start_line, a.line)), Some((5, 6)));
        assert!(drafts[0].reply);
        assert!(!drafts[1].reply);
    }

    #[test]
    fn a_second_draft_in_a_thread_not_yet_on_github_is_a_reply() {
        use perch_github::ThreadOrigin;
        let mut fresh = anchored("t2", 5, 6, "2026-09-18T10:01:00Z", false);
        fresh.origin = ThreadOrigin::GitHub;
        fresh.messages[0].origin = ThreadOrigin::GitHub;
        let mut second = fresh.messages[0].clone();
        second.id = "d2".into();
        fresh.messages.push(second);

        let drafts = drafts_of(&[fresh]);
        assert_eq!(drafts.len(), 2);
        assert!(!drafts[0].reply, "the first opens the thread");
        assert!(drafts[1].reply, "the second posts under it");
        assert_eq!(drafts_summary(&drafts), "1 reply, 1 comment");
    }

    fn a_draft(id: &str, in_reply_to: Option<u64>) -> Draft {
        Draft {
            thread_id: format!("t-{id}"),
            message_id: id.into(),
            anchor: None,
            in_reply_to,
            reply: in_reply_to.is_some(),
            body: "x".into(),
            commit_sha: "h".into(),
        }
    }

    #[test]
    fn the_drafts_are_summed_up_as_replies_and_comments() {
        assert_eq!(drafts_summary(&[]), "");
        assert_eq!(drafts_summary(&[a_draft("a", None)]), "1 comment");
        assert_eq!(
            drafts_summary(&[a_draft("a", None), a_draft("b", None), a_draft("c", None)]),
            "3 comments"
        );
        assert_eq!(
            drafts_summary(&[a_draft("a", Some(1)), a_draft("b", None), a_draft("c", None)]),
            "1 reply, 2 comments"
        );
        assert_eq!(
            drafts_summary(&[a_draft("a", Some(1)), a_draft("b", Some(2))]),
            "2 replies"
        );
    }

    #[test]
    fn the_outcome_says_what_did_not_post_and_nothing_when_all_did() {
        assert_eq!(submit_review_outcome(true, 3, &[], &[], &[]), None);
        assert_eq!(submit_review_outcome(false, 1, &[], &[], &[]), None);
        let kept = [a_draft("r1", Some(31)), a_draft("c1", None), a_draft("c2", None)];
        assert_eq!(
            submit_review_outcome(
                true,
                2,
                &["c1".into()],
                &[("r1".into(), "GitHub returned 422: nope.".into())],
                &kept,
            )
            .as_deref(),
            Some(
                "Posted the review and 2 comments. 1 reply did not post: GitHub returned 422: nope. \
                 1 comment could not be matched to what GitHub returned and is still a draft."
            )
        );
        assert_eq!(
            submit_review_outcome(true, 0, &["c1".into(), "c2".into()], &[], &kept).as_deref(),
            Some(
                "Posted the review. 2 comments could not be matched to what GitHub returned and \
                 are still drafts."
            )
        );
        // No review went: a reply on its own that did not post, or one
        // that did beside one that did not.
        assert_eq!(
            submit_review_outcome(false, 0, &[], &[("r1".into(), "offline".into())], &kept)
                .as_deref(),
            Some("1 reply did not post: offline.")
        );
        assert_eq!(
            submit_review_outcome(false, 1, &[], &[("r1".into(), "offline".into())], &kept)
                .as_deref(),
            Some("Posted 1 comment. 1 reply did not post: offline.")
        );
        assert_eq!(
            submit_review_outcome(
                true,
                1,
                &[],
                &[
                    ("r1".into(), "offline".into()),
                    ("c1".into(), "offline".into()),
                    ("c2".into(), "offline".into()),
                ],
                &kept,
            )
            .as_deref(),
            Some("Posted the review and 1 comment. 1 reply and 2 comments did not post: offline.")
        );
    }
}
