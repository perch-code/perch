//! Code named in what people write: a file, a line of one, or a symbol,
//! in a code span, a fenced block or a link. The markdown links what it
//! finds here to Files.
//!
//! A file is looked up in the worktree's listing, so a path links only
//! when the file is there. A path written short, from a directory down or
//! by the file's name alone, links when one file ends with it, or when
//! one changed file does and others do too. A line after the path, `:17`
//! or GitHub's `#L17`, is where the file opens; the head's line, since an
//! agent reads the worktree.
//!
//! A symbol is looked up by name through rust-analyzer's workspace symbol
//! search, since only the server knows where one is defined. A name with
//! a qualifier, `Db::open` or `WalGcMode::Regular`, is looked for inside
//! what qualifies it; one the server does not index, a struct's field say,
//! links what qualifies it instead. A name defined in more places than
//! `MAX_DEFINITIONS` is too common to mean one of them and links nothing.
//! The answers are kept on the review state for the item's life, so a
//! name is asked about once however often it is drawn, and the text that
//! names it is drawn again, linked, when its answer lands.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalWith};
use floem::views::dyn_container;
use floem::AnyView;
use perch_git::{ChangedFile, TreeEntry};
use perch_lsp::client::Defined;
use perch_lsp::lsp_types::{Location, SymbolKind};

use crate::review::linked_text::Target;
use crate::review::model::{Located, ReviewState};

/// The most definitions a name may have and still be linked to them.
pub const MAX_DEFINITIONS: usize = 10;

// ---------------------------------------------------------------------
// What is written
// ---------------------------------------------------------------------

/// A file someone named, as they wrote it, with the line and column after
/// it when there are any, counted from one as an editor counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPlace {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// `text` as a file and a place in it: `src/db.rs`, `src/db.rs:17`,
/// `src/db.rs:17:5`, `src/db.rs:17-20`, or GitHub's `src/db.rs#L17` and
/// `#L17-L20`. A range opens at its start. `None` for anything a path
/// would not be: a sentence, a URL, a type with its generics.
pub fn written_place(text: &str) -> Option<WrittenPlace> {
    let (path, line, column) = match text.split_once("#L") {
        Some((path, anchor)) => {
            let (line, column) = github_lines(anchor)?;
            (path, Some(line), column)
        }
        None => match text.split_once(':') {
            Some((path, rest)) => {
                let (line, column) = colon_lines(rest)?;
                (path, Some(line), column)
            }
            None => (text, None, None),
        },
    };
    is_path(path).then(|| WrittenPlace {
        path: path.to_string(),
        line,
        column,
    })
}

/// Whether `text` reads as a path: nothing a path would not hold, and a
/// name at the end of it.
fn is_path(text: &str) -> bool {
    let name = text.rsplit('/').next().unwrap_or(text);
    text.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'+' | b'@' | b'~')
    }) && name.bytes().any(|b| b.is_ascii_alphanumeric())
}

/// A line number, from one.
fn number(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|&n| n > 0)
}

/// `17`, `17:5`, `17-20` or `17:5-20:3` after a path's colon, and a colon
/// after that, as grep and the compiler leave one.
fn colon_lines(rest: &str) -> Option<(usize, Option<usize>)> {
    let rest = rest.strip_suffix(':').unwrap_or(rest);
    let (start, end) = match rest.split_once('-') {
        Some((start, end)) => (start, Some(end)),
        None => (rest, None),
    };
    if let Some(end) = end {
        if !end.split(':').all(|n| number(n).is_some()) {
            return None;
        }
    }
    match start.split_once(':') {
        Some((line, column)) => Some((number(line)?, Some(number(column)?))),
        None => Some((number(start)?, None)),
    }
}

/// `17`, `17-L20`, `17-20` or `17C5-L20C3` after GitHub's `#L`.
fn github_lines(anchor: &str) -> Option<(usize, Option<usize>)> {
    let one = |text: &str| -> Option<(usize, Option<usize>)> {
        match text.split_once('C') {
            Some((line, column)) => Some((number(line)?, Some(number(column)?))),
            None => Some((number(text)?, None)),
        }
    };
    match anchor.split_once('-') {
        Some((start, end)) => {
            one(end.strip_prefix('L').unwrap_or(end))?;
            one(start)
        }
        None => one(anchor),
    }
}

/// A symbol someone named, as they wrote it: `Db`, `Db::open`,
/// `DbOptions.ttl`, `flush()`, with the byte range of each identifier in
/// `text`, qualifiers first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WrittenName {
    pub text: String,
    pub segments: Vec<Range<usize>>,
}

impl WrittenName {
    /// The identifiers, qualifiers first.
    pub fn names(&self) -> Vec<String> {
        self.segments
            .iter()
            .map(|r| self.text[r.clone()].to_string())
            .collect()
    }
}

