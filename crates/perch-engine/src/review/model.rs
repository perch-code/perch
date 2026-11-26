//! The data the review's window and the engine share about a pull
//! request's files: a file as loaded, the places a language server
//! names, and what a review by agents is launched with and leaves.

use std::sync::Arc;

use perch_git::{ChangeStatus, DisplayBuffer, Row, Scope};
use perch_syntax::{Language, LineSpans};

use crate::review::picture::PictureSource;

/// Where a file is from: the worktree, which is what the review is of, or
/// outside it, a dependency or the standard library reached from a
/// definition, which is a place to read and no part of the review.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Origin {
    Worktree,
    Outside {
        /// What the tab's chip says: `tokio 1.40.0`, `std`, `git: floem`.
        label: String,
    },
}

/// Whether a path is a file outside the worktree: an absolute path, or a
/// class file jdtls names by its `jdt://` URI, which is no path on disk
/// and is read from jdtls. Everything else is relative to the worktree.
pub fn is_outside_path(path: &str) -> bool {
    std::path::Path::new(path).is_absolute() || path.starts_with("jdt://")
}

impl Origin {
    /// The origin a path says: a worktree file's path is relative to the
    /// worktree, and an outside file's is absolute, or a `jdt://` class
    /// file's. The two cannot collide, which is why `files` and the tabs
    /// are keyed by the path alone.
    pub fn of_path(path: &str) -> Origin {
        if is_outside_path(path) {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            Origin::Outside {
                label: outside_label(std::path::Path::new(path), home.as_deref()),
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
///
/// Over the wire, only the raw inputs (`path`, `head`, `base`, `status`,
/// `truncated`, `scope`, `blob`) travel as JSON; the client rebuilds the
/// diff buffers, alignment maps and syntax spans locally.
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
    /// The head side's verbatim text for changed files, so wire
    /// serialization can send raw text instead of derived buffers and spans.
    /// `None` for unchanged or truncated files, where `buffer.text` already
    /// holds it verbatim.
    pub(crate) head_text: Option<Arc<str>>,
    /// The base side's text, shared so an ask to the language server about
    /// a removed line does not copy the file.
    pub base_text: Option<Arc<str>>,
    /// Which base line sits beside which head line, both 1-based, for a
    /// split: `(Some, Some)` is an unchanged or modified pair, `(Some,
    /// None)` a removed line, `(None, Some)` an added one. Empty for a
    /// file outside the changed list.
    pub alignment: Vec<(Option<usize>, Option<usize>)>,
    /// Where each zero-based base line sits in `buffer`: its ghost row when
    /// removed or rewritten, or its paired unchanged head row.
    pub base_to_display: Vec<Option<usize>>,
    /// Which zero-based base line each display line of `buffer` is: a
    /// ghost's own, or an unchanged head row's aligned base line. `None`
    /// for an added or rewritten head line.
    pub display_to_base: Vec<Option<usize>>,
    /// The head blob, which the reveals are keyed by, as Viewed is: a new
    /// blob is new content and starts folded again. Empty for a file that
    /// has none: one outside the changed list, a deletion, or a working
    /// tree file git has not hashed. Such a file's reveals are kept for
    /// the window's life and never stored.
    pub blob: String,
    /// The file as it draws, when it is an SVG: its sides' markup, which
    /// the window parses and shows in place of the markup unless the
    /// reader asks for the markup. `None` for every other file.
    pub picture: Option<Arc<PictureSource>>,
}

fn str_is_empty(s: &&str) -> bool {
    s.is_empty()
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(serde::Serialize)]
struct WireLoadedFileRef<'a> {
    path: &'a str,
    head: &'a str,
    #[serde(skip_serializing_if = "str_is_empty")]
    base: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<ChangeStatus>,
    #[serde(skip_serializing_if = "is_false")]
    truncated: bool,
    scope: &'a Scope,
    #[serde(skip_serializing_if = "str_is_empty")]
    blob: &'a str,
}

#[derive(serde::Deserialize)]
struct WireLoadedFile {
    path: String,
    head: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    status: Option<ChangeStatus>,
    #[serde(default)]
    truncated: bool,
    scope: Scope,
    #[serde(default)]
    blob: String,
}

impl serde::Serialize for LoadedFile {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let head = if self.truncated {
            self.buffer.text.as_str()
        } else {
            self.head_text
                .as_deref()
                .unwrap_or(self.buffer.text.as_str())
        };
        let base = if self.truncated {
            ""
        } else {
            self.base_text.as_deref().unwrap_or("")
        };
        WireLoadedFileRef {
            path: &self.path,
            head,
            base,
            status: self.status,
            truncated: self.truncated,
            scope: &self.scope,
            blob: &self.blob,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for LoadedFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireLoadedFile::deserialize(deserializer)?;
        if wire.truncated {
            Ok(LoadedFile::truncated(
                wire.path,
                wire.head,
                wire.status,
                wire.scope,
                wire.blob,
            ))
        } else {
            Ok(crate::backend::build_loaded_file(
                wire.path,
                wire.head,
                wire.base,
                wire.status,
                false,
                wire.scope,
                wire.blob,
            ))
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outline {
    pub lines: u64,
    pub blocks: Vec<(usize, usize)>,
}

impl LoadedFile {
    /// A placeholder file for a binary, oversized or unreadable file:
    /// plain message text, no syntax highlighting, no base side.
    pub(crate) fn truncated(
        path: String,
        message: impl AsRef<str>,
        status: Option<ChangeStatus>,
        scope: Scope,
        blob: String,
    ) -> Self {
        let origin = Origin::of_path(&path);
        Self {
            path,
            origin,
            language: None,
            buffer: DisplayBuffer::plain(message.as_ref()),
            head_spans: LineSpans::default(),
            base_spans: LineSpans::default(),
            status,
            truncated: true,
            scope,
            base_buffer: None,
            head_text: None,
            base_text: None,
            alignment: Vec::new(),
            base_to_display: Vec::new(),
            display_to_base: Vec::new(),
            blob,
            picture: None,
        }
    }

    /// Build `base_to_display` and `display_to_base` from `buffer` and
    /// `alignment` once when the file loads, so every row lookup is O(1).
    pub fn base_maps(
        buffer: &DisplayBuffer,
        alignment: &[(Option<usize>, Option<usize>)],
    ) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
        let mut base_to_display: Vec<Option<usize>> = Vec::new();
        let mut display_to_base = vec![None; buffer.len()];
        let mut set_base = |base: usize, display: usize| {
            if base >= base_to_display.len() {
                base_to_display.resize(base + 1, None);
            }
            base_to_display[base] = Some(display);
        };
        for &(base, head) in alignment {
            let (Some(b), Some(h)) = (
                base.and_then(|n| n.checked_sub(1)),
                head.and_then(|n| n.checked_sub(1)),
            ) else {
                continue;
            };
            if let Some(display) = buffer.display_line(h) {
                set_base(b, display);
                if matches!(
                    buffer.row(display),
                    Some(Row::Head {
                        status: perch_git::LineStatus::Unchanged,
                        ..
                    })
                ) {
                    display_to_base[display] = Some(b);
                }
            }
        }
        // A rewritten line has both an aligned head row and a ghost row
        // above it; the base line itself lives on the ghost.
        for (display, &row) in buffer.rows.iter().enumerate() {
            if let Row::Deleted { base_line } = row {
                set_base(base_line, display);
                display_to_base[display] = Some(base_line);
            }
        }
        (base_to_display, display_to_base)
    }

    pub fn change_count(&self) -> usize {
        self.buffer.change_starts.len()
    }

    /// Where a zero-based base line sits in `buffer`.
    pub fn display_of_base(&self, base_line: usize) -> Option<usize> {
        self.base_to_display.get(base_line).copied().flatten()
    }

    /// Which zero-based base line a display line of `buffer` is, when it is
    /// a ghost or an unchanged head line.
    pub fn base_of_display(&self, display: usize) -> Option<usize> {
        self.display_to_base.get(display).copied().flatten()
    }

    /// The zero-based head line an unchanged base line also is.
    pub fn unchanged_head_of_base(&self, base_line: usize) -> Option<usize> {
        self.buffer.head_line(self.display_of_base(base_line)?)
    }

    /// The zero-based base line an unchanged head line also is.
    pub fn unchanged_base_of_head(&self, head_line: usize) -> Option<usize> {
        self.base_of_display(self.buffer.display_line(head_line)?)
    }

    /// The zero-based head line paired with `base_line` in `alignment`,
    /// unchanged or rewritten.
    pub fn paired_head_of_base(&self, base_line: usize) -> Option<usize> {
        let target = base_line + 1;
        self.alignment
            .iter()
            .find_map(|&(base, head)| (base == Some(target)).then_some(head).flatten())
            .and_then(|h| h.checked_sub(1))
    }

    /// The zero-based base line paired with `head_line` in `alignment`,
    /// unchanged or rewritten.
    pub fn paired_base_of_head(&self, head_line: usize) -> Option<usize> {
        let target = head_line + 1;
        self.alignment
            .iter()
            .find_map(|&(base, head)| (head == Some(target)).then_some(base).flatten())
            .and_then(|b| b.checked_sub(1))
    }
}

/// An identifier at a position in the head or base file: what the pointer
/// is over, and what the caret is on. `column` is where it starts on the
/// line and `end` where it ends, both byte columns as the protocol counts
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub path: String,
    /// 0-based line in the head file, or in the base file when `base` is true.
    pub line: usize,
    pub column: usize,
    pub end: usize,
    pub text: String,
    /// True when `line` is counted in the file's base side rather than its head.
    pub base: bool,
}

/// Every use in the file of the symbol the caret rests on, on each side of
/// the diff, as zero-based line, start column and end column. Empty is none.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Highlights {
    pub head: Vec<(usize, usize, usize)>,
    pub base: Vec<(usize, usize, usize)>,
}

impl Highlights {
    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.base.is_empty()
    }
}

/// One place rust-analyzer named: a use of a symbol, a definition, an
/// impl. `path` is relative to the worktree for a file in it and absolute
/// for one outside, in a dependency or the standard library, which then
/// carries the label its tab's chip shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// True when `line` is counted in the file's base side rather than its head.
    pub base: bool,
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Call {
    pub item: perch_lsp::lsp_types::CallHierarchyItem,
    /// Where the call is made: the file and line of the call, with the
    /// line's text. In the caller's file for a caller, and in the file of
    /// the function above for a callee.
    pub site: Located,
}

/// How a question runs: which agent, which of its models, and how hard
/// it thinks.
pub type AgentChoice = (perch_agent::Backend, String, perch_agent::Effort);

/// One effective reviewer captured by a launch. A disabled (`None`) row is
/// omitted from `ReviewLaunchRequest::reviewers`; it is a UI preference, not
/// an effort understood by the native CLI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewLaunchChoice {
    pub backend: perch_agent::Backend,
    pub model: String,
    pub effort: perch_agent::ReviewEffort,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewPreferenceChoice {
    pub backend: perch_agent::Backend,
    pub model: Option<String>,
    pub effort: Option<perch_agent::ReviewEffort>,
    pub enabled: bool,
}

/// Everything needed to draw one immutable review in Conversation/history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

impl ReviewFindingSummary {
    /// Resolved and explicitly linked findings stay in review history, but
    /// do not crowd the default list of actionable threads.
    pub fn is_actionable(&self) -> bool {
        self.finding.disposition == perch_github::FindingDisposition::Open
            && self.finding.verification != perch_github::FindingVerification::Fixed
            && self.finding.duplicate_of.is_none()
    }
}

/// The diff a new thread's lines were picked in, when it is not Branch's:
/// the scope its file was loaded under, the base side's path in it, which
/// a rename makes another, and the text of the side they are on as it was
/// shown. GitHub reads a comment's lines against the merge base and the
/// head, so a draft for it from here is moved to those first.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Picked {
    pub scope: Scope,
    pub base_path: String,
    /// `None` when the file was not loaded, or not as text; the side is
    /// read again then. The working tree and the index can move on after
    /// the lines were picked, so they are mapped from what was on screen.
    pub source: Option<Arc<str>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_file_from_jdtls_is_outside_the_worktree() {
        let path = "jdt://contents/java.base/java.util/List.class";
        assert!(is_outside_path(path));
        assert_eq!(
            Origin::of_path(path),
            Origin::Outside {
                label: "JDK".to_string()
            }
        );
        assert!(is_outside_path("/opt/vendor/src/lib.rs"));
        assert!(!is_outside_path("src/lib.rs"));
        assert_eq!(Origin::of_path("src/lib.rs"), Origin::Worktree);
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
}
