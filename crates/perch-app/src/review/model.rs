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
    Anchor, Checks, CommentPlace, CommentThread, Discussion, Issue, IssueComment, Item, PullEvent,
    PullEventKind, PullRequest, PullRequestOverview, PullReviewEvent, ReactionKind, Reactions,
    Review,
};
use perch_syntax::{Language, LineSpans};

use crate::review::code_refs::{Found, NameAsk, Named};
use crate::review::doc::{self, Card, CardId, Doc, Reveals};
use crate::review::picture::Picture;
use crate::review::timeline::count_of;

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

/// A file's outline from the language server: its blocks, each the line
/// of its name and its last line, and a hash of the lines they are
/// counted in, which is the file on disk when the server was asked, or
/// a deleted file's removed lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    pub lines: u64,
    pub blocks: Vec<(usize, usize)>,
}

impl LoadedFile {
    pub fn change_count(&self) -> usize {
        self.buffer.change_starts.len()
    }
}

/// A key that moves through Conversation or the open file: Page Up and
/// Page Down by a page, Cmd-Up and Cmd-Down to the top and the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    Up,
    Down,
    Top,
    End,
}

/// What the item view shows: Conversation, or Code, the open file and the
/// tabs over it. The two are the switch in the bar; Conversation cannot
/// be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemTab {
    #[default]
    Conversation,
    Code,
}

/// An item's pane as it was left: which of Conversation and Code was
/// showing, the file tabs, the files shown whole, where the reader was on
/// each tab, and the Agents section, unfolded or not and the runs open in it. The item opens on it again; the first time,
/// on Conversation alone.
#[derive(Debug, Clone)]
pub struct Pane {
    tab: ItemTab,
    tabs: Vec<Tab>,
    active: Option<String>,
    whole: HashSet<String>,
    conversation_scroll: f64,
    file_places: HashMap<String, FilePlace>,
    agents_unfolded: bool,
    agents_open_runs: HashSet<String>,
    code_seen: bool,
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            tab: ItemTab::Conversation,
            tabs: Vec::new(),
            active: None,
            whole: HashSet::new(),
            conversation_scroll: 0.0,
            file_places: HashMap::new(),
            agents_unfolded: false,
            agents_open_runs: HashSet::new(),
            code_seen: false,
        }
    }
}

/// Where the reader was in a file: the view's scroll, and the caret.
///
/// The scroll is held as a line and how far the view's top is from it, so
/// that the file shows the same code when its document is built again
/// with other rows above, a card grown or a fold opened, or in the other
/// layout. The line is the first one at or under the view's top, a
/// display line of the unified buffer, which the stacked document and a
/// split's head side both have rows for; the pixels are kept as well, for
/// a document that no longer shows the line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilePlace {
    /// The first line at or under the top of the view, and how far the
    /// top is below that line's own top: negative when the line is under
    /// it, below a card or a divider.
    pub anchor: Option<(usize, f64)>,
    /// The scroll offset, in pixels.
    pub x: f64,
    pub y: f64,
    /// The caret, as a display line and a byte column in it; `None` when
    /// it was on a row that is no line.
    pub caret: Option<(usize, usize)>,
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
    /// The server's error, when it answered with one: that it is still
    /// loading, say. The panel says it beside "None found".
    pub failed: Option<String>,
    /// The symbol asked about, so an ask the server failed can be asked
    /// again once it is ready. `None` for a list that was not asked for
    /// as one: a goto's several answers.
    pub asked: Option<Symbol>,
}