/// `text` as a symbol's name, alone or after what it belongs to, with
/// `::` or `.` between, and `()` or `!` after for a call or a macro. A
/// leading `crate`, `self`, `super` or `Self` is dropped, since it says
/// nothing the search can use. `None` for anything else, and for a name
/// no workspace could be meant by: a keyword, a primitive, the prelude's
/// own, a single letter, or a file's extension after a dot. A lone word
/// in lower case, `inner`, is `None` too unless it is called, `inner()`:
/// it is as often a field or a variable, which the search does not know,
/// as the function it would find.
pub fn written_name(text: &str) -> Option<WrittenName> {
    let called = text.ends_with("()") || text.ends_with('!');
    let body = text
        .strip_suffix("()")
        .or_else(|| text.strip_suffix('!'))
        .unwrap_or(text);
    let mut segments: Vec<Range<usize>> = Vec::new();
    let mut dotted = false;
    let mut at = 0;
    loop {
        let rest = &body[at..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if len == 0 || rest.as_bytes()[0].is_ascii_digit() {
            return None;
        }
        segments.push(at..at + len);
        at += len;
        if at == body.len() {
            break;
        }
        at += if body[at..].starts_with("::") {
            2
        } else if body[at..].starts_with('.') {
            dotted = true;
            1
        } else {
            return None;
        };
    }
    let leading = segments
        .iter()
        .take_while(|r| matches!(&body[(*r).clone()], "crate" | "self" | "super" | "Self"))
        .count();
    segments.drain(..leading);
    let last = &body[segments.last()?.clone()];
    if last.len() < 2 || is_reserved(last) || (dotted && is_extension(last)) {
        return None;
    }
    let word = last
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if segments.len() == 1 && word && !called {
        return None;
    }
    Some(WrittenName {
        text: text.to_string(),
        segments,
    })
}

/// A keyword, a primitive, or a name the prelude gives every crate:
/// nothing a workspace's search would find the one meant by.
fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "bool"
            | "char"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "Option"
            | "Some"
            | "None"
            | "Result"
            | "Ok"
            | "Err"
            | "Vec"
            | "String"
            | "Box"
            | "Clone"
            | "Copy"
            | "Send"
            | "Sync"
            | "Sized"
            | "Default"
            | "Debug"
            | "Drop"
            | "Fn"
            | "FnMut"
            | "FnOnce"
            | "Iterator"
            | "Into"
            | "From"
            | "ToString"
            | "PartialEq"
            | "Eq"
            | "PartialOrd"
            | "Ord"
            | "Hash"
    )
}

/// A file's extension, which after a dot makes the text a file's name.
fn is_extension(name: &str) -> bool {
    matches!(
        name,
        "rs" | "md"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "txt"
            | "lock"
            | "py"
            | "go"
            | "js"
            | "ts"
            | "tsx"
            | "sh"
            | "html"
            | "css"
            | "proto"
            | "sql"
            | "fbs"
            | "sst"
            | "log"
    )
}

/// Whether a name reads as a type's, a trait's or a variant's: a capital,
/// then a small letter somewhere, which `WAL` and `T` do not have.
fn is_type_like(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.bytes().any(|b| b.is_ascii_lowercase())
}

/// Something a run of code may name, at its byte range in the run: a
/// file when the text reads as a path, a symbol when it reads as a name,
/// and both for a bare word such as `Makefile` or `Db`, where the file is
/// looked up first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub range: Range<usize>,
    pub place: Option<WrittenPlace>,
    pub name: Option<WrittenName>,
}

/// What a code span names: the whole span when it is a path or a name,
/// else what `scan` finds in it, as in `Option<WalGcPolicy>`.
pub fn in_code_span(text: &str) -> Vec<Mention> {
    let place = written_place(text);
    let name = written_name(text);
    if place.is_some() || name.is_some() {
        return vec![Mention {
            range: 0..text.len(),
            place,
            name,
        }];
    }
    scan(text, true)
}

/// Whether a fenced block's language is one whose type names may be
/// looked up: Rust, a diff or suggestion of it, output, or none named.
/// The paths in any block are.
pub fn names_in_fence(lang: Option<&str>) -> bool {
    lang.is_none_or(|lang| {
        matches!(
            lang.to_ascii_lowercase().as_str(),
            "rust"
                | "rs"
                | "diff"
                | "patch"
                | "suggestion"
                | "text"
                | "txt"
                | "plain"
                | "plaintext"
                | "console"
                | "log"
                | "output"
        )
    })
}

/// What a run of code names: every word in it that reads as a path with a
/// directory or an extension, and, with `symbols`, every name that reads
/// as a type's, alone or with what follows it after `::`, outside a `//`
/// comment. A word is what lies between spaces, brackets, quotes and the
/// like, with the punctuation that ends a clause taken off its end.
pub fn scan(text: &str, symbols: bool) -> Vec<Mention> {
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let comment = line.find("//").unwrap_or(line.len());
        let mut at = 0;
        while at < line.len() {
            let rest = &line[at..];
            let len = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
            if len == 0 {
                at += rest.chars().next().map_or(1, char::len_utf8);
                continue;
            }
            let word = trim_word(&line[at..at + len]);
            if !word.1.is_empty() {
                let start = at + word.0;
                let range = line_start + start..line_start + start + word.1.len();
                if let Some(mention) = mention_in_code(word.1, range, symbols && start < comment) {
                    out.push(mention);
                }
            }
            at += len;
        }
        line_start += line.len();
    }
    out
}

/// What a word of code names, if anything, at `range`.
fn mention_in_code(word: &str, range: Range<usize>, symbols: bool) -> Option<Mention> {
    if word.contains(['/', '.']) {
        let place = written_place(word)?;
        return Some(Mention {
            range,
            place: Some(place),
            name: None,
        });
    }
    if !symbols || word.contains('#') {
        return None;
    }
    let name = written_name(word)?;
    let names = name.names();
    let typed = names.iter().rev().take(2).any(|n| is_type_like(n));
    typed.then_some(Mention {
        range,
        place: None,
        name: Some(name),
    })
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | ':' | '#' | '-' | '+' | '@' | '~')
}

/// A word without the punctuation that is prose around it rather than
/// part of it: a colon, comma or full stop after, a diff's `+` or `-` or
/// a `::` before. Where the word now starts in the old, and the word.
fn trim_word(word: &str) -> (usize, &str) {
    let start = word.len() - word.trim_start_matches([':', '-', '+', '#', '@']).len();
    let word = word[start..].trim_end_matches(['.', ',', ':', ';', '-', '#']);
    (start, word)
}

// ---------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------

/// The worktree's files, by path and by name, for looking up a path
/// someone wrote.
#[derive(Debug, Default)]
pub struct PathIndex {
    files: HashSet<String>,
    by_name: HashMap<String, Vec<String>>,
    changed: HashSet<String>,
    /// The worktree's own path and a slash, for a path written in full.
    root: Option<String>,
}

impl PathIndex {
    /// The files of the listing and the changed files, which hold a
    /// deleted one the listing does not.
    pub fn new(tree: &[TreeEntry], changed: &[ChangedFile], root: Option<&Path>) -> Self {
        let mut index = PathIndex {
            root: root.map(|r| format!("{}/", r.to_string_lossy().trim_end_matches('/'))),
            ..PathIndex::default()
        };
        let paths = tree
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.as_str())
            .chain(changed.iter().map(|f| f.path.as_str()));
        for path in paths {
            if index.files.insert(path.to_string()) {
                let name = path.rsplit('/').next().unwrap_or(path);
                index
                    .by_name
                    .entry(name.to_string())
                    .or_default()
                    .push(path.to_string());
            }
        }
        index.changed = changed.iter().map(|f| f.path.clone()).collect();
        index
    }

    /// The file `written` names, relative to the worktree: the one at that
    /// path, from the worktree's root or its own; else the one file whose
    /// path ends with it; else, when several do, the one of those that is
    /// changed. `None` for a file outside the worktree, which is nothing a
    /// comment should be able to open.
    pub fn resolve(&self, written: &str) -> Option<String> {
        let mut path = written;
        if let Some(root) = &self.root {
            if let Some(inside) = path.strip_prefix(root.as_str()) {
                path = inside;
            }
        }
        if path.starts_with('/') || path.starts_with('~') {
            return None;
        }
        while let Some(rest) = path.strip_prefix("./") {
            path = rest;
        }
        if self.files.contains(path) {
            return Some(path.to_string());
        }
        // git's own prefixes, as a diff's header writes them.
        for prefix in ["a/", "b/"] {
            if let Some(rest) = path.strip_prefix(prefix) {
                if self.files.contains(rest) {
                    return Some(rest.to_string());
                }
            }
        }
        if path.split('/').any(|part| part.is_empty() || part == "..") {
            return None;
        }
        let name = path.rsplit('/').next()?;
        let ends: Vec<&String> = self
            .by_name
            .get(name)?
            .iter()
            .filter(|f| {
                f.len() > path.len()
                    && f.ends_with(path)
                    && f.as_bytes()[f.len() - path.len() - 1] == b'/'
            })
            .collect();
        match ends.as_slice() {
            [one] => Some(one.to_string()),
            several => {
                let changed: Vec<&&String> = several
                    .iter()
                    .filter(|f| self.changed.contains(f.as_str()))
                    .collect();
                match changed.as_slice() {
                    [one] => Some(one.to_string()),
                    _ => None,
                }
            }
        }
    }
}

thread_local! {
    /// The index of the listing last looked in, with what it was built
    /// from. The listing's `Arc`s are held, so a new listing is never
    /// taken for the old one at the same address.
    #[allow(clippy::type_complexity)]
    static INDEX: RefCell<Option<(Arc<Vec<TreeEntry>>, Arc<Vec<ChangedFile>>, Option<PathBuf>, Rc<PathIndex>)>> =
        const { RefCell::new(None) };
}

/// The index of a review's files, built once per listing.
fn index_of(state: ReviewState) -> Rc<PathIndex> {
    let tree = state.tree.get_untracked();
    let changed = state.changed_files.get_untracked();
    let root = state.item.with_untracked(|i| {
        i.as_ref()
            .and_then(|i| i.checkout.as_ref().map(|c| c.path.clone()))
    });
    INDEX.with(|cell| {
        let mut cell = cell.borrow_mut();
        if let Some((t, c, r, index)) = cell.as_ref() {
            if Arc::ptr_eq(t, &tree) && Arc::ptr_eq(c, &changed) && *r == root {
                return index.clone();
            }
        }
        let index = Rc::new(PathIndex::new(&tree, &changed, root.as_deref()));
        *cell = Some((tree, changed, root, index.clone()));
        index
    })
}

// ---------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------

/// A name to ask rust-analyzer about: the name as written, which its
/// answer is kept under, and its identifiers, qualifiers first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameAsk {
    pub key: String,
    pub segments: Vec<String>,
}

impl NameAsk {
    fn of(name: &WrittenName) -> Self {
        NameAsk {
            key: name.text.clone(),
            segments: name.names(),
        }
    }
}

/// What a name's answer found: the definitions of its first `segments`
/// identifiers, which is all of them when the whole name resolved.
pub type Found = (usize, Arc<Vec<Located>>);

/// What the workspace search made of a name, kept under the name as
/// written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    /// Asked, under the ask's token, and not yet answered.
    Asked(u64),
    Found(Found),
    /// Nothing in the workspace, or too many things to mean one.
    Missing,
}

impl Named {
    pub fn found(&self) -> Option<Found> {
        match self {
            Named::Found(found) => Some(found.clone()),
            Named::Asked(_) | Named::Missing => None,
        }
    }
}