/// What the chip on an outside file's tab says about where the file is
/// from, read off its path. A crate under cargo's registry is its name and
/// version, `tokio 1.40.0`; a checkout under cargo's git directory is
/// `git:` and the repository, `git: floem`; a file of the toolchain's
/// `rust-src` is its library crate, `std`, `core` or `alloc`. A Go module
/// in the module cache is its path and version, `github.com/spf13/cobra
/// v1.8.0`, and a file of Go's own library `go` and its package, `go
/// net/http`. A file under `node_modules` is its package, `react`, with
/// its version where pnpm's store gives one, `react 18.2.0`. Anywhere else under the home directory reads as its path
/// relative to the home directory, and anywhere else as the path.
pub fn outside_label(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    // jdt://contents/<jar or module>/<package>/<Class>.class: the jar, or
    // the JDK for its modules and its old rt.jar.
    if let Some(rest) = path
        .to_str()
        .and_then(|p| p.strip_prefix("jdt://contents/"))
    {
        let container = rest.split('/').next().unwrap_or(rest);
        return if container.ends_with(".jar") && container != "rt.jar" {
            container.to_string()
        } else {
            "JDK".to_string()
        };
    }
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
    if let Some(at) = parts
        .windows(3)
        .position(|w| w == [".cargo", "registry", "src"])
    {
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
    if let Some(package) = go_std_package(path, &parts) {
        return if package.is_empty() {
            "go".to_string()
        } else {
            format!("go {package}")
        };
    }
    if let Some(package) = node_package(&parts) {
        return package;
    }
    // <GOMODCACHE>/<module>@<version>/..., under `pkg/mod`.
    if let Some(at) = parts.windows(2).position(|w| w == ["pkg", "mod"]) {
        let rest = &parts[at + 2..];
        if let Some(i) = rest.iter().position(|p| p.contains('@')) {
            if let Some((last, version)) = rest[i].split_once('@') {
                let module = rest[..i]
                    .iter()
                    .copied()
                    .chain([last])
                    .collect::<Vec<_>>()
                    .join("/");
                return format!("{} {version}", unescape_module(&module));
            }
        }
    }
    if let Some(relative) = home.and_then(|home| path.strip_prefix(home).ok()) {
        return relative.display().to_string();
    }
    path.display().to_string()
}

/// The package of a file of Go's standard library: what lies between a Go
/// root's `src` and the file. A Go root is Homebrew's `libexec`, a
/// toolchain the go command downloaded (`golang.org/toolchain@…`), or a
/// `go` directory with Go's `VERSION` file in it.
fn go_std_package(path: &std::path::Path, parts: &[&str]) -> Option<String> {
    if !parts.last()?.ends_with(".go") {
        return None;
    }
    let at = parts.iter().rposition(|p| *p == "src")?;
    let root = *parts.get(at.checked_sub(1)?)?;
    let is_root = root == "libexec"
        || root.starts_with("toolchain@")
        || (root.starts_with("go") && {
            let up = parts.len() - at;
            path.ancestors()
                .nth(up)
                .is_some_and(|dir| dir.join("VERSION").is_file())
        });
    is_root.then(|| parts[at + 1..parts.len() - 1].join("/"))
}

/// The package a file under `node_modules` is in, from the last
/// `node_modules` on its path, scope and all; with its version when pnpm's
/// store, `.pnpm/<name>@<version>/node_modules/<name>`, names one.
fn node_package(parts: &[&str]) -> Option<String> {
    let at = parts.iter().rposition(|p| *p == "node_modules")?;
    let first = *parts.get(at + 1)?;
    let name = if first.starts_with('@') {
        format!("{first}/{}", parts.get(at + 2)?)
    } else {
        first.to_string()
    };
    // pnpm: `.pnpm/@scope+name@1.2.3_peer@4.5.6/node_modules/@scope/name`.
    let version = at
        .checked_sub(1)
        .and_then(|i| parts.get(i))
        .filter(|_| at >= 2 && parts[at - 2] == ".pnpm")
        .and_then(|store| store.strip_prefix(&format!("{}@", name.replace('/', "+"))))
        .map(|rest| rest.split('_').next().unwrap_or(rest));
    Some(match version {
        Some(version) => format!("{name} {version}"),
        None => name,
    })
}

/// A module path as the module cache writes it, with each capital as `!`
/// and the letter, read back: `github.com/!burnt!sushi/toml`.
fn unescape_module(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len());
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        match c {
            '!' => out.extend(chars.next().map(|c| c.to_ascii_uppercase())),
            c => out.push(c),
        }
    }
    out
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
    /// calls, or that calls nothing. With the server's error when it
    /// failed the ask: "No callers (rust-analyzer is still loading)."
    pub fn none(self, failed: Option<&str>) -> String {
        let none = match self {
            Direction::Callers => "No callers",
            Direction::Callees => "No callees",
        };
        match failed {
            Some(error) => format!("{none} ({error})."),
            None => format!("{none}."),
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
    /// The server's error for the ask for this row's children, when it
    /// answered with one, until the row is asked again.
    pub failed: Option<String>,
}

impl HierarchyRow {
    /// Whether two items are the same function: the server's items carry
    /// no identity but where they are.
    fn same(
        a: &perch_lsp::lsp_types::CallHierarchyItem,
        b: &perch_lsp::lsp_types::CallHierarchyItem,
    ) -> bool {
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
            failed: None,
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
                    failed: None,
                }
            })
            .collect();
        let row = &mut self.rows[at];
        row.pending = None;
        row.expanded = true;
        row.failed = None;
        let count = children.len();
        self.rows.splice(at + 1..at + 1, children);
        Some(count)
    }

    /// The ask for a row's children under `token` failed: the row is open
    /// with nothing under it, and says why. `None` when no row was
    /// waiting on the token.
    pub fn fail(&mut self, token: u64, error: String) -> Option<usize> {
        let at = self.rows.iter().position(|r| r.pending == Some(token))?;
        let row = &mut self.rows[at];
        row.pending = None;
        row.expanded = true;
        row.failed = Some(error);
        Some(at)
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
        let Some(row) = self.rows.get_mut(at) else {
            return;
        };
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
/// `kind`; none says so in a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoAsk {
    pub token: u64,
    pub symbol: String,
    pub kind: Locations,
    /// What was asked for, as the notice names it: "definition", "type
    /// definition", "implementations".
    pub noun: &'static str,
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
/// and not one opened since. `closing` when the text goes with a close
/// of the pull request, or is empty and the close goes alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub token: u64,
    pub opening: u64,
    pub body: String,
    pub closing: bool,
    /// A suggestion going into the worktree's file, not to GitHub.
    pub applying: bool,
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
                let Some(backend) = perch_agent::Backend::parse(backend).filter(|_| offered) else {
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

/// What the reader is told once the submission is done and not every
/// draft posted, from the counts the backend answered with: nothing when
/// they all did. `review` says a review posted; `posted` counts the
/// drafts confirmed on GitHub. `drafts` are the drafts still kept, which
/// say whether a failed one was a reply or a comment, and where each of
/// `outside_diff`, those on files the pull request does not change, is.
/// Those are named, the first three, since each has to be moved or
/// dropped before it can go.
pub fn submit_review_outcome(
    review: bool,
    posted: usize,
    unmatched: &[String],
    failed: &[(String, String)],
    outside_diff: &[String],
    drafts: &[Draft],
) -> Option<String> {
    if unmatched.is_empty() && failed.is_empty() && outside_diff.is_empty() {
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
        text.push_str(&format!(
            "Posted {}.",
            count_of(posted, "comment", "comments")
        ));
    }
    if !outside_diff.is_empty() {
        let n = outside_diff.len();
        let mut named: Vec<String> = outside_diff
            .iter()
            .filter_map(|id| drafts.iter().find(|d| d.message_id == *id)?.anchor.as_ref())
            .map(|anchor| anchor.label())
            .take(3)
            .collect();
        let more = n - named.len();
        if more > 0 {
            named.push(if named.is_empty() {
                count_of(more, "comment", "comments")
            } else {
                format!("{more} more")
            });
        }
        let last = named.pop().unwrap_or_default();
        let which = if named.is_empty() {
            last
        } else {
            format!("{} and {last}", named.join(", "))
        };
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!(
            "GitHub takes comments only on files the pull request changes, \
             so {which} {} still {}.",
            if n == 1 { "is" } else { "are" },
            if n == 1 { "a draft" } else { "drafts" },
        ));
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
    /// What to focus on, trimmed; `None` when the box was left empty.
    pub instructions: Option<String>,
    /// Review the uncommitted work as well as the commits.
    pub include_uncommitted: bool,
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

impl ReviewGroupSummary {
    /// When the review ran: when it started, or was asked for if it has not.
    pub fn ran_at(&self) -> jiff::Timestamp {
        self.group.started_at.unwrap_or(self.group.created_at)
    }
}

/// Durable finding state plus the compact provenance needed to identify it
/// in the ordinary conversation and Files thread surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFindingSummary {
    pub finding: perch_github::Finding,
    pub severity: perch_github::FindingSeverity,
    /// Every reviewer which has reported or verified this identity, as
    /// backend, model and effort.
    pub attributions: Vec<(String, String, String)>,
    /// Review numbers containing an observation of this finding.
    pub seen_in_reviews: Vec<u64>,
    /// The latest observation's reviewer, retained for compact callers.
    pub backend: Option<String>,
    pub model: Option<String>,
    pub original_review_number: Option<u64>,
    /// What the reviewer called it when it first reported it. Kept apart
    /// from the thread, which a dismissal can empty of the draft that
    /// said it.
    pub title: String,
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
    /// Ask or Work, where the item allows Work; see `composer_mode`.
    pub mode: perch_github::RunMode,
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

impl LiveDoc {
    /// What `editor`, built over `doc`, holds.
    pub fn of(editor: &floem::views::TextEditor, doc: RwSignal<Arc<Doc>>) -> Self {
        Self {
            handle: editor.doc(),
            doc,
            cursor: editor.editor().cursor,
            content: editor.editor().editor_view_id.get_untracked(),
        }
    }
}

/// The file an editor shows. A reload of it under the same scope is set
/// in here and edited into the editor where it stands, rather than
/// building the editor again; see `editor::edits_in_place`. Read it when
/// it is used, never keep what it held.
pub type LiveFile = RwSignal<Arc<LoadedFile>>;

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

/// What each of the worktree's language servers last said, by kind. A
/// kind with no entry has said nothing: no file of its language has asked
/// for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerStatuses(std::collections::BTreeMap<perch_lsp::ServerKind, perch_lsp::Status>);

impl ServerStatuses {
    pub fn get(&self, kind: perch_lsp::ServerKind) -> Option<&perch_lsp::Status> {
        self.0.get(&kind)
    }

    pub fn set(&mut self, kind: perch_lsp::ServerKind, status: perch_lsp::Status) {
        self.0.insert(kind, status);
    }

    /// The kinds held back until the reader trusts the code, in order.
    pub fn held(&self) -> Vec<perch_lsp::ServerKind> {
        self.0
            .iter()
            .filter(|(_, status)| **status == perch_lsp::Status::Held)
            .map(|(kind, _)| *kind)
            .collect()
    }

    /// Every server's status, as its status line says it, for a bug
    /// report: `none started` when none has said anything.
    pub fn describe(&self) -> String {
        if self.0.is_empty() {
            return "none started".to_string();
        }
        self.0
            .iter()
            .map(|(kind, status)| status.describe(*kind))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Whether the server for `path`'s language can be asked about it.
    pub fn usable_for(&self, path: &str) -> bool {
        perch_lsp::ServerKind::for_path(path)
            .and_then(|kind| self.get(kind))
            .is_some_and(perch_lsp::Status::is_usable)
    }

    /// Whether any server can be asked, as a name in prose may be of all
    /// of them.
    pub fn any_usable(&self) -> bool {
        self.0.values().any(perch_lsp::Status::is_usable)
    }

    /// Why the server for `path` can't be asked, naming it, or `None`
    /// when it can. A file of a language with no server has none to ask.
    pub fn unusable_reason(&self, path: &str) -> Option<String> {
        let Some(kind) = perch_lsp::ServerKind::for_path(path) else {
            return Some("no language server for this file".to_string());
        };
        match self.get(kind) {
            Some(status) if status.is_usable() => None,
            Some(status) => Some(status.describe(kind)),
            None => Some(format!("{} not started", kind.name())),
        }
    }
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
    /// What GitHub's timeline says was done to the issue. A pull
    /// request's are in its discussion instead.
    pub issue_events: RwSignal<Arc<Vec<perch_github::PullEvent>>>,
    /// Whether GitHub says the reader may close the issue: `None` until
    /// it answers, when only the issue's author is offered the close.
    pub issue_closable: RwSignal<Option<bool>>,
    /// The head the pull request moved to while the worktree had work in
    /// it. The files on screen are the older ones.
    pub stale_head: RwSignal<Option<String>>,
    /// The pull request could not be fetched; this is the checkout as it
    /// was last left.
    pub offline: RwSignal<bool>,
    /// GitHub has news about the item that the view has not fetched. Set
    /// by a notification, cleared by opening the item or asking for it.
    pub upstream_changed: RwSignal<bool>,
    /// The head GitHub says the pull request moved to after the view
    /// opened. Opening it again checks it out.
    pub head_moved: RwSignal<Option<String>>,
    /// True from the click until the review view is left, so the progress
    /// card can show before the pull request itself has resolved.
    pub opening: RwSignal<bool>,
    pub phase: RwSignal<Phase>,
    pub preparing: RwSignal<Preparing>,
    /// A pull request shown before its checkout: how making the checkout
    /// is going, or why it could not be made, which Code shows in place
    /// of the files. `None` once the files are here, and for every other
    /// open.
    pub files_pending: RwSignal<Option<Preparing>>,
    /// The tabs a pull request shown before its checkout had open last
    /// time, loaded once the files are here.
    pub files_reload: RwSignal<Vec<String>>,
    /// The bar's Review popover: the agents' review of the branch and,
    /// on an open pull request, the reader's own review for GitHub. What
    /// to give the keyboard back to when it shuts.
    pub review_open: RwSignal<bool>,
    pub review_opener: RwSignal<Option<floem::ViewId>>,
    /// Whether the popover shows the agents' settings. On a pull request
    /// they fold to one line under the reader's review; a branch has
    /// nothing else to show, and always shows them.
    pub review_agents_expanded: RwSignal<bool>,
    /// The agent launcher and the immutable history selection are
    /// view-local; durable preferences and groups live on `AppState`.
    pub review_launcher_draft: RwSignal<Option<ReviewLaunchDraft>>,
    /// The launcher's instructions box. Apart from the draft, which is
    /// rebuilt whenever a reviewer's support changes while it is open, and
    /// kept across closing it until another checkout opens.
    pub review_launcher_instructions: RwSignal<String>,
    /// Whether the reader picked the working tree (`Some(true)`) or the
    /// last commit as the head to review; `None` until they pick, when
    /// the launcher decides. Kept like the instructions.
    pub review_launcher_uncommitted: RwSignal<Option<bool>>,
    /// The reader's review for GitHub, under the agents' in the same
    /// popover. Its draft stays with the open pull request so a cancelled
    /// or failed submission can be resumed without retyping it.
    pub submit_review_draft: RwSignal<String>,
    pub submit_review_event: RwSignal<PullReviewEvent>,
    /// The Agents dropdown under the bar's button, what to give the
    /// keyboard back to when it shuts, and the button and dropdown
    /// together, so a press anywhere else shuts it.
    pub agents_dropdown_open: RwSignal<bool>,
    pub agents_dropdown_opener: RwSignal<Option<floem::ViewId>>,
    pub agents_dropdown_anchor: RwSignal<Option<floem::ViewId>>,
    /// The plan usage popover under the bar's gauge, the same way.
    pub usage_open: RwSignal<bool>,
    pub usage_opener: RwSignal<Option<floem::ViewId>>,
    pub usage_anchor: RwSignal<Option<floem::ViewId>>,
    /// Send Feedback's dropdown under the bar's bug icon, the same way.
    /// The draft is the app's (`AppState::feedback`); only whether this
    /// window shows it is here.
    pub feedback_open: RwSignal<bool>,
    pub feedback_opener: RwSignal<Option<floem::ViewId>>,
    pub feedback_anchor: RwSignal<Option<floem::ViewId>>,
    /// The invite popover over the drawer's footer, the same way, and
    /// what is typed in it, kept while it is shut.
    pub invite_open: RwSignal<bool>,
    pub invite_opener: RwSignal<Option<floem::ViewId>>,
    pub invite_anchor: RwSignal<Option<floem::ViewId>>,
    pub invite: RwSignal<crate::invite::InviteDraft>,
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
    /// Conversation or the open file.
    pub tab: RwSignal<ItemTab>,
    /// Each item's pane as it was last left, by item, for the life of the
    /// app.
    pub panes: RwSignal<HashMap<String, Pane>>,
    /// Whether the open item has been on Code, so the tree opens for it
    /// the first time only.
    pub code_seen: RwSignal<bool>,
    /// The tree asked for by the item's first visit to Code, taken by the
    /// editor pane, which holds the backend handle to save it as open.
    pub tree_wanted: RwSignal<bool>,
    /// Whether the Agents section on Conversation is unfolded into the item's
    /// runs.
    pub agents_unfolded: RwSignal<bool>,
    /// The runs open in the Agents section, showing what they did.
    pub agents_open_runs: RwSignal<HashSet<String>>,
    /// A run to bring into view in the Agents section, once its row is laid
    /// out: unfolded, open, and at the top of the page.
    pub pending_run_reveal: RwSignal<Option<String>>,
    /// How far down Conversation was scrolled, kept as it scrolls, so the
    /// page shows where it was left when it is built again: after a file's
    /// tab has shown in its place, or when the item comes back. Written
    /// on every scroll and read only as the page is built, so nothing
    /// tracks it.
    pub conversation_scroll: RwSignal<f64>,
    /// Where the reader was in each open file, by path, kept as its editor
    /// scrolls and its caret moves, so a file shows where it was left when
    /// its tab shows again or its editor is built again. The place is the
    /// tab's, as Show whole is: it goes when the tab closes. Nothing
    /// tracks it, as with `conversation_scroll`.
    pub file_places: RwSignal<HashMap<String, FilePlace>>,
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
    /// The newest Work run on the item that changed something, for the
    /// scope menu's Last run.
    pub last_run: RwSignal<crate::backend::LastRun>,
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
    /// The threads whose resolve or unresolve is not yet answered, by
    /// thread id, and whether resolved is what was asked. The thread shows
    /// as asked until the answer, whatever thread lists land meanwhile,
    /// and takes no second click.
    pub resolving: RwSignal<HashMap<String, bool>>,
    /// Suggestions sent to be committed from Apply suggestion, by token:
    /// `None` while GitHub is asked, then how it went, until the popover
    /// that sent it lets it go.
    pub suggestion_commits: RwSignal<HashMap<u64, Option<Result<(), String>>>>,
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
    /// How the branch stands for a pull request, as the backend last
    /// read it, or why it couldn't.
    pub pull_readiness: RwSignal<Option<Result<crate::backend::PullRequestReadiness, String>>>,
    /// The branches a pull request from the branch could go into.
    pub pull_bases: RwSignal<Option<Result<Vec<String>, String>>>,
    /// Where opening a pull request has got to, while it is under way.
    pub pull_step: RwSignal<Option<crate::backend::OpenStep>>,
    pub pull_error: RwSignal<Option<String>>,
    /// How tall the reader has dragged the description's box, and whether
    /// they are dragging it, as `composer_height` and
    /// `composer_resizing` are for a comment's.
    pub pull_body_height: RwSignal<Option<f64>>,
    pub pull_body_resizing: RwSignal<bool>,
    /// A change of the pull request to a draft (true) or out of one
    /// (false) under way.
    pub draft_pending: RwSignal<Option<bool>>,
    /// A Push to the pull request under way, and what stopped the last.
    pub push_pending: RwSignal<bool>,
    pub push_error: RwSignal<Option<String>>,
    /// An agent writing the pull request's description, what stopped the
    /// last, and an answer waiting on the reader because they had typed.
    pub describing: RwSignal<bool>,
    pub description_error: RwSignal<Option<String>>,
    pub description_offer: RwSignal<Option<(String, String)>>,
    /// Close as issue under way, and what stopped the last.
    pub issue_closing: RwSignal<bool>,
    pub issue_error: RwSignal<Option<String>>,
    /// How tall the reader has dragged the issue's body, and whether they
    /// are dragging it.
    pub issue_body_height: RwSignal<Option<f64>>,
    pub issue_body_resizing: RwSignal<bool>,
    /// An agent writing the issue, what stopped the last, and an answer
    /// waiting on the reader because they had typed.
    pub issue_drafting: RwSignal<bool>,
    pub issue_draft_error: RwSignal<Option<String>>,
    pub issue_draft_offer: RwSignal<Option<(String, String)>>,
    /// The (path, blob) pairs the reader has checked as viewed.
    pub viewed: RwSignal<Arc<std::collections::HashSet<(String, String)>>>,
    /// The open files the reader asked to see whole rather than folded to
    /// their changes, by path. Every file opens folded, and the strip's
    /// Show whole tile asks and unasks for the one that is open; the
    /// choice is the tab's, so it goes when the tab closes and a file
    /// opened again opens folded, and waits with the item's pane while
    /// another item is open. Not stored: a reader unfolds a file to read
    /// it now, not to read it that way every time.
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
    pub loading: RwSignal<HashSet<String>>,
    /// Display line the cursor sits on.
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
    /// A card to focus once Conversation has built and laid it out: a
    /// finding's thread that the findings filter hid until Show asked
    /// for it. The card takes it into `focus_card` then, since one built
    /// after `focus_card` was set ignores it.
    pub pending_focus_card: RwSignal<Option<CardId>>,
    /// A thread whose Delete is waiting for a yes, under its byline.
    pub confirm_thread: RwSignal<Option<String>>,
    /// A comment on GitHub whose Delete is waiting for a yes, under its
    /// text, by its node id.
    pub confirm_comment: RwSignal<Option<String>>,
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
    /// Conversation's threads the reader folded to their header, and the
    /// resolved ones they showed. Conversation keeps its own, as
    /// github.com's Conversation keeps its own from Files changed: a
    /// resolved thread is folded there until it is shown, and any other
    /// is open until it is folded. By thread id, for the app's life.
    pub folded_threads: RwSignal<HashSet<String>>,
    pub shown_resolved: RwSignal<HashSet<String>>,
    /// Every card that has arrived since launch. A thread's card starts
    /// collapsed when the thread is outdated, and that default is applied
    /// once, when the thread first arrives; after that the reader's own
    /// choice stands, through closing the item and opening it again.
    pub known_cards: RwSignal<HashSet<CardId>>,
    /// The backend, model and effort last sent, which the next composer
    /// starts on.
    pub agent_last: RwSignal<Option<AgentChoice>>,
    /// Ask or Work, as last picked on each item, by item id. A new
    /// question on the item starts on it.
    pub work_modes: RwSignal<HashMap<String, perch_github::RunMode>>,
    /// What each Work run changed, by run id, as the backend counted it:
    /// files, lines added, lines removed.
    pub run_changes: RwSignal<HashMap<String, (u32, u32, u32)>>,
    /// Requests Work runs are waiting on the reader for, oldest first,
    /// whichever item they are on.
    pub approvals: RwSignal<Vec<crate::backend::ApprovalView>>,
    /// Each repository's grants, by its identity in lower case.
    pub grants: RwSignal<HashMap<String, Vec<perch_github::WorkGrantRecord>>>,
    /// Who each repository's issues and pull requests can be assigned
    /// to, by its identity in lower case, when the reader may assign
    /// them; none when they may not, and absent until GitHub says.
    pub assignable: RwSignal<HashMap<String, Option<Arc<Vec<perch_github::MentionableUser>>>>>,
    /// The destination last sent from each face, by `ThreadFace::index`:
    /// inline in the code, and on Conversation. A new composer on lines
    /// starts on its face's. Kept for the app's life, as `agent_last` is.
    pub last_destination: [RwSignal<Option<Destination>>; 2],
    /// The agent section of config, `AppState::agent`'s own signal once
    /// the app has wired it, so a follow-up under an agent's answer opens
    /// on a model config offers, at an effort it takes.
    pub agent: RwSignal<perch_github::AgentConfig>,
    /// The display line the pointer is over in the gutter, for the `+`,
    /// or the first row of a divider, for its Fold again.
    pub hovered_line: RwSignal<Option<usize>>,
    /// The element that recorded `hovered_line`. A divider is several
    /// elements, its expanders in the gutter and the parts of the bar over
    /// the code, and floem delivers the enter to the next before the
    /// leave from the last; a leave clears the line only when it comes
    /// from the element that set it, so a move between the parts of one
    /// divider keeps its Fold again.
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
    /// How tall the reader has dragged the composer's box by its grip, or
    /// None while it is the height it opens at and grows with what is
    /// typed. Once dragged it is that tall and stays there, and the next
    /// composer opens at the height the last one was left: the size is
    /// the reader's, not this question's.
    pub composer_height: RwSignal<Option<f64>>,
    /// Whether the box's bottom edge is being dragged. The cursor follows
    /// what is under the pointer, and the drag runs over the code and the
    /// footer, which each want their own, so the card keeps the resize
    /// cursor up itself for the duration.
    pub composer_resizing: RwSignal<bool>,
    /// The composer's Comment was clicked once and the footer is asking
    /// whether to post to the pull request; the next Enter or Post does.
    pub composer_confirming: RwSignal<bool>,
    /// What the confirm up is for, when `composer_confirming`: closing
    /// the pull request, with the text as a comment when there is any,
    /// rather than only posting the text. Read under the confirm only.
    pub composer_closing: RwSignal<bool>,
    /// Why an issue closed from the composer is closed: what was last
    /// picked in the close menu, completed until then. It stays with the
    /// item, as github.com's pick stays with the page.
    pub composer_close_reason: RwSignal<perch_github::IssueCloseReason>,
    /// The menu under the close button's arrow, while it is up.
    pub close_menu: RwSignal<Option<crate::review::close_menu::CloseMenu>>,
    /// What is typed into the close menu's search for the issue a
    /// duplicate repeats.
    pub close_duplicate_query: RwSignal<String>,
    /// An issue comment in flight, with the token its answer carries. The
    /// composer stays open and still until the answer comes.
    pub posting: RwSignal<Option<Posting>>,
    /// Why the last issue comment did not post, under the composer's
    /// box until the text changes or the composer closes.
    pub composer_error: RwSignal<Option<String>>,
    pub lsp_status: RwSignal<ServerStatuses>,
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
    /// The server's outline of each file it has been asked about, by path,
    /// for the lines pinned over the code.
    pub symbols: RwSignal<HashMap<String, Arc<Outline>>>,
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
    /// Whether the find bar is over Conversation's page. Its query and
    /// toggles are the code's bar's; the box, its keyboard and the rest
    /// above are shared too, since only one of the bars is ever built.
    pub conversation_find_open: RwSignal<bool>,
    /// Every match of the query on the page, in the order it reads; the
    /// page's text tints them. Empty while the bar is closed.
    pub conversation_find_hits: RwSignal<std::rc::Rc<Vec<crate::review::conversation_find::Hit>>>,
    /// Which of `conversation_find_hits` the bar is on, when it has
    /// walked to one.
    pub conversation_find_current: RwSignal<Option<usize>>,
    /// Why a pattern matched nothing on the page, when it did not parse.
    pub conversation_find_error: RwSignal<Option<String>>,
    /// The page's scroll while Conversation is up, for the bar to look
    /// through, scroll and give the keyboard back to.
    pub conversation_page: RwSignal<Option<floem::ViewId>>,
    /// Cmd-P's palette.
    pub go_to: crate::review::go_to::GoTo,
    /// The list an `@` in a composer puts up.
    pub mentions: crate::review::mention::Mentions,
    /// The files each item has had on screen, by item, the most recent
    /// first, for Cmd-P to list before anything is typed: a file moves to
    /// the front whenever it becomes the active one, from a tab, the
    /// tree, a definition or anywhere else. Kept for the life of the app,
    /// as `panes` is.
    pub recent_files: RwSignal<HashMap<String, Vec<String>>>,
    /// A jump waiting to be made, as the file, the head line and the column
    /// the caret lands on. The view with the file-opening closure takes it.
    pub pending_jump: RwSignal<Option<(String, usize, usize)>>,
    /// A file to open with no line to go to, as a link that names only the
    /// file opens it. Taken by the same view as `pending_jump`.
    pub pending_open: RwSignal<Option<String>>,
    /// Whether the file being opened is a jump's or a link's, which name
    /// a place in the code: see `open_for_jump`.
    opening_for_jump: RwSignal<bool>,
    /// What rust-analyzer's workspace search made of each name written in
    /// a comment or an answer, by the name as written, for the item's life.
    pub names: RwSignal<HashMap<String, Named>>,
    /// Names asked about and not yet sent, with each ask's token, drained
    /// by the root view, which holds the backend handle.
    pub pending_names: RwSignal<Vec<(u64, Vec<NameAsk>)>>,
    /// A notice for the popup, drained by the root view, which holds the
    /// app state: why a goto opened nothing, for one.
    pub pending_notice: RwSignal<Option<String>>,
    /// An offer to install the server a goto wanted, drained by the root
    /// view as the notice is.
    pub pending_offer: RwSignal<Option<crate::state::ServerOffer>>,
    /// A scope for the diff, asked for by a commit a comment links,
    /// drained by the root view, which holds the backend handle.
    pub pending_scope: RwSignal<Option<Scope>>,
    /// The servers to start again, as their installs land, drained by the
    /// root view, which holds the backend.
    pub pending_server_starts: RwSignal<Vec<perch_lsp::ServerKind>>,
    /// The servers that have just said they are ready, drained by the
    /// root view, which asks again what they answered with nothing while
    /// they loaded. See `ask_again`.
    pub pending_ready: RwSignal<Vec<perch_lsp::ServerKind>>,
    /// A head line to scroll to once the file it belongs to is loaded.
    pub jump_to_line: RwSignal<Option<usize>>,
    /// Where the caret lands after `jump_to_line`, as the head line and a
    /// column, when the jump is a navigation and not a scroll: a jump to a
    /// definition puts the caret on it, so the next jump remembers leaving
    /// from there. Read by the jump, and cleared by it.
    pub jump_caret: RwSignal<Option<(usize, usize)>>,
    /// Page Up, Page Down, Cmd-Up or Cmd-Down, pressed. Conversation or
    /// the open file's editor, whichever is showing, scrolls and clears it.
    pub paging: RwSignal<Option<Paging>>,
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

/// How many files an item's recently opened list keeps.
const MAX_RECENT: usize = 50;

impl Default for ReviewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewState {
    /// Keep what the server of `kind` last said.
    pub fn set_server_status(&self, kind: perch_lsp::ServerKind, status: perch_lsp::Status) {
        self.lsp_status
            .update(|statuses| statuses.set(kind, status));
    }

    pub fn new() -> Self {
        Self {
            pull_request: RwSignal::new(None),
            item: RwSignal::new(None),
            ready: RwSignal::new(false),
            issue: RwSignal::new(None),
            issue_comments: RwSignal::new(Arc::new(Vec::new())),
            issue_events: RwSignal::new(Arc::new(Vec::new())),
            issue_closable: RwSignal::new(None),
            stale_head: RwSignal::new(None),
            offline: RwSignal::new(false),
            upstream_changed: RwSignal::new(false),
            head_moved: RwSignal::new(None),
            opening: RwSignal::new(false),
            phase: RwSignal::new(Phase::Preparing),
            preparing: RwSignal::new(Preparing::Idle),
            files_pending: RwSignal::new(None),
            files_reload: RwSignal::new(Vec::new()),
            review_open: RwSignal::new(false),
            review_opener: RwSignal::new(None),
            review_agents_expanded: RwSignal::new(false),
            review_launcher_draft: RwSignal::new(None),
            review_launcher_instructions: RwSignal::new(String::new()),
            review_launcher_uncommitted: RwSignal::new(None),
            submit_review_draft: RwSignal::new(String::new()),
            submit_review_event: RwSignal::new(PullReviewEvent::Comment),
            agents_dropdown_open: RwSignal::new(false),
            agents_dropdown_opener: RwSignal::new(None),
            agents_dropdown_anchor: RwSignal::new(None),
            usage_open: RwSignal::new(false),
            usage_opener: RwSignal::new(None),
            usage_anchor: RwSignal::new(None),
            feedback_open: RwSignal::new(false),
            feedback_opener: RwSignal::new(None),
            feedback_anchor: RwSignal::new(None),
            invite_open: RwSignal::new(false),
            invite_opener: RwSignal::new(None),
            invite_anchor: RwSignal::new(None),
            invite: RwSignal::new(Default::default()),
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
            panes: RwSignal::new(HashMap::new()),
            code_seen: RwSignal::new(false),
            tree_wanted: RwSignal::new(false),
            agents_unfolded: RwSignal::new(false),
            agents_open_runs: RwSignal::new(HashSet::new()),
            pending_run_reveal: RwSignal::new(None),
            conversation_scroll: RwSignal::new(0.0),
            file_places: RwSignal::new(HashMap::new()),
            layout: RwSignal::new(Layout::Stacked),
            as_source: RwSignal::new(HashSet::new()),
            scope: RwSignal::new(Scope::Branch),
            scope_gen: RwSignal::new(None),
            commits: RwSignal::new(Arc::new(Vec::new())),
            dirty: RwSignal::new(Dirt::default()),
            since_head: RwSignal::new(None),
            last_run: RwSignal::new(crate::backend::LastRun::None),
            discussion: RwSignal::new(None),
            reactions: RwSignal::new(HashMap::new()),
            reacting: RwSignal::new(HashSet::new()),
            resolving: RwSignal::new(HashMap::new()),
            suggestion_commits: RwSignal::new(HashMap::new()),
            checks: RwSignal::new(None),
            pull_request_overview: RwSignal::new(None),
            pull_request_overview_loading: RwSignal::new(false),
            pull_request_overview_error: RwSignal::new(None),
            pull_request_status_generation: RwSignal::new(0),
            merge_pending: RwSignal::new(false),
            merge_error: RwSignal::new(None),
            merge_attempt_id: RwSignal::new(None),
            pull_readiness: RwSignal::new(None),
            pull_bases: RwSignal::new(None),
            pull_step: RwSignal::new(None),
            pull_error: RwSignal::new(None),
            pull_body_height: RwSignal::new(None),
            pull_body_resizing: RwSignal::new(false),
            draft_pending: RwSignal::new(None),
            push_pending: RwSignal::new(false),
            push_error: RwSignal::new(None),
            describing: RwSignal::new(false),
            description_error: RwSignal::new(None),
            description_offer: RwSignal::new(None),
            issue_closing: RwSignal::new(false),
            issue_error: RwSignal::new(None),
            issue_body_height: RwSignal::new(None),
            issue_body_resizing: RwSignal::new(false),
            issue_drafting: RwSignal::new(false),
            issue_draft_error: RwSignal::new(None),
            issue_draft_offer: RwSignal::new(None),
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
            loading: RwSignal::new(HashSet::new()),
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
            pending_focus_card: RwSignal::new(None),
            confirm_thread: RwSignal::new(None),
            confirm_comment: RwSignal::new(None),
            expanded_runs: RwSignal::new(std::collections::HashSet::new()),
            card_rows: RwSignal::new(HashMap::new()),
            collapsed_cards: RwSignal::new(HashSet::new()),
            opened_cards: RwSignal::new(HashSet::new()),
            folded_threads: RwSignal::new(HashSet::new()),
            shown_resolved: RwSignal::new(HashSet::new()),
            known_cards: RwSignal::new(HashSet::new()),
            agent_last: RwSignal::new(None),
            work_modes: RwSignal::new(HashMap::new()),
            run_changes: RwSignal::new(HashMap::new()),
            approvals: RwSignal::new(Vec::new()),
            grants: RwSignal::new(HashMap::new()),
            assignable: RwSignal::new(HashMap::new()),
            last_destination: [RwSignal::new(None), RwSignal::new(None)],
            agent: RwSignal::new(perch_github::AgentConfig::default()),
            hovered_line: RwSignal::new(None),
            hovered_by: RwSignal::new(None),
            composer_focused: RwSignal::new(false),
            composer_draft: RwSignal::new(String::new()),
            composer_autofocus: RwSignal::new(true),
            composer_box: RwSignal::new(None),
            composer_height: RwSignal::new(None),
            composer_resizing: RwSignal::new(false),
            composer_confirming: RwSignal::new(false),
            composer_closing: RwSignal::new(false),
            composer_close_reason: RwSignal::new(Default::default()),
            close_menu: RwSignal::new(None),
            close_duplicate_query: RwSignal::new(String::new()),
            posting: RwSignal::new(None),
            composer_error: RwSignal::new(None),
            lsp_status: RwSignal::new(ServerStatuses::default()),
            request_token: RwSignal::new(0),
            pointer_symbol: RwSignal::new(None),
            hover: RwSignal::new(None),
            highlights: RwSignal::new(Arc::new(Vec::new())),
            highlight_ask: RwSignal::new(None),
            symbols: RwSignal::new(HashMap::new()),
            find_open: RwSignal::new(false),
            find_query: RwSignal::new(String::new()),
            find_options: RwSignal::new(Default::default()),
            find_matches: RwSignal::new(Arc::new(Vec::new())),
            find_current: RwSignal::new(None),
            find_error: RwSignal::new(None),
            find_focused: RwSignal::new(false),
            find_box: RwSignal::new(None),
            find_focus: RwSignal::new(false),
            conversation_find_open: RwSignal::new(false),
            conversation_find_hits: RwSignal::new(std::rc::Rc::new(Vec::new())),
            conversation_find_current: RwSignal::new(None),
            conversation_find_error: RwSignal::new(None),
            conversation_page: RwSignal::new(None),
            go_to: crate::review::go_to::GoTo::new(),
            mentions: crate::review::mention::Mentions::new(),
            recent_files: RwSignal::new(HashMap::new()),
            pending_jump: RwSignal::new(None),
            pending_open: RwSignal::new(None),
            opening_for_jump: RwSignal::new(false),
            names: RwSignal::new(HashMap::new()),
            pending_names: RwSignal::new(Vec::new()),
            pending_notice: RwSignal::new(None),
            pending_offer: RwSignal::new(None),
            pending_scope: RwSignal::new(None),
            pending_server_starts: RwSignal::new(Vec::new()),
            pending_ready: RwSignal::new(Vec::new()),
            jump_to_line: RwSignal::new(None),
            jump_caret: RwSignal::new(None),
            paging: RwSignal::new(None),
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
    /// The item and `opening` are the caller's to set. Its pane is kept,
    /// for the next time it opens.
    fn reset_checkout(&self) {
        self.remember_pane();
        self.files_pending.set(None);
        self.files_reload.set(Vec::new());
        self.pull_request.set(None);
        self.upstream_changed.set(false);
        self.head_moved.set(None);
        self.ready.set(false);
        self.issue.set(None);
        self.issue_comments.set(Arc::new(Vec::new()));
        self.issue_events.set(Arc::new(Vec::new()));
        self.issue_closable.set(None);
        self.composer_close_reason.set(Default::default());
        self.close_menu.set(None);
        self.stale_head.set(None);
        self.offline.set(false);
        self.review_open.set(false);
        self.review_opener.set(None);
        self.review_agents_expanded.set(false);
        self.review_launcher_draft.set(None);
        self.review_launcher_instructions.set(String::new());
        self.review_launcher_uncommitted.set(None);
        self.submit_review_draft.set(String::new());
        self.submit_review_event.set(PullReviewEvent::Comment);
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
        self.last_run.set(crate::backend::LastRun::None);
        self.discussion.set(None);
        self.reactions.set(HashMap::new());
        self.reacting.set(HashSet::new());
        self.resolving.set(HashMap::new());
        self.suggestion_commits.set(HashMap::new());
        self.checks.set(None);
        self.pull_request_overview.set(None);
        self.pull_request_overview_loading.set(false);
        self.pull_request_overview_error.set(None);
        self.pull_request_status_generation.set(0);
        self.merge_pending.set(false);
        self.merge_error.set(None);
        self.merge_attempt_id.set(None);
        self.pull_readiness.set(None);
        self.pull_bases.set(None);
        self.pull_step.set(None);
        self.pull_error.set(None);
        self.draft_pending.set(None);
        self.push_pending.set(false);
        self.push_error.set(None);
        self.describing.set(false);
        self.description_error.set(None);
        self.description_offer.set(None);
        self.issue_closing.set(false);
        self.issue_error.set(None);
        self.issue_drafting.set(false);
        self.issue_draft_error.set(None);
        self.issue_draft_offer.set(None);
        self.viewed.set(Arc::new(std::collections::HashSet::new()));
        // Only written when they change: the open editor rebuilds its
        // document on either, and it is about to go.
        if !self.whole.with_untracked(HashSet::is_empty) {
            self.whole.set(HashSet::new());
        }
        if !self.reveals.with_untracked(HashMap::is_empty) {
            self.reveals.set(HashMap::new());
        }
        self.conversation_scroll.set(0.0);
        self.file_places.set(HashMap::new());
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
        self.loading.set(HashSet::new());
        self.changed_only.set(true);
        self.lsp_status.set(ServerStatuses::default());
        self.comment_threads.set(Arc::new(Vec::new()));
        self.hidden_review_threads.set(HashSet::new());
        self.review_finding_filter.set(ReviewFindingFilter::Open);
        self.composer.set(None);
        self.sent_composer.set(None);
        self.selection.set(None);
        self.focus_card.set(None);
        self.pending_focus_card.set(None);
        self.confirm_thread.set(None);
        self.confirm_comment.set(None);
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
        self.symbols.set(HashMap::new());
        self.last_position.set(None);
        // The names were looked up in this checkout's worktree.
        self.names.set(HashMap::new());
        self.pending_names.set(Vec::new());
        self.pending_open.set(None);
        self.pending_scope.set(None);
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
        self.conversation_find_open.set(false);
        // Cmd-P's palette goes too; what each item had open stays.
        self.go_to.reset();
        // And an `@`'s list, with the people GitHub found in the item's
        // repository.
        self.mentions.reset();
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
            self.lsp_status.set(ServerStatuses::default());
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

    /// Show Conversation or Code. Code with no file open opens none, and
    /// the pane says so; the item's first visit to Code asks for the tree
    /// beside it, to pick one from.
    pub fn show_tab(&self, tab: ItemTab) {
        if tab == ItemTab::Code && !self.code_seen.get_untracked() {
            self.code_seen.set(true);
            self.tree_wanted.set(true);
        }
        // A write of the same value would still rebuild the pane.
        if self.tab.get_untracked() != tab {
            self.tab.set(tab);
        }
    }

    /// Whether the open item has a checkout, and so files to open, or
    /// is a pull request whose checkout is on its way. Tracked.
    pub fn has_files(&self) -> bool {
        self.files_pending.with(Option::is_some)
            || self
                .item
                .with(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()))
    }

    /// Keep the open item's pane for the next time it opens.
    fn remember_pane(&self) {
        let Some(id) = self
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        else {
            return;
        };
        let pane = Pane {
            tab: self.tab.get_untracked(),
            tabs: self.tabs.get_untracked(),
            active: self.active.get_untracked(),
            whole: self.whole.get_untracked(),
            conversation_scroll: self.conversation_scroll.get_untracked(),
            agents_unfolded: self.agents_unfolded.get_untracked(),
            agents_open_runs: self.agents_open_runs.get_untracked(),
            file_places: self.file_places.get_untracked(),
            code_seen: self.code_seen.get_untracked(),
        };
        self.panes.update(|panes| {
            panes.insert(id, pane);
        });
    }

    /// Put back the pane the open item was last left with, or Conversation
    /// alone the first time it opens. Answers the files its tabs hold, the
    /// active one first, which are loading as it returns: the caller asks
    /// the backend for them.
    pub fn restore_pane(&self) -> Vec<String> {
        let pane = self
            .item
            .with_untracked(|i| {
                i.as_ref()
                    .and_then(|i| self.panes.with_untracked(|panes| panes.get(&i.id).cloned()))
            })
            .unwrap_or_default();
        let mut paths: Vec<String> = pane.tabs.iter().map(|t| t.path.clone()).collect();
        if let Some(at) = pane
            .active
            .as_ref()
            .and_then(|active| paths.iter().position(|p| p == active))
        {
            let active = paths.remove(at);
            paths.insert(0, active);
        }
        // One rebuild of a pane on screen, not one for each write.
        floem::reactive::Effect::batch(|| {
            self.tab.set(pane.tab);
            self.tabs.set(pane.tabs);
            self.active.set(pane.active);
            self.cursor_line.set(0);
            if !pane.whole.is_empty() {
                self.whole.set(pane.whole);
            }
            self.conversation_scroll.set(pane.conversation_scroll);
            self.agents_unfolded.set(pane.agents_unfolded);
            self.agents_open_runs.set(pane.agents_open_runs);
            self.file_places.set(pane.file_places);
            self.code_seen.set(pane.code_seen);
            self.tree_wanted.set(false);
            if !paths.is_empty() {
                self.loading
                    .update(|loading| loading.extend(paths.iter().cloned()));
            }
        });
        paths
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
                    self.review_open.set(false);
                    self.review_agents_expanded.set(false);
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
                match current
                    .reviews
                    .iter_mut()
                    .find(|known| known.id == review.id)
                {
                    Some(known) => *known = review.clone(),
                    None => current.reviews.push(review.clone()),
                }
                *discussion = Some(Arc::new(current));
            });
        }

        if owns_surface && !self.review_open.get_untracked() {
            if let Some(opener) = self.review_opener.get_untracked() {
                opener.request_focus();
            }
            self.review_opener.set(None);
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

    /// Whether GitHub lets the reader delete the line comment with this
    /// node id, as the discussion read it. Tracked.
    pub fn may_delete_line_comment(&self, node_id: &str) -> bool {
        self.line_comment_allows(node_id, |c| c.viewer_can_delete)
    }

    /// Whether GitHub lets the reader edit the line comment with this
    /// node id, as the discussion read it. Tracked.
    pub fn may_update_line_comment(&self, node_id: &str) -> bool {
        self.line_comment_allows(node_id, |c| c.viewer_can_update)
    }

    /// Whether the discussion holds a line comment with this node id that
    /// `allows` says yes to. Tracked.
    fn line_comment_allows(
        &self,
        node_id: &str,
        allows: impl Fn(&perch_github::ReviewComment) -> bool,
    ) -> bool {
        self.discussion.with(|d| {
            d.as_ref().is_some_and(|d| {
                d.review_comments
                    .iter()
                    .any(|c| c.node_id == node_id && allows(c))
            })
        })
    }

    /// Where what has this node id on GitHub is shown: the description,
    /// a review's summary, a comment in the conversation or on a line.
    /// `None` when nothing shown has it.
    pub fn github_comment_place(&self, node_id: &str) -> Option<CommentPlace> {
        if node_id.is_empty() {
            return None;
        }
        if self
            .issue
            .with_untracked(|i| i.as_ref().is_some_and(|i| i.node_id == node_id))
        {
            return Some(CommentPlace::IssueBody);
        }
        if self
            .issue_comments
            .with_untracked(|comments| comments.iter().any(|c| c.node_id == node_id))
        {
            return Some(CommentPlace::Conversation);
        }
        self.discussion
            .with_untracked(|d| d.as_ref()?.place_of(node_id))
            .or_else(|| {
                self.comment_threads
                    .with_untracked(|threads| {
                        threads
                            .iter()
                            .flat_map(|t| &t.messages)
                            .any(|m| m.node_id.as_deref() == Some(node_id))
                    })
                    .then_some(CommentPlace::Line)
            })
    }

    /// Show `body` for what the reader is editing on GitHub, before
    /// GitHub answers, wherever it is shown.
    pub fn rewrite_github_comment(&self, node_id: &str, body: &str) {
        if node_id.is_empty() {
            return;
        }
        if self
            .issue
            .with_untracked(|i| i.as_ref().is_some_and(|i| i.node_id == node_id))
        {
            self.issue.update(|i| {
                if let Some(i) = i {
                    Arc::make_mut(i).body = body.to_string();
                }
            });
        }
        if self
            .issue_comments
            .with_untracked(|comments| comments.iter().any(|c| c.node_id == node_id))
        {
            self.issue_comments.update(|comments| {
                for c in Arc::make_mut(comments)
                    .iter_mut()
                    .filter(|c| c.node_id == node_id)
                {
                    c.body = body.to_string();
                }
            });
        }
        if self
            .discussion
            .with_untracked(|d| d.as_ref().is_some_and(|d| d.place_of(node_id).is_some()))
        {
            self.discussion.update(|d| {
                if let Some(d) = d {
                    Arc::make_mut(d).set_body(node_id, body);
                }
            });
        }
        let holds = |t: &CommentThread| {
            t.messages
                .iter()
                .any(|m| m.node_id.as_deref() == Some(node_id))
        };
        if self
            .comment_threads
            .with_untracked(|threads| threads.iter().any(holds))
        {
            self.comment_threads.update(|threads| {
                for m in Arc::make_mut(threads)
                    .iter_mut()
                    .flat_map(|t| t.messages.iter_mut())
                    .filter(|m| m.node_id.as_deref() == Some(node_id))
                {
                    m.body = body.to_string();
                }
            });
        }
    }

    /// Take a comment the reader is deleting on GitHub out of what is
    /// shown, before GitHub answers. One in the conversation leaves a line
    /// in the timeline where it was, `actor` deleting it, as GitHub's own
    /// timeline will. One on a line goes from its thread, and the thread
    /// with it when nothing else was under it. Where the comment was, or
    /// `None` when nothing shown has that id.
    pub fn take_out_github_comment(&self, node_id: &str, actor: &str) -> Option<CommentPlace> {
        if node_id.is_empty() {
            return None;
        }
        let deletion = |author: String| {
            PullEvent::new(
                PullEventKind::CommentDeleted { author },
                jiff::Timestamp::now(),
                actor,
            )
        };
        let mut place = None;
        let issue_author = self.issue_comments.with_untracked(|comments| {
            comments
                .iter()
                .find(|c| c.node_id == node_id)
                .map(|c| c.user.clone())
        });
        if let Some(author) = issue_author {
            self.issue_comments
                .update(|comments| Arc::make_mut(comments).retain(|c| c.node_id != node_id));
            self.issue_events
                .update(|events| Arc::make_mut(events).push(deletion(author)));
            place = Some(CommentPlace::Conversation);
        }
        let in_discussion = self.discussion.with_untracked(|d| {
            d.as_ref().and_then(|d| {
                let author = d
                    .comments
                    .iter()
                    .find(|c| c.node_id == node_id)
                    .map(|c| c.user.clone());
                let line = d.review_comments.iter().any(|c| c.node_id == node_id);
                (author.is_some() || line).then_some(author)
            })
        });
        if let Some(author) = in_discussion {
            place = place.or(Some(if author.is_some() {
                CommentPlace::Conversation
            } else {
                CommentPlace::Line
            }));
            self.discussion.update(|d| {
                if let Some(d) = d {
                    let d = Arc::make_mut(d);
                    d.comments.retain(|c| c.node_id != node_id);
                    d.review_comments.retain(|c| c.node_id != node_id);
                    if let Some(author) = author {
                        d.events.push(deletion(author));
                    }
                }
            });
        }
        let holds = |t: &CommentThread| {
            t.messages
                .iter()
                .any(|m| m.node_id.as_deref() == Some(node_id))
        };
        if self
            .comment_threads
            .with_untracked(|threads| threads.iter().any(holds))
        {
            self.comment_threads.update(|threads| {
                let threads = Arc::make_mut(threads);
                threads.retain_mut(|t| {
                    if !holds(t) {
                        return true;
                    }
                    t.messages.retain(|m| m.node_id.as_deref() != Some(node_id));
                    !t.messages.is_empty()
                });
            });
            place = place.or(Some(CommentPlace::Line));
        }
        place
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
        backend.hover(
            token,
            &symbol.path,
            symbol.line as u32,
            symbol.column as u32,
        );
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
        if !self
            .lsp_status
            .with_untracked(|s| s.usable_for(&symbol.path))
        {
            return;
        }
        let token = self.next_token();
        backend.document_highlight(
            token,
            &symbol.path,
            symbol.line as u32,
            symbol.column as u32,
        );
        self.highlight_ask.set(Some((token, symbol)));
    }

    /// The server of `kind` is ready, or ready again. The hover under the
    /// pointer and the uses of the symbol at the caret were answered with
    /// nothing if they were asked while it loaded, and nothing asks them
    /// again until the pointer or the caret moves, so whichever of them
    /// it answered with nothing is asked again here. So is what the panel
    /// under the editor shows the server's error for: the references, or
    /// a row of the call hierarchy.
    pub fn ask_again(&self, backend: &crate::backend::Backend, kind: perch_lsp::ServerKind) {
        let served = |symbol: &Symbol| perch_lsp::ServerKind::for_path(&symbol.path) == Some(kind);
        let references = self
            .references
            .get_untracked()
            .filter(|r| r.failed.is_some() && !r.pending)
            .and_then(|r| Some((r.asked.clone().filter(served)?, r.kind)));
        if let Some((symbol, asked_for)) = references {
            crate::review::references::find(*self, backend, &symbol, asked_for);
        }
        if let Some(hierarchy) = self.hierarchy.get_untracked() {
            let served_item = |item: &perch_lsp::lsp_types::CallHierarchyItem| {
                perch_lsp::client::uri_to_path(item.uri.as_str())
                    .and_then(|path| perch_lsp::ServerKind::for_path(&path.to_string_lossy()))
                    == Some(kind)
            };
            let failed: Vec<usize> = (0..hierarchy.rows.len())
                .filter(|&at| {
                    let row = &hierarchy.rows[at];
                    row.failed.is_some()
                        && row.expanded
                        && row.pending.is_none()
                        && served_item(&row.item)
                })
                .collect();
            if !failed.is_empty() {
                let mut open = (*hierarchy).clone();
                for at in failed {
                    let token = self.ask_calls(open.rows[at].item.clone(), open.direction);
                    open.expanding(at, token);
                }
                self.hierarchy.set(Some(Arc::new(open)));
            }
        }
        let highlight = self
            .highlight_ask
            .get_untracked()
            .map(|(_, symbol)| symbol)
            .filter(|symbol| served(symbol) && self.highlights.with_untracked(|h| h.is_empty()));
        if let Some(symbol) = highlight {
            self.ask_highlights(backend, symbol);
        }
        let hover = self.hover.get_untracked().filter(|h| {
            h.markdown.is_empty()
                && served(&h.symbol)
                && self
                    .pointer_symbol
                    .with_untracked(|p| p.as_ref() == Some(&h.symbol))
        });
        if let Some(hover) = hover {
            // The slot is let go first: a symbol with one is not asked.
            self.hover.set(None);
            self.ask_hover(backend, hover.symbol);
        }
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
        self.pending_jump
            .set(Some((path.to_string(), line, column)));
    }

    /// `Ctrl--`: back to where the last jump left.
    pub fn go_back(&self) {
        let Some(current) = self.current_place() else {
            return;
        };
        let place = self.history.try_update(|h| h.back(current)).flatten();
        if let Some(place) = place {
            self.pending_jump.set(Some((place.path, place.line, 0)));
        }
    }

    /// `Ctrl-Shift--`: forward again.
    pub fn go_forward(&self) {
        let Some(current) = self.current_place() else {
            return;
        };
        let place = self.history.try_update(|h| h.forward(current)).flatten();
        if let Some(place) = place {
            self.pending_jump.set(Some((place.path, place.line, 0)));
        }
    }

    /// Open a location rust-analyzer named: the file, at the line, with
    /// the caret on the symbol.
    pub fn open_located(&self, located: &Located) {
        self.navigate_to(
            &located.path,
            located.line.saturating_sub(1),
            located.character,
        );
    }

    /// Go to a file a comment or an answer names, on Code: at the head
    /// line and column it names, or, named alone, as the tree opens it,
    /// with no jump to unfold anything. The place left is remembered for
    /// `Ctrl--`, unless the file named alone is the one open, which
    /// leaves nothing. A line is gone to straight: Code with no file open
    /// does not open its first change on the way.
    pub fn go_to_file(&self, path: &str, at: Option<(usize, usize)>) {
        match at {
            Some((line, column)) => {
                self.navigate_to(path, line, column);
                if self.tab.get_untracked() != ItemTab::Code {
                    self.tab.set(ItemTab::Code);
                }
            }
            None => {
                if self.tab.get_untracked() != ItemTab::Code {
                    self.show_tab(ItemTab::Code);
                }
                let from = self.current_place().filter(|from| from.path != path);
                if let Some(from) = from {
                    self.history.update(|h| h.push(from));
                }
                self.pending_open.set(Some(path.to_string()));
            }
        }
    }

    /// The SHAs, in full, of the commits the review knows: the pull
    /// request's, as GitHub's timeline lists them, and the checkout's
    /// over the base, among them a branch's that are not pushed. Tracked.
    pub fn known_commits(&self) -> Vec<String> {
        let mut known: Vec<String> = self.discussion.with(|d| {
            d.iter()
                .flat_map(|d| &d.events)
                .filter_map(|e| match &e.kind {
                    PullEventKind::Committed { sha, .. } => Some(sha.clone()),
                    _ => None,
                })
                .collect()
        });
        self.commits
            .with(|c| known.extend(c.iter().map(|c| c.sha.clone())));
        known.sort_unstable();
        known.dedup();
        known
    }

    /// The subject of a commit the review knows, by its SHA, in full or
    /// cut short.
    pub fn commit_subject(&self, sha: &str) -> Option<String> {
        let checkout = self.commits.with_untracked(|c| {
            c.iter()
                .find(|c| c.sha.starts_with(sha))
                .map(|c| c.subject.clone())
        });
        checkout.or_else(|| {
            self.discussion.with_untracked(|d| {
                d.iter()
                    .flat_map(|d| &d.events)
                    .find_map(|e| match &e.kind {
                        PullEventKind::Committed {
                            sha: s, subject, ..
                        } if s.starts_with(sha) => Some(subject.clone()),
                        _ => None,
                    })
            })
        })
    }

    /// Show a commit's changes by themselves on Files, by its SHA, in
    /// full or cut short, when the checkout has it: whether it does. A
    /// checkout that has not fetched a push yet lacks its commits.
    pub fn show_commit(&self, sha: &str) -> bool {
        let full = self.commits.with_untracked(|c| {
            c.iter()
                .find(|c| c.sha.starts_with(sha))
                .map(|c| c.sha.clone())
        });
        let Some(sha) = full else {
            return false;
        };
        self.pending_scope.set(Some(Scope::Commit { sha }));
        self.show_tab(ItemTab::Code);
        true
    }

    /// Go to what a name in a comment or an answer names, on Code: one
    /// definition opens, and several open the panel as its definitions.
    pub fn go_to_definitions(&self, name: &str, locations: &[Located]) {
        if self.tab.get_untracked() != ItemTab::Code {
            self.show_tab(ItemTab::Code);
        }
        match locations {
            [] => {}
            [one] => self.open_located(one),
            several => self.open_references(References {
                kind: Locations::Definitions,
                symbol: name.to_string(),
                locations: several.to_vec(),
                pending: false,
                token: self.next_token(),
                failed: None,
                asked: None,
            }),
        }
    }

    /// Ask the workspace search about the names not yet asked about, in
    /// one batch under one token.
    pub fn ask_names(&self, asks: &[NameAsk]) {
        let mut seen = HashSet::new();
        let fresh: Vec<NameAsk> = self.names.with_untracked(|names| {
            asks.iter()
                .filter(|ask| !names.contains_key(&ask.key) && seen.insert(ask.key.clone()))
                .cloned()
                .collect()
        });
        if fresh.is_empty() {
            return;
        }
        let token = self.next_token();
        floem::reactive::Effect::batch(|| {
            self.names.update(|names| {
                for ask in &fresh {
                    names.insert(ask.key.clone(), Named::Asked(token));
                }
            });
            self.pending_names
                .update(|pending| pending.push((token, fresh)));
        });
    }

    /// The names' answers landed. Each is kept for the ask still waiting
    /// on it: an item opened since has asked afresh, or not at all.
    pub fn finish_names(&self, token: u64, answers: Vec<(String, Option<Found>)>) {
        self.names.update(|names| {
            for (key, answer) in answers {
                if let Some(named) = names.get_mut(&key).filter(|n| **n == Named::Asked(token)) {
                    *named = answer.map_or(Named::Missing, Named::Found);
                }
            }
        });
    }

    /// The server is ready, or ready again: what it found nothing for may
    /// have been asked before it had read the workspace, so it is asked
    /// again by whatever still shows it.
    pub fn forget_missing_names(&self) {
        if self
            .names
            .with_untracked(|n| n.values().any(|n| *n == Named::Missing))
        {
            self.names
                .update(|names| names.retain(|_, n| *n != Named::Missing));
        }
    }

    /// A goto's answer landed. One location opens; several open the panel
    /// under the ask's kind, and the reader picks. None, or the server's
    /// error, says so in a notice: a click that opens nothing and says
    /// nothing reads as a click that missed.
    pub fn finish_goto(&self, token: u64, answer: Result<Vec<Located>, String>) {
        let ask = self
            .goto
            .try_update(|g| g.take_if(|g| g.token == token))
            .flatten();
        let Some(ask) = ask else { return };
        let locations = match answer {
            Ok(locations) => locations,
            Err(error) => {
                self.pending_notice.set(Some(format!(
                    "No {} found for `{}` ({error})",
                    ask.noun, ask.symbol
                )));
                return;
            }
        };
        match locations.as_slice() {
            [] => self
                .pending_notice
                .set(Some(format!("No {} found for `{}`", ask.noun, ask.symbol))),
            [one] => self.open_located(one),
            _ => self.open_references(References {
                kind: ask.kind,
                symbol: ask.symbol,
                locations,
                pending: false,
                token,
                failed: None,
                asked: None,
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
    pub fn ask_calls(
        &self,
        item: perch_lsp::lsp_types::CallHierarchyItem,
        direction: Direction,
    ) -> u64 {
        let token = self.next_token();
        self.pending_calls
            .update(|p| p.push((token, item, direction)));
        token
    }

    /// The server named what a call hierarchy can start from. Nothing
    /// named opens nothing; the first item opens the panel as its callers,
    /// with the calls on their way. The server's error, that it is still
    /// loading say, is said in a notice, as a goto's is.
    pub fn finish_prepare_hierarchy(
        &self,
        token: u64,
        items: Result<Vec<perch_lsp::lsp_types::CallHierarchyItem>, String>,
        place: Option<Located>,
    ) {
        let ask = self
            .hierarchy_ask
            .try_update(|a| a.take_if(|a| a.token == token))
            .flatten();
        let Some(ask) = ask else { return };
        let items = match items {
            Ok(items) => items,
            Err(error) => {
                self.pending_notice.set(Some(format!(
                    "No call hierarchy for `{}` ({error})",
                    ask.symbol
                )));
                return;
            }
        };
        let Some(root) = items.into_iter().next() else {
            return;
        };
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
        let Some(hierarchy) = self.hierarchy.get_untracked() else {
            return;
        };
        let Some(row) = hierarchy.rows.get(at) else {
            return;
        };
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
        let Some(hierarchy) = self.hierarchy.get_untracked() else {
            return;
        };
        let mut open = (*hierarchy).clone();
        open.collapse(at);
        self.hierarchy.set(Some(Arc::new(open)));
    }

    /// Turn the call hierarchy the other way: the same root, its tree
    /// started again.
    pub fn turn_hierarchy(&self, direction: Direction) {
        let Some(hierarchy) = self.hierarchy.get_untracked() else {
            return;
        };
        if hierarchy.direction == direction {
            return;
        }
        let token = self.ask_calls(hierarchy.root.clone(), direction);
        self.hierarchy
            .set(Some(Arc::new(hierarchy.turned(direction, token))));
    }

    /// The calls under a row arrived, or the server's error did.
    pub fn finish_calls(&self, token: u64, calls: Result<Vec<Call>, String>) {
        self.hierarchy.update(|h| {
            if let Some(h) = h.as_mut() {
                let mut open = (**h).clone();
                let filled = match calls {
                    Ok(calls) => open.fill(token, calls),
                    Err(error) => open.fail(token, error),
                };
                if filled.is_some() {
                    *h = Arc::new(open);
                }
            }
        });
    }

    /// The references asked for under `token` arrived, or the server's
    /// error did. Matched against the panel's own token: the shared
    /// counter moves on with every hover while the answer is on its way.
    pub fn finish_references(&self, token: u64, locations: Result<Vec<Located>, String>) {
        self.references.update(|r| {
            if let Some(r) = r.as_mut().filter(|r| r.token == token) {
                let mut open = (**r).clone();
                match locations {
                    Ok(locations) => open.locations = locations,
                    Err(error) => open.failed = Some(error),
                }
                open.pending = false;
                *r = Arc::new(open);
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
    /// pane shows in place of the editor, and what the strip's tile
    /// describes.
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
        self.loading.with(|m| m.contains(path))
    }

    /// Mark `path` loading unless it is loaded or loading already.
    /// Returns true when the caller should ask the backend to load it.
    pub fn begin_load(&self, path: &str) -> bool {
        let wanted = !self.files.with_untracked(|m| m.contains_key(path))
            && !self.loading.with_untracked(|m| m.contains(path));
        if wanted {
            self.loading.update(|m| {
                m.insert(path.to_string());
            });
        }
        wanted
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
                self.forget_place(&dropped.path);
            }
        }
        self.active.set(Some(path.to_string()));
        self.remember_recent(path);
        self.cursor_line.set(0);
        // The tint was another file's, or another caret's.
        self.clear_highlights();
        self.editor_epoch.update(|e| *e += 1);
        self.keep_composer_in_view(composer_was_in_code);

        self.begin_load(path)
    }

    /// The open file changed under an open composer. One that was drawn in
    /// the code of the file that was open, `was_in_code`, would now be
    /// hidden, so show Conversation, where new questions and follow-ups
    /// always have a home. One that was not, a question about the item as
    /// a whole say, or a card in a file other than the open one, is not
    /// hidden by the change: the reader chose Files over it, and keeps it.
    ///
    /// A jump or a link opening the file is the exception: the reader
    /// asked for a place in the code, so Code stays, and the composer
    /// waits in its file, a card again when they come back to it.
    fn keep_composer_in_view(&self, was_in_code: bool) {
        if self.opening_for_jump.get_untracked() {
            return;
        }
        let hidden = was_in_code && !self.composer_in_code_untracked();
        if hidden && self.tab.get_untracked() != ItemTab::Conversation {
            self.show_tab(ItemTab::Conversation);
        }
    }

    /// Open a file, through `open`, for a jump or a link: Code stays up
    /// over a composer drawn in the file it leaves.
    pub fn open_for_jump<R>(&self, open: impl FnOnce() -> R) -> R {
        self.opening_for_jump.set(true);
        let opened = open();
        self.opening_for_jump.set(false);
        opened
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
            if let Some(neighbour) = &neighbour {
                self.remember_recent(neighbour);
            }
            self.active.set(neighbour);
            self.cursor_line.set(0);
            self.editor_epoch.update(|e| *e += 1);
            self.keep_composer_in_view(composer_was_in_code);
        }
        // So was the place in the file: opened again, it opens at its top.
        // Forgotten once its editor is no longer the one showing, which
        // folding it again above may have moved the caret of.
        self.forget_place(path);
    }

    /// The files the open item has had on screen, the most recent first.
    /// Tracked.
    pub fn recent(&self) -> Vec<String> {
        let Some(id) = self.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
            return Vec::new();
        };
        self.recent_files
            .with(|all| all.get(&id).cloned())
            .unwrap_or_default()
    }

    /// A file became the active one: first among the item's recent files.
    /// Not written when it is first already, so nothing that lists them
    /// is woken for a tab clicked twice.
    fn remember_recent(&self, path: &str) {
        let Some(id) = self
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        else {
            return;
        };
        let first = self.recent_files.with_untracked(|all| {
            all.get(&id)
                .and_then(|recent| recent.first())
                .is_some_and(|p| p == path)
        });
        if first {
            return;
        }
        self.recent_files.update(|all| {
            let recent = all.entry(id).or_default();
            recent.retain(|p| p != path);
            recent.insert(0, path.to_string());
            recent.truncate(MAX_RECENT);
        });
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

    /// A tab is gone: its file opens at its top next time.
    fn forget_place(&self, path: &str) {
        self.file_places.update(|places| {
            places.remove(path);
        });
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
        // New sides of a file already loaded, as an agent's edit brings,
        // are edited into its editor in place when they can be: a new
        // editor draws a frame at the top of the file before the reader's
        // place is put back.
        let in_place = self.files.with_untracked(|m| {
            m.get(path)
                .is_some_and(|old| crate::review::editor::edits_in_place(old, &file))
        });
        self.files.update(|m| {
            m.insert(path.to_string(), file);
        });
        if !in_place && self.active.with(|a| a.as_deref() == Some(path)) {
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
        let paths = self.reload_loaded(|_| true);
        self.files.set(HashMap::new());
        self.editor_epoch.update(|e| *e += 1);
        paths
    }

    /// The scope read again with a new list under it: the working tree
    /// was written to, or the head moved. The files asked for again stay
    /// on screen until their new sides arrive, so each edit of an agent at
    /// work does not flash the open file to its loading view.
    ///
    /// With the head where it was, a file whose entry in the list is the
    /// same, blob and counts, has the same sides, and is kept as it is: a
    /// file reloaded builds its editor again, which draws a frame at the
    /// top of the file before the reader's place is put back. One loading
    /// is asked for again all the same, since its answer is for the old
    /// generation and will be dropped. `all` is for a head that moved,
    /// where a base side can change under an unchanged entry, and a file
    /// outside the list can be another file.
    pub fn reread_scope(&self, files: Arc<Vec<ChangedFile>>, all: bool) -> Vec<String> {
        let old = self.changed_files.get_untracked();
        self.set_changed_files(files.clone());
        if all {
            return self.reload_loaded(|_| true);
        }
        let entry =
            |list: &[ChangedFile], path: &str| list.iter().find(|f| f.path == path).cloned();
        let loaded = self.files.get_untracked();
        let loading = self.loading.get_untracked();
        self.reload_loaded(|path| {
            !loaded.contains_key(path)
                || loading.contains(path)
                || entry(&old, path) != entry(&files, path)
        })
    }

    /// The open item came ready again on a checkout read anew, its head
    /// moved say. A file on its way was asked of the last generation and
    /// will be dropped, and one loaded may be the old head's: all are
    /// asked for again, and stay on screen until their new sides arrive.
    pub fn reload_all(&self) -> Vec<String> {
        self.reload_loaded(|_| true)
    }

    /// Every loaded file, not only the tabs: a thread card on
    /// Conversation shows lines from a file no tab has, and would go
    /// blank until something loaded it again. And every file on its way,
    /// whose answer is for the old generation and will be dropped. Then
    /// every tab, the active one first. Those `wanted` says are all
    /// marked loading.
    fn reload_loaded(&self, wanted: impl Fn(&str) -> bool) -> Vec<String> {
        let mut paths: Vec<String> = self.files.with(|m| m.keys().cloned().collect());
        for path in self.loading.with(|m| m.iter().cloned().collect::<Vec<_>>()) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
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
        paths.retain(|p| wanted(p));
        self.loading.set(paths.iter().cloned().collect());
        paths
    }

    /// Whether a changed file's content has been marked viewed.
    pub fn is_viewed(&self, file: &ChangedFile) -> bool {
        match file.viewed_key() {
            Some(key) => self
                .viewed
                .with(|v| v.contains(&(file.path.clone(), key.to_string()))),
            None => false,
        }
    }

    /// How many of the changed files are viewed, and how many there are.
    ///
    /// A file with nothing to key Viewed on is in neither number.
    pub fn viewed_count(&self) -> (usize, usize) {
        self.changed_files.with(|files| {
            let viewable = files.iter().filter(|f| f.viewed_key().is_some());
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
                .find(|f| f.viewed_key().is_some() && !self.is_viewed(f))
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
                .filter(|thread| !thread.outdated && self.thread_visible(thread))
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
                .filter(|thread| !thread.outdated && self.thread_visible(thread))
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
                .find(|t| t.id == thread_id && !t.outdated && self.thread_visible(t))
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

    /// Whether the card is a resolved thread's, or one the reader has just
    /// asked to resolve. Tracked.
    fn thread_resolved(&self, card: &CardId) -> bool {
        let CardId::Thread(id) = card else {
            return false;
        };
        self.is_resolved(id)
    }

    /// Whether a thread is resolved: as asked while the ask is out, else
    /// as the threads say. Tracked on both.
    pub fn is_resolved(&self, thread_id: &str) -> bool {
        if let Some(asked) = self.resolving.with(|r| r.get(thread_id).copied()) {
            return asked;
        }
        self.comment_threads
            .with(|threads| threads.iter().any(|t| t.id == thread_id && t.resolved))
    }

    /// Whether a resolve or unresolve of the thread is not yet answered.
    /// Tracked.
    pub fn is_resolving(&self, thread_id: &str) -> bool {
        self.resolving.with(|r| r.contains_key(thread_id))
    }

    /// The reader resolves a thread, or unresolves it: the thread shows
    /// it before the backend is asked. A thread resolved here folds, in
    /// the code and on Conversation, as one resolved on GitHub arrives
    /// folded, whatever the reader opened before. Whether to ask: not
    /// while an earlier ask about the thread is unanswered.
    pub fn begin_resolve(&self, thread_id: &str, resolve: bool) -> bool {
        if self.resolving.with_untracked(|r| r.contains_key(thread_id)) {
            return false;
        }
        let card = CardId::Thread(thread_id.to_string());
        floem::reactive::Effect::batch(|| {
            if resolve && self.opened_cards.with_untracked(|c| c.contains(&card)) {
                self.opened_cards.update(|c| {
                    c.remove(&card);
                });
            }
            if resolve
                && self
                    .shown_resolved
                    .with_untracked(|s| s.contains(thread_id))
            {
                self.shown_resolved.update(|s| {
                    s.remove(thread_id);
                });
            }
            self.resolving.update(|r| {
                r.insert(thread_id.to_string(), resolve);
            });
        });
        true
    }

    /// The ask about a thread was answered, or did not go: the thread
    /// shows as the thread list says, which the answer came after.
    pub fn settle_resolution(&self, thread_id: &str) {
        if self.resolving.with_untracked(|r| r.contains_key(thread_id)) {
            self.resolving.update(|r| {
                r.remove(thread_id);
            });
        }
    }

    /// A suggestion was sent to be committed under `token`.
    pub fn start_suggestion_commit(&self, token: u64) {
        self.suggestion_commits.update(|c| {
            c.insert(token, None);
        });
    }

    /// How the commit sent under `token` went, kept while its popover
    /// is still waiting on it.
    pub fn settle_suggestion_commit(&self, token: u64, result: Result<(), String>) {
        if self
            .suggestion_commits
            .with_untracked(|c| c.contains_key(&token))
        {
            self.suggestion_commits.update(|c| {
                c.insert(token, Some(result));
            });
        }
    }

    /// The commit sent under `token` is no longer waited on.
    pub fn forget_suggestion_commit(&self, token: u64) {
        if self
            .suggestion_commits
            .with_untracked(|c| c.contains_key(&token))
        {
            self.suggestion_commits.update(|c| {
                c.remove(&token);
            });
        }
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

    /// Whether a thread on Conversation is folded to its header. A
    /// resolved thread's is, unless the reader showed it; any other's is
    /// when the reader folded it. Tracked.
    pub fn is_folded(&self, thread_id: &str) -> bool {
        if self.is_resolved(thread_id) {
            return !self.shown_resolved.with(|s| s.contains(thread_id));
        }
        self.folded_threads.with(|f| f.contains(thread_id))
    }

    /// A thread's header on Conversation was clicked, or Show resolved or
    /// Hide resolved: a folded thread opens, an open one folds. A
    /// resolved thread shown is no longer one the reader folded, so it
    /// stays open if it is unresolved.
    pub fn toggle_fold(&self, thread_id: &str) {
        if self.is_resolved(thread_id) {
            let showing = self
                .shown_resolved
                .with_untracked(|s| !s.contains(thread_id));
            floem::reactive::Effect::batch(|| {
                self.shown_resolved.update(|s| {
                    if !s.remove(thread_id) {
                        s.insert(thread_id.to_string());
                    }
                });
                if showing
                    && self
                        .folded_threads
                        .with_untracked(|f| f.contains(thread_id))
                {
                    self.folded_threads.update(|f| {
                        f.remove(thread_id);
                    });
                }
            });
            return;
        }
        self.folded_threads.update(|f| {
            if !f.remove(thread_id) {
                f.insert(thread_id.to_string());
            }
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
    #[cfg(test)]
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
        self.change_run(run_id, |run| {
            crate::ask::apply_run_event(run, event);
        });
    }

    /// The reader told run `run_id` something while it worked.
    pub fn apply_steer(&self, run_id: &str, text: &str) {
        self.change_run(run_id, |run| crate::ask::apply_steer(run, text));
    }

    /// Change run `run_id` where a message holds it. Every agent's steps
    /// come through here, so a run no thread shown holds, as another
    /// item's, changes nothing and tells no one.
    fn change_run(&self, run_id: &str, change: impl Fn(&mut perch_github::RunRecord)) {
        let holds = |t: &CommentThread| {
            t.messages
                .iter()
                .any(|m| m.run.as_ref().is_some_and(|r| r.id == run_id))
        };
        if !self
            .comment_threads
            .with_untracked(|threads| threads.iter().any(holds))
        {
            return;
        }
        self.comment_threads.update(|threads| {
            for run in Arc::make_mut(threads)
                .iter_mut()
                .flat_map(|t| t.messages.iter_mut())
                .filter_map(|m| m.run.as_mut().filter(|r| r.id == run_id))
            {
                change(run);
            }
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
            Scope::Run { after, .. } => after,
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
            let mode = self.composer_mode(thread_id.as_deref());
            self.composer.set(Some(Composer {
                thread_id,
                anchor,
                destination,
                draft,
                opening,
                editing,
                mode,
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
        self.close_menu.set(None);
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
    /// What a composer starts on: a follow-up on its thread's last run;
    /// a new question on what was last picked on the item, else Work on
    /// a branch Perch made, which is there to be worked on, and Ask on
    /// anything else. Whether Work is allowed at all is the view's and the
    /// backend's to say.
    fn composer_mode(&self, thread_id: Option<&str>) -> perch_github::RunMode {
        use perch_github::RunMode;
        let last = thread_id.and_then(|id| {
            self.comment_threads.with_untracked(|threads| {
                threads
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.last_mode())
            })
        });
        if let Some(mode) = last {
            return mode;
        }
        self.item.with_untracked(|item| {
            let Some(item) = item else {
                return RunMode::Ask;
            };
            if let Some(mode) = self.work_modes.with_untracked(|m| m.get(&item.id).copied()) {
                return mode;
            }
            let made_here =
                item.subject.is_none() && item.checkout.as_ref().is_some_and(|c| c.owned);
            if made_here {
                RunMode::Work
            } else {
                RunMode::Ask
            }
        })
    }

    /// Switch the open composer between Ask and Work, and remember it for
    /// the item.
    pub fn set_composer_mode(&self, mode: perch_github::RunMode) {
        self.composer.update(|c| {
            if let Some(c) = c {
                c.mode = mode;
            }
        });
        if let Some(id) = self
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        {
            self.work_modes.update(|m| {
                m.insert(id, mode);
            });
        }
    }

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
        self.start_sending(token, body, false, false);
    }

    /// The open composer's suggestion is going into the worktree's file,
    /// settled by `settle_issue_comment` as a comment is.
    pub fn start_applying(&self, token: u64, body: String) {
        self.start_sending(token, body, false, true);
    }

    /// The pull request is being closed from the open composer, with
    /// its text as a comment first when there is any. The composer is
    /// still until `settle_issue_comment`, as for a comment.
    pub fn start_closing(&self, token: u64, body: String) {
        self.start_sending(token, body, true, false);
    }

    fn start_sending(&self, token: u64, body: String, closing: bool, applying: bool) {
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
            closing,
            applying,
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
        let sent_from = self
            .posting
            .with_untracked(|p| p.as_ref().filter(|p| p.token == token).map(|p| p.opening));
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
            // No thread comes of a comment or a suggestion put in the
            // file, so a card's rows are not kept for one.
            Ok(()) => {
                self.composer_sent();
                self.drop_sent_placeholder();
            }
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

    /// Whether the composer's text, sent to GitHub, posts at once as a
    /// reply: it answers a thread that is on GitHub, as github.com's
    /// Reply does, rather than waiting in a draft for the review. A draft
    /// being edited stays one. Untracked.
    pub fn posts_reply(&self, composer: &Composer) -> bool {
        let Some(id) = composer.thread_id.as_deref() else {
            return false;
        };
        composer.editing.is_none()
            && self.comment_threads.with_untracked(|t| {
                t.iter()
                    .find(|t| t.id == id)
                    .is_some_and(|t| t.github_id.is_some())
            })
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
    /// request or an issue as a whole, where it posts as an issue
    /// comment. About lines of a branch there is nowhere to go.
    /// Untracked.
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
            self.comment_threads
                .with_untracked(|t| t.iter().any(|t| t.id == id && t.github_id.is_some()))
        });
        if on_github {
            return open_pull;
        }
        let subject = self.pull_request.with_untracked(Option::is_some)
            || self.issue.with_untracked(Option::is_some);
        subject && self.posts_issue_comment_about(thread_id, anchor)
    }

    /// Whether a message from `thread` has anywhere on GitHub to go, as
    /// `github_offered` asks it of a composer. A thread with lines or a
    /// place on GitHub takes a draft, which rides in a review, so the
    /// pull request has to be open to take one. A thread of Perch's own
    /// with neither has nowhere to reply, so it posts as an issue
    /// comment about the pull request instead, which a closed one takes
    /// as well. An issue takes only the comment. What Post to GitHub on
    /// an agent's message is offered on, and Reply on a reviewer's. A
    /// tracked read of the pull request and the issue, which land after
    /// the threads do.
    pub fn github_offered_on(&self, thread: &CommentThread) -> bool {
        let takes_draft = thread.anchor.is_some() || thread.github_id.is_some();
        let on_issue = self.issue.with(Option::is_some);
        self.pull_request.with(|pull| match pull.as_ref() {
            None => on_issue && !takes_draft,
            Some(pull) => !takes_draft || pull.is_open(),
        })
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
                .filter(|thread| !thread.outdated && self.thread_visible(thread))
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
                .filter(|thread| !thread.outdated && self.thread_visible(thread))
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

    /// Whether a thread is drawn. One holding a draft always is: the draft
    /// posts with the review, and nothing posts from out of sight. One with
    /// nothing in it never is, as a finding's is once its draft is deleted.
    /// Otherwise the findings filter says.
    pub fn thread_visible(&self, thread: &CommentThread) -> bool {
        if thread.messages.iter().any(|m| m.is_draft()) {
            return true;
        }
        !thread.messages.is_empty()
            && !self
                .hidden_review_threads
                .with(|hidden| hidden.contains(&thread.id))
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
        perch_github::Side::Left => display_line_of_base_line(file, line),
    }
}

/// Where a zero-based base line appears in the unified display, as
/// `display_line_of_anchor_line` maps the left side.
pub(crate) fn display_line_of_base_line(file: &LoadedFile, line: usize) -> Option<usize> {
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

    /// A file asks the server of its own language, and a name in prose
    /// any server that can answer.
    #[test]
    fn a_file_is_served_by_its_own_languages_server() {
        use perch_lsp::{ServerKind, Status};
        let mut statuses = ServerStatuses::default();
        assert!(!statuses.usable_for("src/lib.rs"));
        assert!(!statuses.any_usable());
        assert_eq!(
            statuses.unusable_reason("src/lib.rs").as_deref(),
            Some("rust-analyzer not started")
        );
        statuses.set(ServerKind::RustAnalyzer, Status::Starting);
        assert_eq!(
            statuses.unusable_reason("src/lib.rs").as_deref(),
            Some("rust-analyzer starting")
        );
        statuses.set(ServerKind::RustAnalyzer, Status::Ready);
        assert!(statuses.usable_for("src/lib.rs"));
        assert!(statuses.any_usable());
        assert_eq!(statuses.unusable_reason("src/lib.rs"), None);
        assert!(!statuses.usable_for("README.md"));
        assert!(statuses.unusable_reason("README.md").is_some());
    }

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
    fn viewed_is_by_path_and_blob_and_a_deletion_by_a_fixed_key() {
        let state = state();
        let files = state.changed_files.get();
        assert!(state.is_viewed(&files[1]));
        assert!(!state.is_viewed(&files[0]));
        assert!(
            !state.is_viewed(&changed("b.rs", Some("blob-b2"))),
            "a new blob is new content"
        );
        assert!(!state.is_viewed(&files[2]));
        assert_eq!(state.viewed_count(), (1, 4));

        state.viewed.update(|v| {
            Arc::make_mut(v).insert(("gone.rs".into(), perch_git::DELETED_VIEWED_KEY.into()));
        });
        assert!(state.is_viewed(&files[2]), "a deletion can be marked");
        assert_eq!(state.viewed_count(), (2, 4));
    }

    #[test]
    fn next_unviewed_skips_viewed_files_and_wraps_both_ways() {
        let state = state();
        // b is viewed, so it is stepped over; the deletion is not.
        assert_eq!(
            state.next_unviewed(Some("a.rs"), true).as_deref(),
            Some("gone.rs")
        );
        assert_eq!(
            state.next_unviewed(Some("gone.rs"), true).as_deref(),
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
        assert_eq!(
            state.scroll_to_row.get().as_deref(),
            Some("src/untouched.rs")
        );
    }

    #[test]
    fn the_anchors_in_a_file_are_its_threads_head_side_lines() {
        use perch_github::{CommentThread, Side, ThreadOrigin};
        let at = "2026-09-18T09:59:00Z".parse().unwrap();
        let thread =
            |id: &str, path: &str, start_line: u32, line: u32, side: Side, outdated: bool| {
                CommentThread {
                    id: id.into(),
                    worktree_id: "wt".into(),
                    origin: if id.starts_with("gh-") {
                        ThreadOrigin::GitHub
                    } else {
                        ThreadOrigin::Local
                    },
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
                    node_id: None,
                    can_resolve: false,
                    created_at: at,
                    messages: anchored(id, 1, 1, "2026-09-18T09:59:00Z", false).messages,
                }
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
        assert_eq!(
            anchors_of(&threads, "src/z.rs"),
            Vec::<(usize, usize)>::new()
        );

        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(threads));
        assert_eq!(state.anchors_in("src/y.rs"), vec![(0, 2), (8, 8)]);
    }

    /// The findings filter hides a finding's thread, but never one with a
    /// draft in it, which would post from out of sight; and a thread with
    /// nothing in it, as a finding's is once its draft is deleted, is
    /// never drawn, whatever the filter.
    #[test]
    fn a_draft_is_never_hidden_and_an_empty_thread_never_drawn() {
        use perch_github::ThreadOrigin;
        let at = "2026-09-18T10:00:00Z";
        let finding = anchored("t1", 3, 3, at, false);
        let mut drafted = anchored("t2", 5, 5, at, false);
        drafted.origin = ThreadOrigin::GitHub;
        drafted.messages[0].origin = ThreadOrigin::GitHub;
        assert!(drafted.messages[0].is_draft());
        let mut empty = anchored("t3", 7, 7, at, false);
        empty.messages.clear();

        let state = ReviewState::new();
        state
            .hidden_review_threads
            .set(["t1", "t2", "t3"].map(String::from).into_iter().collect());
        assert!(!state.thread_visible(&finding));
        assert!(state.thread_visible(&drafted));
        assert!(!state.thread_visible(&empty));

        state.hidden_review_threads.set(HashSet::new());
        assert!(state.thread_visible(&finding));
        assert!(state.thread_visible(&drafted));
        assert!(!state.thread_visible(&empty));
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
            node_id: None,
            can_resolve: false,
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
            cards_of(&on_a, None, &cut, &HashSet::new(), &HashMap::new())[0].after_display_line,
            2
        );

        // Collapsed, a card is its fixed rows; laid out, it has the rows the
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
        assert_eq!(state.cards_in("src/x.rs")[1].rows, doc::COLLAPSED_ROWS);
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
            cards_of(&left_on_a, None, &cut, &HashSet::new(), &HashMap::new())[0]
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
        state.show_tab(ItemTab::Code);
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

    /// A line comment being deleted goes from its thread, and the thread
    /// with it when it was the only comment there; no timeline line is
    /// put in for it, GitHub's timeline having none. An id nothing shown
    /// has is nothing to delete.
    #[test]
    fn a_deleted_line_comment_leaves_its_thread_and_an_emptied_thread_goes() {
        let state = ReviewState::new();
        let mut one = github_thread(10, 3, perch_github::Side::Right, "2026-09-18T09:00:00Z", 0);
        one.messages[0].node_id = Some("PRRC_10".into());
        let mut two = github_thread(20, 5, perch_github::Side::Right, "2026-09-18T09:00:00Z", 1);
        two.messages[1].node_id = Some("PRRC_21".into());
        state.comment_threads.set(Arc::new(vec![one, two]));

        assert_eq!(
            state.take_out_github_comment("PRRC_21", "chris"),
            Some(CommentPlace::Line)
        );
        let threads = state.comment_threads.get();
        assert_eq!(threads.len(), 2);
        assert_eq!(
            threads[1].messages.len(),
            1,
            "the reply went, the root stayed"
        );

        assert_eq!(
            state.take_out_github_comment("PRRC_10", "chris"),
            Some(CommentPlace::Line)
        );
        let threads = state.comment_threads.get();
        assert_eq!(
            threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["gh-20"]
        );
        assert!(state.issue_events.get().is_empty());

        assert_eq!(state.take_out_github_comment("PRRC_99", "chris"), None);
        assert_eq!(state.take_out_github_comment("", "chris"), None);
    }

    /// A comment being edited shows its new words wherever it is shown,
    /// and its place names the mutation that edits it; an id nothing
    /// shown has has no place.
    #[test]
    fn an_edited_comment_shows_its_new_words_and_knows_its_place() {
        let state = ReviewState::new();
        let mut thread = github_thread(10, 3, perch_github::Side::Right, "2026-09-18T09:00:00Z", 0);
        thread.messages[0].node_id = Some("PRRC_10".into());
        state.comment_threads.set(Arc::new(vec![thread]));
        state.issue_comments.set(Arc::new(vec![IssueComment {
            id: 1,
            node_id: "IC_1".into(),
            user: "rohan".into(),
            user_avatar_url: String::new(),
            body: "tpyo".into(),
            created_at: "2026-10-03T22:00:00Z".parse().unwrap(),
            updated_at: None,
            html_url: String::new(),
            reactions: Default::default(),
            viewer_can_delete: true,
            viewer_can_update: true,
        }]));

        assert_eq!(
            state.github_comment_place("IC_1"),
            Some(CommentPlace::Conversation)
        );
        assert_eq!(
            state.github_comment_place("PRRC_10"),
            Some(CommentPlace::Line)
        );
        assert_eq!(state.github_comment_place("PRRC_99"), None);
        state
            .discussion
            .set(Some(Arc::new(perch_github::Discussion {
                node_id: "PR_1".into(),
                reviews: vec![Review {
                    id: 9,
                    node_id: "PRR_9".into(),
                    user: "rohan".into(),
                    body: "lgtm".into(),
                    state: "APPROVED".into(),
                    submitted_at: None,
                    html_url: String::new(),
                    reactions: Default::default(),
                    viewer_can_update: true,
                }],
                ..Default::default()
            })));
        assert_eq!(
            state.github_comment_place("PR_1"),
            Some(CommentPlace::PullRequestBody)
        );
        assert_eq!(
            state.github_comment_place("PRR_9"),
            Some(CommentPlace::Review)
        );
        state.rewrite_github_comment("PR_1", "described");
        state.rewrite_github_comment("PRR_9", "approved");
        let d = state.discussion.get().unwrap();
        assert_eq!(
            (d.body.as_str(), d.reviews[0].body.as_str()),
            ("described", "approved")
        );
        assert_eq!(state.github_comment_place(""), None);

        state.rewrite_github_comment("IC_1", "typo");
        state.rewrite_github_comment("PRRC_10", "fixed");
        assert_eq!(state.issue_comments.get()[0].body, "typo");
        assert_eq!(state.comment_threads.get()[0].messages[0].body, "fixed");
    }

    /// An issue's comment being deleted goes, and a line saying who
    /// deleted whose takes its place in the issue's timeline.
    #[test]
    fn a_deleted_issue_comment_leaves_a_line_in_the_timeline() {
        let state = ReviewState::new();
        let comment = |id: u64, node: &str, user: &str| IssueComment {
            id,
            node_id: node.into(),
            user: user.into(),
            user_avatar_url: String::new(),
            body: "spam".into(),
            created_at: "2026-10-03T22:00:00Z".parse().unwrap(),
            updated_at: None,
            html_url: String::new(),
            reactions: Default::default(),
            viewer_can_delete: true,
            viewer_can_update: false,
        };
        state.issue_comments.set(Arc::new(vec![
            comment(1, "IC_1", "kvnloo"),
            comment(2, "IC_2", "rohan"),
        ]));

        assert_eq!(
            state.take_out_github_comment("IC_1", "criccomini"),
            Some(CommentPlace::Conversation)
        );
        let left: Vec<u64> = state.issue_comments.get().iter().map(|c| c.id).collect();
        assert_eq!(left, vec![2]);
        let events = state.issue_events.get();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actor, "criccomini");
        assert_eq!(
            events[0].kind,
            PullEventKind::CommentDeleted {
                author: "kvnloo".into()
            }
        );
    }

    /// One of GitHub's threads on head line `line` of `src/x.rs`,
    /// one-based, with `replies` comments under its first.
    fn github_thread(
        id: u64,
        line: u32,
        side: perch_github::Side,
        at: &str,
        replies: usize,
    ) -> CommentThread {
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
            node_id: None,
            can_resolve: false,
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
        let rows: HashMap<CardId, usize> =
            [(CardId::Thread("gh-20".into()), 6), (CardId::Composer, 12)]
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
        state.show_tab(ItemTab::Code);
        state.open_composer(None, Some(composer.clone()), String::new(), choice.clone());
        assert!(state.composer_is_inline());
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(
            state.composer_card_anchor("src/x.rs"),
            Some(composer.clone())
        );
        assert_eq!(state.composer_card_anchor("src/y.rs"), None);
        assert_eq!(state.cards_in("src/x.rs").len(), 6);
        // A follow-up is inside its thread's card, not a card of its own,
        // and stays on Code when that card is in the open file. A question
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
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.cards_in("src/x.rs").len(), 5);
        state.open_composer(None, None, "whole item".into(), choice.clone());
        assert!(!state.composer_is_inline());
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "whole item");
        assert_eq!(state.cards_in("src/x.rs").len(), 5);

        // A follow-up on a thread whose card is collapsed opens the card,
        // so the box it sits inside is seen.
        state.show_tab(ItemTab::Code);
        state.toggle_card(&CardId::Thread("mine".into()));
        state.open_composer(Some("mine".into()), Some(composer), String::new(), choice);
        assert!(!state.is_collapsed(&CardId::Thread("mine".into())));
        assert_eq!(state.tab.get(), ItemTab::Code);
    }

    #[test]
    fn a_composer_starts_on_work_where_perch_made_the_branch_and_follows_its_thread() {
        use perch_github::RunMode;
        let state = ReviewState::new();
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        let mode = || state.composer.with_untracked(|c| c.as_ref().unwrap().mode);
        let mut item = perch_github::Item::new("a/b", "2026-09-18T10:00:00Z".parse().unwrap());
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "chris/readme".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/x".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item.clone())));
        state.open_composer(None, None, String::new(), choice.clone());
        assert_eq!(
            mode(),
            RunMode::Work,
            "a branch Perch made is there to be worked on"
        );

        // Picked, it is what the next question on the item starts on.
        state.set_composer_mode(RunMode::Ask);
        state.open_composer(None, None, String::new(), choice.clone());
        assert_eq!(mode(), RunMode::Ask);

        // A follow-up goes on as its thread's last run went.
        let mut thread = github_thread(1, 3, perch_github::Side::Right, "2026-09-18T09:00:00Z", 0);
        thread.messages[0].run = Some(perch_github::RunRecord {
            mode: RunMode::Work,
            ..Default::default()
        });
        let thread_id = thread.id.clone();
        state.comment_threads.set(Arc::new(vec![thread]));
        state.open_composer(Some(thread_id), None, String::new(), choice.clone());
        assert_eq!(mode(), RunMode::Work);

        // A root's branch, not one Perch made, starts on Ask.
        item.id = "other".into();
        item.checkout.as_mut().unwrap().owned = false;
        state.item.set(Some(Arc::new(item)));
        state.open_composer(None, None, String::new(), choice);
        assert_eq!(mode(), RunMode::Ask);
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
        assert!(
            state.collapsed_cards.get().is_empty(),
            "opening a resolved thread is its own set"
        );
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

    /// Conversation keeps its own folds, as github.com's Conversation
    /// keeps its own from Files changed: a resolved thread is folded there
    /// until the reader shows it, any other is open until they fold it,
    /// and resolving one folds it wherever it was opened. Nothing done on
    /// Conversation opens or collapses the card in the code.
    #[test]
    fn conversation_folds_a_resolved_thread_until_it_is_shown_and_resolving_folds_it_again() {
        use perch_github::Side;
        let state = ReviewState::new();
        let mut resolved = github_thread(10, 3, Side::Right, "2026-09-18T09:00:00Z", 1);
        resolved.resolved = true;
        let open = github_thread(20, 4, Side::Right, "2026-09-18T09:30:00Z", 0);
        state
            .comment_threads
            .set(Arc::new(vec![resolved.clone(), open.clone()]));
        let card = CardId::Thread("gh-10".into());
        assert!(state.is_folded("gh-10"), "resolved, so folded");
        assert!(!state.is_folded("gh-20"), "open until folded");

        // Shown on Conversation, the resolved thread is open there and
        // still collapsed in the code.
        state.toggle_fold("gh-10");
        assert!(!state.is_folded("gh-10"));
        assert!(state.is_collapsed(&card), "Conversation's fold is its own");
        state.toggle_fold("gh-10");
        assert!(state.is_folded("gh-10"), "Hide resolved folds it again");

        // Any other thread folds from its chevron, and not in the code.
        state.toggle_fold("gh-20");
        assert!(state.is_folded("gh-20"));
        assert!(!state.is_collapsed(&CardId::Thread("gh-20".into())));
        state.toggle_fold("gh-20");
        assert!(!state.is_folded("gh-20"));

        // Resolving a thread the reader showed, or never folded, folds it,
        // before the answer and after.
        state.toggle_fold("gh-10");
        let mut reopened = resolved.clone();
        reopened.resolved = false;
        state
            .comment_threads
            .set(Arc::new(vec![reopened, open.clone()]));
        assert!(
            !state.is_folded("gh-10"),
            "unresolved on GitHub, it stays as the reader left it"
        );
        assert!(state.begin_resolve("gh-10", true));
        assert!(state.is_folded("gh-10"), "resolved here, folded at once");
        state.comment_threads.set(Arc::new(vec![resolved, open]));
        state.settle_resolution("gh-10");
        assert!(
            state.is_folded("gh-10"),
            "and still folded once GitHub has it"
        );
        assert!(state.begin_resolve("gh-20", true));
        assert!(state.is_folded("gh-20"));

        // Unresolving shows the thread, as github.com does.
        state.settle_resolution("gh-20");
        assert!(state.begin_resolve("gh-10", false));
        assert!(!state.is_folded("gh-10"));
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
        state.show_tab(ItemTab::Code);
        state.open_composer(None, Some(anchor.clone()), "why?".into(), choice.clone());
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Code);
        state.open_tab("src/y.rs");
        assert!(!state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "why?");
        state.open_tab("src/x.rs");
        assert!(state.composer_in_code());
        state.show_tab(ItemTab::Code);
        assert_eq!(state.tab.get(), ItemTab::Code);

        // Closing the composer's file for its neighbour does the same.
        state.close_tab("src/x.rs");
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(state.composer_draft.get(), "why?");

        // A follow-up inside a card of the open file likewise.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Code);
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
        assert_eq!(state.tab.get(), ItemTab::Code);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Conversation);

        // Without a composer, changing files leaves Files selected.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Code);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Code);
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
        state.show_tab(ItemTab::Code);
        state.open_tab("src/y.rs");
        assert_eq!(state.tab.get(), ItemTab::Code);
        state.open_tab("src/x.rs");
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.composer_draft.get(), "what does this do?");

        // Closing a tab likewise.
        state.close_tab("src/x.rs");
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert_eq!(state.tab.get(), ItemTab::Code);

        // A composer on lines of a file other than the open one is not in
        // the code either, so opening a third file over it leaves Files
        // too; opening its own file draws it in the code again.
        state.close_composer();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Code);
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
        state.show_tab(ItemTab::Code);
        state.open_tab("src/z.rs");
        assert_eq!(state.tab.get(), ItemTab::Code);
        state.open_tab("src/x.rs");
        assert!(state.composer_in_code());
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.composer_draft.get(), "why?");
    }

    /// Code with no file open opens none, with changes or without. The
    /// item's first visit asks for the tree, and later ones do not, nor
    /// the first again once the item comes back. Closing the last file's
    /// tab leaves Code showing, not Conversation.
    #[test]
    fn code_opens_no_file_asks_for_the_tree_once_and_stays_when_its_last_tab_closes() {
        let state = ReviewState::new();
        let open = |id: &str| {
            let mut item =
                perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
            item.id = id.into();
            state.begin("Reading the worktree");
            state.item.set(Some(Arc::new(item)));
            state.restore_pane();
        };
        open("a");
        state
            .changed_files
            .set(Arc::new(vec![perch_git::ChangedFile {
                path: "src/a.rs".into(),
                old_path: None,
                status: perch_git::ChangeStatus::Modified,
                additions: 1,
                deletions: 0,
                binary: false,
                head_blob: None,
            }]));
        state.show_tab(ItemTab::Code);
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.pending_open.get(), None, "a change opened by itself");
        assert!(
            state.tree_wanted.get(),
            "the first visit did not ask for the tree"
        );

        state.tree_wanted.set(false);
        state.show_tab(ItemTab::Conversation);
        state.show_tab(ItemTab::Code);
        assert!(!state.tree_wanted.get(), "a later visit asked for the tree");

        // Another item's first visit asks; back on the first, none does.
        open("b");
        state.show_tab(ItemTab::Code);
        assert!(
            state.tree_wanted.get(),
            "another item's first visit did not ask"
        );
        open("a");
        assert!(!state.tree_wanted.get(), "the ask followed the item away");
        state.show_tab(ItemTab::Conversation);
        state.show_tab(ItemTab::Code);
        assert!(
            !state.tree_wanted.get(),
            "the item came back and asked again"
        );

        state.open_tab("src/a.rs");
        state.close_tab("src/a.rs");
        assert_eq!(state.active.get(), None);
        assert_eq!(state.tab.get(), ItemTab::Code);
    }

    /// An item opens on Conversation the first time, and after that on
    /// the pane it was left with: Conversation or Code, the files' tabs, the one showing, and the files shown whole. What
    /// its tabs hold is loading, the file showing first.
    #[test]
    fn an_item_opens_on_conversation_the_first_time_and_on_its_pane_after() {
        let state = ReviewState::new();
        let open = |id: &str| {
            let mut item =
                perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
            item.id = id.into();
            state.begin("Reading the worktree");
            state.item.set(Some(Arc::new(item)));
            state.restore_pane()
        };
        let tabs = || {
            state
                .tabs
                .with(|t| t.iter().map(|t| t.path.clone()).collect::<Vec<_>>())
        };

        assert!(open("a").is_empty());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert!(tabs().is_empty());

        // Two files on Code, the second showing and shown whole.
        state.open_tab("src/x.rs");
        state.open_tab("src/y.rs");
        state.show_tab(ItemTab::Code);
        state.toggle_whole();

        // Another item, the first time: Conversation alone.
        assert!(open("b").is_empty());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert!(tabs().is_empty());
        assert_eq!(state.active.get(), None);
        assert!(state.whole.get().is_empty());
        state.open_tab("src/z.rs");

        // Back to the first, as it was left.
        assert_eq!(open("a"), ["src/y.rs", "src/x.rs"]);
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(tabs(), ["src/x.rs", "src/y.rs"]);
        assert_eq!(state.active.get().as_deref(), Some("src/y.rs"));
        assert!(!state.folded("src/y.rs"));
        assert!(state.folded("src/x.rs"));
        assert!(state.is_loading("src/x.rs") && state.is_loading("src/y.rs"));

        // And the second: its file's tab behind Conversation.
        assert_eq!(open("b"), ["src/z.rs"]);
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(tabs(), ["src/z.rs"]);
        assert_eq!(state.active.get().as_deref(), Some("src/z.rs"));
        assert!(state.whole.get().is_empty());
    }

    /// The Agents section, unfolded or not and the runs open in it, waits with
    /// the item's pane while another item is open.
    #[test]
    fn the_agents_row_waits_with_the_items_pane() {
        let state = ReviewState::new();
        let open = |id: &str| {
            let mut item =
                perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
            item.id = id.into();
            state.begin("Reading the worktree");
            state.item.set(Some(Arc::new(item)));
            state.restore_pane()
        };
        open("a");
        assert!(!state.agents_unfolded.get(), "folded the first time");
        state.agents_unfolded.set(true);
        state.agents_open_runs.update(|open| {
            open.insert("run".into());
        });

        open("b");
        assert!(!state.agents_unfolded.get());
        assert!(state.agents_open_runs.with(HashSet::is_empty));

        open("a");
        assert!(state.agents_unfolded.get());
        assert!(state.agents_open_runs.with(|open| open.contains("run")));
    }

    /// Where the reader was, on Conversation and in each file, waits with
    /// the item's pane while another item is open, and a file's place goes
    /// with its tab.
    #[test]
    fn the_places_on_an_items_tabs_wait_with_its_pane() {
        let state = ReviewState::new();
        let open = |id: &str| {
            let mut item =
                perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
            item.id = id.into();
            state.begin("Reading the worktree");
            state.item.set(Some(Arc::new(item)));
            state.restore_pane()
        };
        let place = |y: f64| FilePlace {
            anchor: Some((3, 0.0)),
            x: 0.0,
            y,
            caret: Some((3, 1)),
        };

        open("a");
        state.open_tab("src/x.rs");
        state.open_tab("src/y.rs");
        state.conversation_scroll.set(300.0);
        state.file_places.update(|places| {
            places.insert("src/x.rs".into(), place(66.0));
            places.insert("src/y.rs".into(), place(88.0));
        });

        open("b");
        assert_eq!(state.conversation_scroll.get(), 0.0);
        assert!(state.file_places.with(HashMap::is_empty));

        open("a");
        assert_eq!(state.conversation_scroll.get(), 300.0);
        assert_eq!(
            state.file_places.with(|p| p.get("src/x.rs").copied()),
            Some(place(66.0))
        );

        state.close_tab("src/x.rs");
        assert!(!state.file_places.with(|p| p.contains_key("src/x.rs")));
        assert_eq!(
            state.file_places.with(|p| p.get("src/y.rs").copied()),
            Some(place(88.0))
        );
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
        state.open_composer(None, Some(anchor.clone()), String::new(), choice.clone());
        assert_eq!(state.sent_composer.get(), None);
        assert_eq!(
            state
                .cards_in("src/x.rs")
                .iter()
                .filter(|c| c.id == CardId::Composer)
                .count(),
            1
        );

        // A suggestion put in the file brings no thread, so its rows go
        // with the composer rather than wait under the lines for one.
        state.close_composer();
        state.open_composer(None, Some(anchor), "```suggestion\nx\n```".into(), choice);
        state.set_card_rows(&CardId::Composer, 10);
        state.start_applying(7, "```suggestion\nx\n```".into());
        state.settle_issue_comment(7, Ok(()));
        assert!(state.composer.get().is_none());
        assert_eq!(state.sent_composer.get(), None);
        assert!(!state
            .cards_in("src/x.rs")
            .iter()
            .any(|c| c.id == CardId::Composer));
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
            [
                ("a.rs", "blob-a"),
                ("b.rs", "blob-b"),
                ("gone.rs", perch_git::DELETED_VIEWED_KEY),
                ("c.rs", "blob-c"),
            ]
            .into_iter()
            .map(|(p, b)| (p.to_string(), b.to_string()))
            .collect(),
        ));
        assert_eq!(state.next_unviewed(Some("a.rs"), true), None);
        assert_eq!(state.viewed_count(), (4, 4));

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
        assert_eq!(
            label("/Users/me/Code/other/src/lib.rs"),
            "Code/other/src/lib.rs"
        );
        assert_eq!(label("/opt/vendor/src/lib.rs"), "/opt/vendor/src/lib.rs");
        assert_eq!(
            outside_label(Path::new("/Users/me/Code/other/src/lib.rs"), None),
            "/Users/me/Code/other/src/lib.rs"
        );
    }

    /// Go's module cache labels a module by its path and version, capitals
    /// read back; Go's own library labels by package.
    #[test]
    fn a_go_file_outside_is_labelled_by_module_or_package() {
        use std::path::Path;
        let home = Path::new("/Users/me");
        let label = |p: &str| outside_label(Path::new(p), Some(home));
        assert_eq!(
            label("/Users/me/go/pkg/mod/github.com/spf13/cobra@v1.8.0/command.go"),
            "github.com/spf13/cobra v1.8.0"
        );
        assert_eq!(
            label("/Users/me/go/pkg/mod/github.com/!burnt!sushi/toml@v1.3.2/decode.go"),
            "github.com/BurntSushi/toml v1.3.2"
        );
        assert_eq!(
            label("/opt/homebrew/Cellar/go/1.25.1/libexec/src/net/http/client.go"),
            "go net/http"
        );
        assert_eq!(
            label("/Users/me/go/pkg/mod/golang.org/toolchain@v0.0.1-go1.25.1.darwin-arm64/src/fmt/print.go"),
            "go fmt"
        );
        // A `go` directory is Go's root only with Go's VERSION in it.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("go/src/strings/builder.go");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "").unwrap();
        assert_ne!(outside_label(&file, None), "go strings");
        std::fs::write(dir.path().join("go/VERSION"), "go1.25.1").unwrap();
        assert_eq!(outside_label(&file, None), "go strings");
    }

    /// A file under node_modules is its package, by the last node_modules
    /// on its path, with the version pnpm's store gives.
    #[test]
    fn a_package_file_outside_is_labelled_by_its_package() {
        use std::path::Path;
        let home = Path::new("/Users/me");
        let label = |p: &str| outside_label(Path::new(p), Some(home));
        assert_eq!(
            label("/Users/me/Code/app/node_modules/react/index.d.ts"),
            "react"
        );
        assert_eq!(
            label("/Users/me/Code/app/node_modules/@types/node/fs.d.ts"),
            "@types/node"
        );
        assert_eq!(
            label("/Users/me/Code/app/node_modules/.pnpm/react-dom@18.2.0_react@18.2.0/node_modules/react-dom/index.d.ts"),
            "react-dom 18.2.0"
        );
        assert_eq!(
            label(
                "jdt://contents/guava-33.3.1-jre.jar/com.google.common.collect/ImmutableList.class"
            ),
            "guava-33.3.1-jre.jar"
        );
        assert_eq!(
            label("jdt://contents/java.base/java.util/List.class"),
            "JDK"
        );
        assert_eq!(
            label("/Users/me/Code/app/node_modules/.pnpm/@scope+pkg@1.2.3/node_modules/@scope/pkg/a.d.ts"),
            "@scope/pkg 1.2.3"
        );
        assert_eq!(
            label("/Users/me/.perch/servers/typescript-language-server/6.0.1/node_modules/typescript/lib/lib.dom.d.ts"),
            "typescript"
        );
    }

    #[test]
    fn the_history_goes_back_and_forward_and_drops_the_oldest() {
        let place = |line: usize| Place {
            path: "src/a.rs".into(),
            line,
        };
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
        let ask = |token| GotoAsk {
            token,
            symbol: "x".into(),
            kind: Locations::Definitions,
            noun: "definition",
        };
        state.goto.set(Some(ask(7)));

        // A stale token answers nothing and leaves the ask waiting.
        state.finish_goto(6, Ok(vec![located("src/a.rs", 3)]));
        assert_eq!(state.pending_jump.get(), None);
        assert!(state.goto.get().is_some());

        state.finish_goto(7, Ok(vec![located("src/a.rs", 3)]));
        assert_eq!(state.pending_jump.get(), Some(("src/a.rs".into(), 2, 4)));
        assert!(state.goto.get().is_none(), "the ask is answered");

        state.pending_jump.set(None);
        state.goto.set(Some(ask(8)));
        state.finish_goto(8, Ok(vec![located("src/a.rs", 3), located("src/b.rs", 9)]));
        assert_eq!(
            state.pending_jump.get(),
            None,
            "several open the panel, not a file"
        );
        let panel = state.references.get().expect("the panel opened");
        assert_eq!(panel.kind, Locations::Definitions);
        assert_eq!(panel.symbol, "x");
        assert_eq!(panel.locations.len(), 2);
        assert!(!panel.pending);

        assert_eq!(state.pending_notice.get(), None, "an answer is no news");
    }

    #[test]
    fn a_goto_that_finds_nothing_or_fails_says_so() {
        let state = ReviewState::new();
        let ask = |token, noun| GotoAsk {
            token,
            symbol: "alpha".into(),
            kind: Locations::Definitions,
            noun,
        };

        state.goto.set(Some(ask(9, "definition")));
        state.finish_goto(9, Ok(vec![]));
        assert_eq!(state.pending_jump.get(), None, "none opens nothing");
        assert!(state.references.get().is_none());
        assert_eq!(
            state.pending_notice.get().as_deref(),
            Some("No definition found for `alpha`")
        );

        // rust-analyzer with no workspace loaded answers "content modified".
        state.pending_notice.set(None);
        state.goto.set(Some(ask(10, "type definition")));
        state.finish_goto(
            10,
            Err("the language server reported: content modified".into()),
        );
        assert_eq!(state.pending_jump.get(), None);
        assert_eq!(
            state.pending_notice.get().as_deref(),
            Some(
                "No type definition found for `alpha` \
                 (the language server reported: content modified)"
            )
        );

        // An answer to an ask since replaced says nothing.
        state.pending_notice.set(None);
        state.goto.set(Some(ask(12, "definition")));
        state.finish_goto(11, Ok(vec![]));
        assert_eq!(state.pending_notice.get(), None);
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
            Origin::Outside {
                label: "/opt/vendor/src/lib.rs".into()
            },
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
        assert_eq!(
            under(posted),
            Destination::GitHub,
            "a comment the reader posted"
        );
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
        assert_eq!(
            d,
            Destination::agent(opus_high()),
            "nothing sent from this face yet"
        );
        let d = default_destination(
            None,
            None,
            Some(Destination::GitHub),
            Some(opus_high()),
            sonnet_medium(),
            false,
            &agent,
        );
        assert_eq!(
            d,
            Destination::agent(opus_high()),
            "no anchor: an agent, whatever was last"
        );
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
        state
            .comment_threads
            .set(Arc::new(vec![reply, fresh, mine]));
        let drafts = state.drafts();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].thread_id, "gh-31");
        assert_eq!(drafts[0].message_id, "d1");
        assert_eq!(drafts[0].in_reply_to, Some(31));
        assert_eq!(drafts[0].commit_sha, "h");
        assert_eq!(drafts[0].body, "I will fix it");
        assert_eq!(drafts[1].thread_id, "t2");
        assert_eq!(
            drafts[1].in_reply_to, None,
            "a new thread has no root there yet"
        );
        assert_eq!(
            drafts[1].anchor.as_ref().map(|a| (a.start_line, a.line)),
            Some((5, 6))
        );
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
            drafts_summary(&[
                a_draft("a", Some(1)),
                a_draft("b", None),
                a_draft("c", None)
            ]),
            "1 reply, 2 comments"
        );
        assert_eq!(
            drafts_summary(&[a_draft("a", Some(1)), a_draft("b", Some(2))]),
            "2 replies"
        );
    }

    #[test]
    fn the_outcome_says_what_did_not_post_and_nothing_when_all_did() {
        assert_eq!(submit_review_outcome(true, 3, &[], &[], &[], &[]), None);
        assert_eq!(submit_review_outcome(false, 1, &[], &[], &[], &[]), None);
        let kept = [
            a_draft("r1", Some(31)),
            a_draft("c1", None),
            a_draft("c2", None),
        ];
        assert_eq!(
            submit_review_outcome(
                true,
                2,
                &["c1".into()],
                &[("r1".into(), "GitHub returned 422: nope.".into())],
                &[],
                &kept,
            )
            .as_deref(),
            Some(
                "Posted the review and 2 comments. 1 reply did not post: GitHub returned 422: nope. \
                 1 comment could not be matched to what GitHub returned and is still a draft."
            )
        );
        assert_eq!(
            submit_review_outcome(true, 0, &["c1".into(), "c2".into()], &[], &[], &kept).as_deref(),
            Some(
                "Posted the review. 2 comments could not be matched to what GitHub returned and \
                 are still drafts."
            )
        );
        // No review went: a reply on its own that did not post, or one
        // that did beside one that did not.
        assert_eq!(
            submit_review_outcome(
                false,
                0,
                &[],
                &[("r1".into(), "offline".into())],
                &[],
                &kept
            )
            .as_deref(),
            Some("1 reply did not post: offline.")
        );
        assert_eq!(
            submit_review_outcome(
                false,
                1,
                &[],
                &[("r1".into(), "offline".into())],
                &[],
                &kept
            )
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
                &[],
                &kept,
            )
            .as_deref(),
            Some("Posted the review and 1 comment. 1 reply and 2 comments did not post: offline.")
        );
    }

    /// The drafts kept out of the review, on files the pull request does
    /// not change, are named by their lines, the first three, since each
    /// has to be moved or dropped; one the form no longer has is counted
    /// instead.
    #[test]
    fn the_outcome_names_the_drafts_kept_off_the_diff() {
        let on = |id: &str, start_line: u32, line: u32| Draft {
            anchor: Some(perch_github::Anchor {
                path: format!("src/{id}.rs"),
                start_line,
                line,
                side: perch_github::Side::Right,
                commit_sha: "h".into(),
            }),
            ..a_draft(id, None)
        };
        let kept = [
            on("a", 9, 9),
            on("b", 3, 5),
            on("c", 1, 1),
            on("d", 7, 7),
            a_draft("r1", Some(31)),
        ];
        assert_eq!(
            submit_review_outcome(true, 1, &[], &[], &["a".into()], &kept).as_deref(),
            Some(
                "Posted the review and 1 comment. GitHub takes comments only on files the pull \
                 request changes, so src/a.rs:9 is still a draft."
            )
        );
        assert_eq!(
            submit_review_outcome(true, 0, &[], &[], &["a".into(), "b".into()], &kept).as_deref(),
            Some(
                "Posted the review. GitHub takes comments only on files the pull request \
                 changes, so src/a.rs:9 and src/b.rs:3\u{2013}5 are still drafts."
            )
        );
        // No review went, and a reply under one of them could not follow it.
        let four = ["a".into(), "b".into(), "c".into(), "d".into()];
        let orphaned = [(
            "r1".into(),
            "the comment that opens its thread did not post".into(),
        )];
        assert_eq!(
            submit_review_outcome(false, 0, &[], &orphaned, &four, &kept).as_deref(),
            Some(
                "GitHub takes comments only on files the pull request changes, so \
                 src/a.rs:9, src/b.rs:3\u{2013}5, src/c.rs:1 and 1 more are still drafts. 1 reply \
                 did not post: the comment that opens its thread did not post."
            )
        );
        assert_eq!(
            submit_review_outcome(false, 0, &[], &[], &["gone".into()], &kept).as_deref(),
            Some(
                "GitHub takes comments only on files the pull request changes, so \
                 1 comment is still a draft."
            )
        );
    }
}