/// The definitions a name means, from the symbols the search found for
/// each of its identifiers, and how many of its identifiers they are the
/// definitions of: the whole name's when it resolves, else the longest
/// qualifier's that does and reads as a type, as `DbOptions` for
/// `DbOptions.ttl`, whose field the search does not index. `root` is the
/// worktree, for reading a module in a qualifier off a definition's path.
///
/// An identifier's definitions are those of its exact name inside what
/// qualifies it: the type or trait the search says holds it, or, for a
/// qualifier in lower case, a module or crate on the definition's path.
/// Types are kept over anything else of the name, so `Db` is the struct
/// and not a variant called `Db`; code written by hand over generated
/// code of the same name; and more than `MAX_DEFINITIONS` is too many to
/// mean one.
pub fn pick(
    segments: &[String],
    found: &HashMap<String, Vec<Defined>>,
    root: &Path,
) -> Option<(usize, Vec<Location>)> {
    (1..=segments.len()).rev().find_map(|k| {
        let name = &segments[k - 1];
        if k < segments.len() && !is_type_like(name) {
            return None;
        }
        let qualifier = k.checked_sub(2).map(|q| segments[q].as_str());
        let exact: Vec<&Defined> = found
            .get(name)?
            .iter()
            .filter(|d| d.name == *name)
            .collect();
        let mut chosen: Vec<&Defined> = match qualifier {
            None => exact,
            Some(qualifier) => {
                let held: Vec<&Defined> = exact
                    .iter()
                    .copied()
                    .filter(|d| d.container.as_deref().map(bare_type) == Some(qualifier))
                    .collect();
                if !held.is_empty() || qualifier.starts_with(|c: char| c.is_ascii_uppercase()) {
                    held
                } else {
                    exact
                        .into_iter()
                        .filter(|d| in_module(&d.location, qualifier, root))
                        .collect()
                }
            }
        };
        if chosen.iter().any(|d| is_type_kind(d.kind)) {
            chosen.retain(|d| is_type_kind(d.kind));
        }
        if chosen.iter().any(|d| !is_generated(&d.location)) {
            chosen.retain(|d| !is_generated(&d.location));
        }
        let mut seen = HashSet::new();
        let locations: Vec<Location> = chosen
            .into_iter()
            .filter(|d| seen.insert((d.location.uri.to_string(), d.location.range.start.line)))
            .map(|d| d.location.clone())
            .collect();
        (!locations.is_empty() && locations.len() <= MAX_DEFINITIONS).then_some((k, locations))
    })
}

/// A container as the type's bare name: `PeekingIterator<T>` as
/// `PeekingIterator`, `&'a Db` as `Db`.
fn bare_type(container: &str) -> &str {
    let name = container.split('<').next().unwrap_or(container).trim();
    let name = name.rsplit([' ', '&', '*']).next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name)
}

/// Whether a definition is in the module or crate `module`: a directory
/// or a file of that name on its path within the worktree, a crate's
/// dashes read as underscores.
fn in_module(location: &Location, module: &str, root: &Path) -> bool {
    let Some(path) = perch_lsp::client::uri_to_path(location.uri.as_str()) else {
        return false;
    };
    let path = path.strip_prefix(root).unwrap_or(&path);
    path.iter().any(|part| {
        let part = part.to_string_lossy();
        let stem = part.strip_suffix(".rs").unwrap_or(&part);
        stem.replace('-', "_") == module
    })
}

/// Whether a definition is in generated code, as flatbuffers and bindgen
/// leave it: under a `generated` directory or in a `_generated` file.
fn is_generated(location: &Location) -> bool {
    location
        .uri
        .as_str()
        .split('/')
        .any(|part| part == "generated" || part.contains("_generated."))
}

fn is_type_kind(kind: SymbolKind) -> bool {
    kind == SymbolKind::STRUCT
        || kind == SymbolKind::ENUM
        || kind == SymbolKind::INTERFACE
        || kind == SymbolKind::CLASS
        || kind == SymbolKind::TYPE_PARAMETER
}

// ---------------------------------------------------------------------
// Linking
// ---------------------------------------------------------------------

/// The review a piece of markdown is drawn in, where what it names is
/// looked up and opened. Only for an item with a checkout: without one
/// there is no Files to open.
#[derive(Clone, Copy)]
pub struct Refs {
    state: ReviewState,
}

/// A file a link opens, relative to the worktree, and the line and column
/// it opens at, counted from one as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLink {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Refs {
    pub fn of(state: ReviewState) -> Option<Refs> {
        let has_files = state
            .item
            .with_untracked(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()));
        has_files.then_some(Refs { state })
    }

    /// The file a written place names, when the worktree has it.
    pub fn file(&self, place: &WrittenPlace) -> Option<FileLink> {
        let path = index_of(self.state).resolve(&place.path)?;
        Some(FileLink {
            path,
            line: place.line,
            column: place.column,
        })
    }

    /// The file a link's destination names, when it is a path into the
    /// worktree rather than an address: `src/db.rs#L17`, or the absolute
    /// path an agent writes, with `:17` after it or not. An anchor that is
    /// not a line, `README.md#usage`, is dropped.
    pub fn file_for_url(&self, url: &str) -> Option<FileLink> {
        let url = url.strip_prefix("file://").unwrap_or(url);
        if url.contains("://") || url.starts_with("mailto:") {
            return None;
        }
        let place = match url.split_once('#') {
            Some((path, _)) if !path.is_empty() => {
                written_place(url).or_else(|| written_place(path))?
            }
            _ => written_place(url)?,
        };
        self.file(&place)
    }

    /// A link to a file: its tip says where it goes, and a click goes.
    pub fn file_target(&self, link: FileLink) -> Target {
        let tip = match link.line {
            Some(line) => format!("{}:{line}", link.path),
            None => link.path.clone(),
        };
        let state = self.state;
        Target::Local {
            tip,
            open: Rc::new(move || {
                let at = link
                    .line
                    .map(|line| (line - 1, link.column.map_or(0, |c| c - 1)));
                state.go_to_file(&link.path, at);
            }),
        }
    }

    /// A link to a name's definitions: one opens, several open the panel.
    pub fn name_target(&self, name: String, locations: Arc<Vec<Located>>) -> Target {
        let tip = match locations.as_slice() {
            [one] => format!("{}:{}", one.path, one.line),
            several => format!("{} definitions", several.len()),
        };
        let state = self.state;
        Target::Local {
            tip,
            open: Rc::new(move || state.go_to_definitions(&name, &locations)),
        }
    }

    /// What `keys` have found, tracked: none while the server is not
    /// there to have been asked, so the links go when it does.
    fn answers(&self, keys: &[String]) -> Vec<Option<Found>> {
        let usable = self.state.lsp_status.with(|s| s.is_usable());
        self.state.names.with(|names| {
            keys.iter()
                .map(|key| names.get(key).filter(|_| usable).and_then(|n| n.found()))
                .collect()
        })
    }
}

/// What a run of text links: the links known as it is drawn, and the
/// names whose links wait on an answer, each at its byte offset in the
/// run.
#[derive(Clone, Default)]
pub struct Plan {
    pub links: Vec<(Range<usize>, Target)>,
    pub names: Vec<(usize, WrittenName)>,
}

impl Plan {
    /// Add what a run of code at `offset` names: each file at once, each
    /// name to ask about unless it is a file.
    pub fn add_mentions(&mut self, refs: Refs, offset: usize, mentions: Vec<Mention>) {
        for mention in mentions {
            let range = offset + mention.range.start..offset + mention.range.end;
            match mention.place.as_ref().and_then(|p| refs.file(p)) {
                Some(link) => self.links.push((range, refs.file_target(link))),
                None => {
                    if let Some(name) = mention.name {
                        self.names.push((range.start, name));
                    }
                }
            }
        }
    }

    /// Every link, those of the names that have found something among
    /// them, in order along the run. A name found whole links whole, its
    /// `()` and all; one found only by its qualifier links as far as that.
    fn with_answers(&self, refs: Refs, answers: &[Option<Found>]) -> Vec<(Range<usize>, Target)> {
        let mut links = self.links.clone();
        for ((offset, name), answer) in self.names.iter().zip(answers) {
            let Some((segments, locations)) = answer else {
                continue;
            };
            let Some(last) = name.segments.get(segments.saturating_sub(1)) else {
                continue;
            };
            let named = name.text[..last.end].to_string();
            let end = if *segments == name.segments.len() {
                name.text.len()
            } else {
                last.end
            };
            links.push((
                *offset..offset + end,
                refs.name_target(named, locations.clone()),
            ));
        }
        links.sort_by_key(|(range, _)| range.start);
        links
    }
}

/// A run drawn by `draw` with the plan's links, and drawn again with the
/// names' links as their answers land. The names are asked about while
/// the server is there to answer, and again when it comes back.
pub fn linked_run(
    plan: Plan,
    refs: Option<Refs>,
    draw: impl Fn(Vec<(Range<usize>, Target)>) -> AnyView + 'static,
) -> AnyView {
    let Some(refs) = refs.filter(|_| !plan.names.is_empty()) else {
        return draw(plan.links);
    };
    let asks: Vec<NameAsk> = plan
        .names
        .iter()
        .map(|(_, name)| NameAsk::of(name))
        .collect();
    let keys: Vec<String> = asks.iter().map(|a| a.key.clone()).collect();
    let state = refs.state;
    Effect::new(move |_| {
        if state.lsp_status.with(|s| s.is_usable()) {
            state.ask_names(&asks);
        }
    });
    let answers = Memo::new(move |_| refs.answers(&keys));
    dyn_container(
        move || answers.get(),
        move |answers| draw(plan.with_answers(refs, &answers)),
    )
    .into_any()
}

/// A review of a checkout at `/tmp/wt` whose listing is `files`, for
/// looking code up in.
#[cfg(test)]
pub(crate) fn review_of(files: &[&str]) -> ReviewState {
    let state = ReviewState::new();
    let mut item =
        perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
    item.id = "wt".into();
    item.checkout = Some(perch_github::ItemCheckout {
        branch: "perch/pr-1".into(),
        base_ref: "main".into(),
        head_sha: "h".into(),
        path: PathBuf::from("/tmp/wt"),
        owned: true,
        on_disk: true,
        churn: None,
        last_seen_head: None,
        source: None,
    });
    state.item.set(Some(Arc::new(item)));
    state.tree.set(Arc::new(
        files.iter().map(|f| TreeEntry::new(*f, false)).collect(),
    ));
    state
}

/// A definition's location, as the backend reads one.
#[cfg(test)]
pub(crate) fn located(path: &str, line: usize, character: usize) -> Located {
    Located {
        path: path.into(),
        outside: None,
        line,
        character,
        text: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_lsp::lsp_types::{Position, Range as LspRange, Uri};

    fn place(path: &str, line: Option<usize>, column: Option<usize>) -> Option<WrittenPlace> {
        Some(WrittenPlace {
            path: path.into(),
            line,
            column,
        })
    }

    #[test]
    fn a_path_is_read_with_the_line_and_column_after_it() {
        assert_eq!(
            written_place("slatedb/src/gc.rs"),
            place("slatedb/src/gc.rs", None, None)
        );
        assert_eq!(written_place("gc.rs:17"), place("gc.rs", Some(17), None));
        assert_eq!(
            written_place("src/gc.rs:17:5"),
            place("src/gc.rs", Some(17), Some(5))
        );
        assert_eq!(
            written_place("src/gc.rs:17-20"),
            place("src/gc.rs", Some(17), None)
        );
        assert_eq!(
            written_place("src/gc.rs:17:"),
            place("src/gc.rs", Some(17), None),
            "grep's"
        );
        assert_eq!(
            written_place("src/gc.rs#L17"),
            place("src/gc.rs", Some(17), None)
        );
        assert_eq!(
            written_place("src/gc.rs#L17-L20"),
            place("src/gc.rs", Some(17), None)
        );
        assert_eq!(
            written_place("src/gc.rs#L17C5-L20C3"),
            place("src/gc.rs", Some(17), Some(5))
        );
        assert_eq!(written_place("./Makefile"), place("./Makefile", None, None));
        for not in [
            "https://example.com/x",
            "seqnum = 10",
            "Option<Result<X>>",
            "src/gc.rs:0",
            "src/gc.rs:x",
            "src/gc.rs:17-x",
            "src/",
            "[0, 20)",
            "",
        ] {
            assert_eq!(written_place(not), None, "{not:?}");
        }
    }

    fn segments(text: &str) -> Option<Vec<String>> {
        written_name(text).map(|n| n.names())
    }

    #[test]
    fn a_name_is_read_with_what_qualifies_it() {
        assert_eq!(segments("ObjectName"), Some(vec!["ObjectName".into()]));
        assert_eq!(segments("Db::open"), Some(vec!["Db".into(), "open".into()]));
        assert_eq!(
            segments("DbOptions.sst_auto_recovery"),
            Some(vec!["DbOptions".into(), "sst_auto_recovery".into()])
        );
        assert_eq!(
            segments("block_capacity()"),
            Some(vec!["block_capacity".into()])
        );
        assert_eq!(segments("define_codec!"), Some(vec!["define_codec".into()]));
        assert_eq!(
            segments("crate::db::Db"),
            Some(vec!["db".into(), "Db".into()])
        );
        assert_eq!(segments("self.flush()"), Some(vec!["flush".into()]));
        assert_eq!(
            segments("list_wal_ssts"),
            Some(vec!["list_wal_ssts".into()])
        );
        // A lone word in lower case is as likely a field as a function,
        // unless it is called.
        assert_eq!(segments("inner()"), Some(vec!["inner".into()]));
        for not in [
            "inner",
            "self.flush",
            "Some",
            "None",
            "true",
            "self",
            "u64",
            "T",
            "x",
            "wal_gc.rs",
            "a b",
            "Vec<u8>",
            "collect(ranges)",
            "Db::",
            "::Db",
            "1st",
            "a..b",
        ] {
            assert_eq!(segments(not), None, "{not:?}");
        }
        let name = written_name("Db::open()").unwrap();
        assert_eq!(
            name.segments,
            vec![0..2, 4..8],
            "ranges in the text as written"
        );
    }

    fn mentions(text: &str, symbols: bool) -> Vec<(&str, bool, bool)> {
        scan(text, symbols)
            .into_iter()
            .map(|m| (&text[m.range.clone()], m.place.is_some(), m.name.is_some()))
            .collect()
    }

    #[test]
    fn a_block_names_its_paths_and_its_types() {
        let code = "error[E0425]: cannot find `x`\n  --> crates/app/src/main.rs:17:5,\n\
                    let gc = WalGcMode::Regular; // Regular is a mode\n\
                    let t: Arc<SlateDbWalGc> = self.table_store.list(x);\n";
        assert_eq!(
            mentions(code, true),
            vec![
                ("crates/app/src/main.rs:17:5", true, false),
                ("WalGcMode::Regular", false, true),
                ("Arc", false, true),
                ("SlateDbWalGc", false, true),
                ("self.table_store.list", true, false),
            ]
        );
        // A block in another language has its paths linked and no names.
        assert_eq!(
            mentions("from Foo import Bar  # see a/b.py:3", false),
            vec![("a/b.py:3", true, false)]
        );
        assert!(names_in_fence(None));
        assert!(names_in_fence(Some("Rust")));
        assert!(!names_in_fence(Some("python")));
    }

    #[test]
    fn a_code_span_is_one_mention_when_it_is_a_path_or_a_name() {
        let whole = in_code_span("wal/mod.rs:303");
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].place, place("wal/mod.rs", Some(303), None));
        assert_eq!(whole[0].name, None);
        // A bare word could be either, and is looked up as a file first.
        let word = in_code_span("Db");
        assert_eq!(
            (word[0].place.is_some(), word[0].name.is_some()),
            (true, true)
        );
        // Anything else is scanned, as a block is.
        let typed = in_code_span("regular: Option<WalGcPolicy>");
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].range, 16..27);
    }

    fn entry(path: &str) -> TreeEntry {
        TreeEntry::new(path, false)
    }

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: perch_git::ChangeStatus::Modified,
            additions: 1,
            deletions: 0,
            binary: false,
            head_blob: None,
        }
    }

    #[test]
    fn a_path_is_found_whole_short_or_among_the_changes() {
        let tree = vec![
            TreeEntry::new("slatedb", true),
            entry("slatedb/src/wal/mod.rs"),
            entry("slatedb/src/wal/slatedb/gc.rs"),
            entry("slatedb/src/garbage_collector/gc.rs"),
            entry("slatedb/src/db.rs"),
            entry("Makefile"),
        ];
        let index = PathIndex::new(
            &tree,
            &[
                changed("slatedb/src/wal/slatedb/gc.rs"),
                changed("slatedb/src/gone.rs"),
            ],
            Some(Path::new("/w/pr-1")),
        );
        let found = |p: &str| index.resolve(p);
        assert_eq!(
            found("slatedb/src/db.rs").as_deref(),
            Some("slatedb/src/db.rs")
        );
        assert_eq!(
            found("./slatedb/src/db.rs").as_deref(),
            Some("slatedb/src/db.rs")
        );
        assert_eq!(
            found("b/slatedb/src/db.rs").as_deref(),
            Some("slatedb/src/db.rs")
        );
        assert_eq!(
            found("/w/pr-1/slatedb/src/db.rs").as_deref(),
            Some("slatedb/src/db.rs")
        );
        assert_eq!(
            found("wal/mod.rs").as_deref(),
            Some("slatedb/src/wal/mod.rs"),
            "short"
        );
        assert_eq!(
            found("db.rs").as_deref(),
            Some("slatedb/src/db.rs"),
            "by name"
        );
        assert_eq!(found("Makefile").as_deref(), Some("Makefile"));
        // Two gc.rs, one of them changed: the changed one.
        assert_eq!(
            found("gc.rs").as_deref(),
            Some("slatedb/src/wal/slatedb/gc.rs")
        );
        // A deleted file is only among the changes.
        assert_eq!(found("gone.rs").as_deref(), Some("slatedb/src/gone.rs"));
        for not in [
            "b.rs",
            "src",
            "slatedb",
            "/etc/passwd",
            "/w/pr-10/slatedb/src/db.rs",
            "~/x.rs",
            "../db.rs",
        ] {
            assert_eq!(found(not), None, "{not:?}");
        }
        // A name that ends a path but is not a whole part of it is not it.
        assert_eq!(found("b.rs"), None);
    }

    fn defined(
        name: &str,
        kind: SymbolKind,
        container: Option<&str>,
        path: &str,
        line: u32,
    ) -> Defined {
        let at = Position { line, character: 4 };
        Defined {
            name: name.into(),
            kind,
            container: container.map(str::to_string),
            location: Location {
                uri: format!("file:///w/{path}").parse::<Uri>().unwrap(),
                range: LspRange { start: at, end: at },
            },
        }
    }

    fn picked(
        segments: &[&str],
        found: &HashMap<String, Vec<Defined>>,
    ) -> Option<(usize, Vec<String>)> {
        let segments: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
        pick(&segments, found, Path::new("/w")).map(|(k, locations)| {
            let at = locations
                .iter()
                .map(|l| {
                    format!(
                        "{}:{}",
                        l.uri.as_str().trim_start_matches("file:///w/"),
                        l.range.start.line
                    )
                })
                .collect();
            (k, at)
        })
    }

    #[test]
    fn a_name_picks_its_definitions() {
        let mut found: HashMap<String, Vec<Defined>> = HashMap::new();
        found.insert(
            "Db".into(),
            vec![
                defined("Db", SymbolKind::STRUCT, None, "slatedb/src/db.rs", 593),
                defined(
                    "Db",
                    SymbolKind::ENUM_MEMBER,
                    Some("ObjectStoreComponent"),
                    "slatedb/src/ios.rs",
                    52,
                ),
                defined(
                    "Db",
                    SymbolKind::STRUCT,
                    None,
                    "bindings/uniffi/src/db.rs",
                    18,
                ),
                defined(
                    "DbReader",
                    SymbolKind::STRUCT,
                    None,
                    "slatedb/src/db_reader.rs",
                    9,
                ),
            ],
        );
        found.insert(
            "open".into(),
            vec![
                defined(
                    "open",
                    SymbolKind::FUNCTION,
                    Some("Db"),
                    "slatedb/src/db.rs",
                    625,
                ),
                defined(
                    "open",
                    SymbolKind::FUNCTION,
                    Some("DbReader"),
                    "slatedb/src/db_reader.rs",
                    909,
                ),
                defined(
                    "open",
                    SymbolKind::FUNCTION,
                    Some("PeekingIterator<T>"),
                    "slatedb/src/p.rs",
                    1,
                ),
            ],
        );
        found.insert("ttl".into(), Vec::new());
        found.insert("db".into(), Vec::new());
        found.insert(
            "SsTableInfo".into(),
            vec![
                defined(
                    "SsTableInfo",
                    SymbolKind::STRUCT,
                    None,
                    "slatedb/src/db_state.rs",
                    496,
                ),
                defined(
                    "SsTableInfo",
                    SymbolKind::STRUCT,
                    None,
                    "slatedb/src/generated/root_generated.rs",
                    1207,
                ),
            ],
        );
        found.insert(
            "Table".into(),
            vec![defined(
                "Table",
                SymbolKind::STRUCT,
                None,
                "src/table_generated.rs",
                3,
            )],
        );
        found.insert(
            "new".into(),
            (0..40)
                .map(|i| {
                    defined(
                        "new",
                        SymbolKind::FUNCTION,
                        Some(&format!("T{i}")),
                        "x.rs",
                        i,
                    )
                })
                .collect(),
        );

        // The structs, not the variant of the same name, and never the
        // fuzzy match.
        assert_eq!(
            picked(&["Db"], &found),
            Some((
                1,
                vec![
                    "slatedb/src/db.rs:593".into(),
                    "bindings/uniffi/src/db.rs:18".into()
                ]
            ))
        );
        assert_eq!(
            picked(&["Db", "open"], &found),
            Some((2, vec!["slatedb/src/db.rs:625".into()]))
        );
        assert_eq!(
            picked(&["PeekingIterator", "open"], &found),
            Some((2, vec!["slatedb/src/p.rs:1".into()])),
            "a container's generics are not its name"
        );
        // A module or crate qualifies by the path.
        assert_eq!(
            picked(&["slatedb", "Db"], &found),
            Some((2, vec!["slatedb/src/db.rs:593".into()]))
        );
        assert_eq!(
            picked(&["uniffi", "Db"], &found),
            Some((2, vec!["bindings/uniffi/src/db.rs:18".into()]))
        );
        // A field is not indexed, so its type is what links.
        assert_eq!(
            picked(&["Db", "ttl"], &found),
            Some((
                1,
                vec![
                    "slatedb/src/db.rs:593".into(),
                    "bindings/uniffi/src/db.rs:18".into()
                ]
            ))
        );
        // Too many to mean one, alone; one inside its type.
        assert_eq!(picked(&["new"], &found), None);
        assert_eq!(
            picked(&["T7", "new"], &found),
            Some((2, vec!["x.rs:7".into()]))
        );
        assert_eq!(picked(&["Nothing"], &found), None);
        // Only a type stands in for a member it does not find.
        assert_eq!(picked(&["db", "ttl"], &found), None);
        // Generated code gives way to code written by hand, and is kept
        // when there is nothing else.
        assert_eq!(
            picked(&["SsTableInfo"], &found),
            Some((1, vec!["slatedb/src/db_state.rs:496".into()]))
        );
        assert_eq!(
            picked(&["Table"], &found),
            Some((1, vec!["src/table_generated.rs:3".into()]))
        );
        assert_eq!(bare_type("&'a Db"), "Db");
        assert_eq!(bare_type("BytesRange<'a>"), "BytesRange");
    }

    #[test]
    fn there_is_nothing_to_link_to_without_a_checkout() {
        let state = review_of(&["src/db.rs"]);
        assert!(Refs::of(state).is_some());
        state.item.update(|item| {
            let mut bare = (**item.as_ref().unwrap()).clone();
            bare.checkout = None;
            *item = Some(Arc::new(bare));
        });
        assert!(Refs::of(state).is_none());
    }

    #[test]
    fn a_link_to_a_path_in_the_worktree_is_a_file_and_any_other_is_not() {
        let refs = Refs::of(review_of(&["slatedb/src/db.rs"])).unwrap();
        let file = |url: &str| refs.file_for_url(url).map(|f| (f.path, f.line));
        assert_eq!(
            file("slatedb/src/db.rs#L17"),
            Some(("slatedb/src/db.rs".into(), Some(17)))
        );
        // An agent's absolute path, with a line after it, and a file URL.
        assert_eq!(
            file("/tmp/wt/slatedb/src/db.rs:9"),
            Some(("slatedb/src/db.rs".into(), Some(9)))
        );
        assert_eq!(
            file("file:///tmp/wt/slatedb/src/db.rs"),
            Some(("slatedb/src/db.rs".into(), None))
        );
        assert_eq!(
            file("slatedb/src/db.rs#usage"),
            Some(("slatedb/src/db.rs".into(), None))
        );
        for not in [
            "https://github.com/slatedb/slatedb/blob/main/slatedb/src/db.rs",
            "mailto:a@b.c",
            "#usage",
            "/etc/hosts",
            "db.rs?plain=1",
        ] {
            assert_eq!(file(not), None, "{not:?}");
        }
    }

    #[test]
    fn names_are_asked_about_once_and_kept_for_the_ask_still_waiting() {
        let state = review_of(&[]);
        let ask = |key: &str| NameAsk {
            key: key.into(),
            segments: vec![key.into()],
        };
        state.ask_names(&[ask("Db"), ask("Manifest")]);
        state.ask_names(&[ask("Db")]);
        let asked = state.pending_names.get();
        assert_eq!(
            asked.len(),
            1,
            "a name already asked is not asked again: {asked:?}"
        );
        let token = asked[0].0;
        assert_eq!(asked[0].1, vec![ask("Db"), ask("Manifest")]);

        // An answer to another ask, an older item's say, is dropped.
        state.finish_names(token + 1, vec![("Db".into(), None)]);
        assert_eq!(state.names.get().get("Db"), Some(&Named::Asked(token)));
        let found = (1, Arc::new(vec![located("src/db.rs", 594, 11)]));
        state.finish_names(
            token,
            vec![
                ("Db".into(), Some(found.clone())),
                ("Manifest".into(), None),
            ],
        );
        assert_eq!(state.names.get().get("Db"), Some(&Named::Found(found)));
        assert_eq!(state.names.get().get("Manifest"), Some(&Named::Missing));

        // Ready again: what was missing may be found now, and is asked
        // again; what was found stays.
        state.forget_missing_names();
        assert_eq!(state.names.get().get("Manifest"), None);
        assert!(state.names.get().contains_key("Db"));
    }
}
