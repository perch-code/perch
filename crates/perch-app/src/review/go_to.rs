//! Cmd-P: going to a file by its name, as VS Code's Quick Open does.
//!
//! A box over the top of the main pane, and under it the files it
//! matches. Before anything is typed the list is the files the item has
//! had open, the most recent first, then its changes; after, it is the
//! recently opened files that match, then every other file in the
//! worktree that does. The letters a query matched are drawn in blue, in
//! the name and in the directory beside it, and a changed file carries
//! the rail's status letter.
//!
//! The matching is VS Code's own scorer, ported: a query's letters must
//! come in order, a run of them scores more than the same letters apart,
//! and a letter at the start of the text or after a separator more than
//! one inside a word. A match in a file's name outranks one that needs
//! its directory, and a name that starts with the query outranks both.
//! The recently opened files match only as one run, and on the name
//! unless the query names a directory, so what was open does not crowd
//! out what was asked for. Words separated by spaces must all match, in
//! any order, and a query in double quotes must match as one run.
//!
//! `config.rs:42` opens the file at line 42, and `:42`, with no name,
//! goes to line 42 of the open file.
//!
//! The palette is modal while it is up: the window's keys stand down for
//! it, and a press outside it puts it away.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use floem::event::EventPropagation;
use floem::kurbo::Rect;
use floem::peniko::Brush;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::editor::SelectionColor;
use floem::views::{
    dyn_container, virtual_stack, ClipExt, Container, Decorators, Empty, Label,
    PlaceholderTextClass, ScrollExt, Stack, TextInput, VirtualVector,
};
use perch_git::{ChangeStatus, ChangedFile, TreeEntry};

use crate::backend::Backend;
use crate::review::model::{ItemTab, Origin, Phase, ReviewState};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::clickable;
use crate::ui::OS_MOD;

/// The most rows a section lists. The rows are built again for every
/// letter typed, and a query that matches thousands is one to type on.
const MAX_PICKS: usize = 100;

/// The room kept at either side of the palette in a narrow pane.
const GUTTER: f64 = 12.0;

// ---------------------------------------------------------------------
// The query
// ---------------------------------------------------------------------

/// Where in a file to land, counted from one: `:42` is line 42, `:42:3`
/// its third column. A colon with no number after it yet has no line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct At {
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// A query parted into the text the files are matched by and where in
/// the file to land, as VS Code parts it: `name:line`, `name:line:column`,
/// `name:line,column`, `name#line` and `name(line,column)`. A query with
/// none of these is all text.
pub fn parse(query: &str) -> (&str, Option<At>) {
    let body = query.trim_end();
    let body = body.strip_suffix(')').unwrap_or(body);
    let (rest, last) = trailing_digits(body);
    let Some(mark) = rest.chars().next_back() else {
        return (query, None);
    };
    let rest = &rest[..rest.len() - mark.len_utf8()];
    // A line and a column: the one before the mark, after a mark of its
    // own. Tried first, since it starts further left, as the leftmost
    // match of VS Code's pattern does.
    if matches!(mark, '#' | ':' | ',') {
        let (before, first) = trailing_digits(rest);
        if let Some(open) = before
            .chars()
            .next_back()
            .filter(|c| matches!(c, '#' | ':' | '('))
        {
            let at = At {
                line: number(first),
                column: number(last),
            };
            return (&before[..before.len() - open.len_utf8()], Some(at));
        }
    }
    if matches!(mark, '#' | ':' | '(') {
        let at = At {
            line: number(last),
            column: None,
        };
        return (rest, Some(at));
    }
    (query, None)
}

/// The place in a query that is only a place, `:42`: a line of the open
/// file, as VS Code's `:` goes to one. A bare `#42` or `(42)` is not one,
/// and is looked for as a name.
fn line_only(query: &str) -> Option<At> {
    match parse(query) {
        (filter, Some(at)) if filter.trim().is_empty() && query.trim_start().starts_with(':') => {
            Some(at)
        }
        _ => None,
    }
}

/// `text` without the digits at its end, and those digits.
fn trailing_digits(text: &str) -> (&str, &str) {
    let digits = text.len() - text.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    text.split_at(text.len() - digits)
}

/// A number typed, or none for no digits; one too long for a `usize` is
/// the end of any file.
fn number(digits: &str) -> Option<usize> {
    (!digits.is_empty()).then(|| digits.parse().unwrap_or(usize::MAX))
}

/// One word of a query, ready to score with: its letters without what
/// only steers the search, whitespace, quotes and `*`, and those letters
/// in lowercase, one for one.
#[derive(Debug, Clone)]
struct Piece {
    letters: Vec<char>,
    lower: Vec<char>,
    /// It was in double quotes, and must match as one run.
    exact: bool,
}

impl Piece {
    fn new(text: &str) -> Piece {
        let exact = text.starts_with('"') && text.ends_with('"');
        let letters: Vec<char> = text
            .chars()
            .map(|c| if c == '\\' { '/' } else { c })
            .filter(|c| !c.is_whitespace() && *c != '"' && *c != '*')
            .collect();
        let lower = letters.iter().map(|&c| lower(c)).collect();
        Piece {
            letters,
            lower,
            exact,
        }
    }
}

/// A query as the scorer reads it: whole, and in its words when there
/// are several.
struct Prepared {
    /// The query as typed with its backslashes as slashes, for a query
    /// that is a whole path.
    path: String,
    whole: Piece,
    /// The words, when there are two or more; each must match.
    words: Vec<Piece>,
    /// The query names a directory, so it is matched against the path
    /// rather than the name alone.
    separator: bool,
}

impl Prepared {
    fn new(query: &str) -> Prepared {
        let path = query.replace('\\', "/");
        let words: Vec<Piece> = query
            .split(' ')
            .map(Piece::new)
            .filter(|p| !p.letters.is_empty())
            .collect();
        Prepared {
            separator: path.contains('/'),
            path,
            whole: Piece::new(query),
            words: if words.len() > 1 { words } else { Vec::new() },
        }
    }

    fn is_empty(&self) -> bool {
        self.whole.letters.is_empty()
    }

    /// What is scored: the words, or the whole when it is one.
    fn pieces(&self) -> &[Piece] {
        if self.words.is_empty() {
            std::slice::from_ref(&self.whole)
        } else {
            &self.words
        }
    }
}

/// A character in lowercase, one for one, so positions in the lowercase
/// text are positions in the text.
fn lower(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

// ---------------------------------------------------------------------
// Scoring, as VS Code's fuzzyScorer.ts does it
// ---------------------------------------------------------------------

/// A match on the file's name scores over this, so that it outranks any
/// match that needed the directory.
const NAME_SCORE: u32 = 1 << 16;
/// A name that starts with the query scores over this.
const NAME_PREFIX_SCORE: u32 = 1 << 17;
/// A query that is the file's whole path.
const PATH_SCORE: u32 = 1 << 18;

/// How well a file matched, and at which characters of its name and its
/// directory, as ranges of characters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Score {
    score: u32,
    name: Vec<Range<usize>>,
    directory: Vec<Range<usize>>,
}

/// Whether two lowercase characters match: the same, or two path
/// separators.
fn same(a: char, b: char) -> bool {
    a == b || (matches!(a, '/' | '\\') && matches!(b, '/' | '\\'))
}

/// What a character after this one is worth for starting a word.
fn separator_bonus(c: char) -> u32 {
    match c {
        '/' | '\\' => 5,
        '_' | '-' | '.' | ' ' | '\'' | '"' | ':' => 4,
        _ => 0,
    }
}

/// What the query's letter at `target[at]` is worth, `run` letters into
/// a run: one for the match, five for each letter of the run so far, one
/// for the same case, and a bonus for the start of the text, the start of
/// a word, or an uppercase letter inside one.
fn letter_score(
    q: char,
    q_lower: char,
    target: &[char],
    target_lower: &[char],
    at: usize,
    run: u32,
) -> u32 {
    if !same(q_lower, target_lower[at]) {
        return 0;
    }
    let mut score = 1;
    if run > 0 {
        score += run * 5;
    }
    if q == target[at] {
        score += 1;
    }
    if at == 0 {
        score += 8;
    } else {
        let separator = separator_bonus(target[at - 1]);
        if separator > 0 {
            score += separator;
        } else if target[at].is_ascii_uppercase() && run == 0 {
            score += 2;
        }
    }
    score
}

/// VS Code's `scoreFuzzy`: the best score of `piece` in `target`, and the
/// characters it matched, by a matrix of the query's letters against the
/// target's. `None` when the letters are not all there in order, or,
/// without `gaps`, not as one run.
fn fuzzy(
    scores: &mut Vec<u32>,
    runs: &mut Vec<u32>,
    target: &[char],
    target_lower: &[char],
    piece: &Piece,
    gaps: bool,
) -> Option<(u32, Vec<usize>)> {
    let (t, q) = (target.len(), piece.letters.len());
    if q == 0 || t < q {
        return None;
    }
    scores.clear();
    scores.resize(q * t, 0);
    runs.clear();
    runs.resize(q * t, 0);
    for qi in 0..q {
        for ti in 0..t {
            let at = qi * t + ti;
            let left = if ti > 0 { scores[at - 1] } else { 0 };
            let (diagonal, run) = if qi > 0 && ti > 0 {
                (scores[at - t - 1], runs[at - t - 1])
            } else {
                (0, 0)
            };
            // Past the first letter, a letter scores only after the one
            // before it has, so the query matches in order.
            let score = if qi > 0 && diagonal == 0 {
                0
            } else {
                letter_score(
                    piece.letters[qi],
                    piece.lower[qi],
                    target,
                    target_lower,
                    ti,
                    run,
                )
            };
            let in_run = gaps || qi > 0 || target_lower[ti..].starts_with(&piece.lower);
            if score > 0 && diagonal + score >= left && in_run {
                runs[at] = run + 1;
                scores[at] = diagonal + score;
            } else {
                runs[at] = 0;
                scores[at] = left;
            }
        }
    }
    let total = scores[q * t - 1];
    if total == 0 {
        return None;
    }
    // Back from the bottom right: a matched cell is a position, then up
    // and left; any other, left.
    let mut positions = Vec::with_capacity(q);
    let (mut qi, mut ti) = (q, t);
    while qi > 0 && ti > 0 {
        if runs[(qi - 1) * t + ti - 1] == 0 {
            ti -= 1;
        } else {
            positions.push(ti - 1);
            qi -= 1;
            ti -= 1;
        }
    }
    positions.reverse();
    Some((total, positions))
}

/// Positions as ranges, a run of neighbours in one.
fn spans(positions: &[usize]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for &p in positions {
        match out.last_mut() {
            Some(last) if last.end == p => last.end += 1,
            _ => out.push(p..p + 1),
        }
    }
    out
}

/// Ranges in order, those that touch or overlap made one.
fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Whether the letters are in `text` in order, as the matrix needs them
/// to be: a cheap test that turns away most of a large tree before the
/// matrix is built.
fn in_order(text: &str, letters: &[char]) -> bool {
    // An ASCII letter is never part of a longer character in UTF-8, so
    // the bytes will do, and they are most of what a keystroke costs.
    if letters.iter().all(char::is_ascii) {
        let mut want = letters.iter().map(|&c| c as u8);
        let mut next = want.next();
        for b in text.bytes() {
            let Some(w) = next else { break };
            if same(w as char, lower(b as char)) {
                next = want.next();
            }
        }
        return next.is_none();
    }
    let mut want = letters.iter();
    let mut next = want.next();
    for c in text.chars() {
        let Some(&w) = next else { break };
        if same(w, lower(c)) {
            next = want.next();
        }
    }
    next.is_none()
}

/// Scores one query against many files, keeping its buffers between
/// them.
#[derive(Default)]
struct Matcher {
    scores: Vec<u32>,
    runs: Vec<u32>,
    name: Vec<char>,
    name_lower: Vec<char>,
    /// The directory, a slash and the name, when there is a directory.
    path: Vec<char>,
    path_lower: Vec<char>,
    /// How many characters of `path` are the directory.
    directory: usize,
}

impl Matcher {
    /// VS Code's `scoreItemFuzzy`. `directory` is empty to match on the
    /// name alone; `path` is the file's, for a query that is all of it.
    fn score(
        &mut self,
        name: &str,
        directory: &str,
        path: &str,
        query: &Prepared,
        gaps: bool,
    ) -> Option<Score> {
        // A match of one run is a list of one range.
        #[allow(clippy::single_range_in_vec_init)]
        if query.path.len() == path.len() && query.path.to_lowercase() == path.to_lowercase() {
            let whole = |text: &str| match text.chars().count() {
                0 => Vec::new(),
                n => vec![0..n],
            };
            return Some(Score {
                score: PATH_SCORE,
                name: whole(name),
                directory: whole(directory),
            });
        }
        let haystack = if directory.is_empty() { name } else { path };
        if !query.pieces().iter().all(|p| in_order(haystack, &p.lower)) {
            return None;
        }
        self.load(name, directory);
        let prefer_name = !query.separator;
        if query.words.is_empty() {
            return self.single(&query.whole, prefer_name, gaps);
        }
        let mut total = Score::default();
        for word in &query.words {
            let score = self.single(word, prefer_name, gaps)?;
            total.score += score.score;
            total.name.extend(score.name);
            total.directory.extend(score.directory);
        }
        total.name = merged(total.name);
        total.directory = merged(total.directory);
        Some(total)
    }

    fn load(&mut self, name: &str, directory: &str) {
        self.name.clear();
        self.name.extend(name.chars());
        self.name_lower.clear();
        self.name_lower.extend(self.name.iter().map(|&c| lower(c)));
        self.path.clear();
        self.path_lower.clear();
        self.directory = 0;
        if !directory.is_empty() {
            self.path.extend(directory.chars());
            self.directory = self.path.len();
            self.path.push('/');
            self.path.extend(name.chars());
            self.path_lower.extend(self.path.iter().map(|&c| lower(c)));
        }
    }

    /// VS Code's `doScoreItemFuzzySingle`: the name first, unless the
    /// query names a directory, then the directory and the name as one.
    fn single(&mut self, piece: &Piece, prefer_name: bool, gaps: bool) -> Option<Score> {
        let gaps = gaps && !piece.exact;
        if prefer_name || self.directory == 0 {
            if let Some((score, positions)) = fuzzy(
                &mut self.scores,
                &mut self.runs,
                &self.name,
                &self.name_lower,
                piece,
                gaps,
            ) {
                // A name that starts with the query, and the more of the
                // name it is the better: `window` wants `window.ts` over
                // `windowActions.ts`.
                if self.name_lower.starts_with(&piece.lower) {
                    let share = piece.letters.len() as f64 / self.name.len() as f64;
                    #[allow(clippy::single_range_in_vec_init)]
                    return Some(Score {
                        score: NAME_PREFIX_SCORE + (share * 100.0).round() as u32 + score,
                        name: vec![0..piece.letters.len()],
                        directory: Vec::new(),
                    });
                }
                return Some(Score {
                    score: NAME_SCORE + score,
                    name: spans(&positions),
                    directory: Vec::new(),
                });
            }
        }
        if self.directory == 0 {
            return None;
        }
        let (score, positions) = fuzzy(
            &mut self.scores,
            &mut self.runs,
            &self.path,
            &self.path_lower,
            piece,
            gaps,
        )?;
        // Back onto the two parts, a run across the slash onto both; the
        // slash itself is neither's.
        let split = self.directory + 1;
        let (mut name, mut directory) = (Vec::new(), Vec::new());
        for r in spans(&positions) {
            if r.end > split {
                name.push(r.start.max(split) - split..r.end - split);
            }
            if r.start < self.directory {
                directory.push(r.start..r.end.min(self.directory));
            }
        }
        Some(Score {
            score,
            name,
            directory,
        })
    }
}

/// A file that matched, with what the order needs of it.
struct Found<'a> {
    path: &'a str,
    name: &'a str,
    score: Score,
    /// The path, the name, and the directory as it was matched, empty
    /// when only the name was, in characters: counted once rather than in
    /// every comparison.
    lengths: (usize, usize, usize),
}

impl<'a> Found<'a> {
    fn new(path: &'a str, name: &'a str, directory: &'a str, score: Score) -> Self {
        let count = |s: &str| {
            if s.is_ascii() {
                s.len()
            } else {
                s.chars().count()
            }
        };
        Found {
            path,
            name,
            score,
            lengths: (count(path), count(name), count(directory)),
        }
    }
}

/// VS Code's `compareItemsByFuzzyScore`: the whole path first, then the
/// better match on the name, the shorter run of it, and the shorter name;
/// then the better score, a match in the name before one without, the
/// tighter match, and last the shorter and then the earlier text.
fn compare(a: &Found, b: &Found) -> Ordering {
    let (sa, sb) = (a.score.score, b.score.score);
    if (sa == PATH_SCORE || sb == PATH_SCORE) && sa != sb {
        return if sa == PATH_SCORE {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if sa > NAME_SCORE || sb > NAME_SCORE {
        if sa != sb {
            return sb.cmp(&sa);
        }
        // A prefix longer is a better prefix; any other match is
        // better shorter.
        if sa < NAME_PREFIX_SCORE && sb < NAME_PREFIX_SCORE {
            let by_length = run_length(&a.score.name, &b.score.name);
            if by_length != Ordering::Equal {
                return by_length;
            }
        }
        let (la, lb) = (a.lengths.1, b.lengths.1);
        if la != lb {
            return la.cmp(&lb);
        }
    }
    if sa != sb {
        return sb.cmp(&sa);
    }
    let (na, nb) = (!a.score.name.is_empty(), !b.score.name.is_empty());
    if na != nb {
        return if na {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let (da, db) = (distance(a), distance(b));
    if da != 0 && db != 0 && da != db {
        return da.cmp(&db);
    }
    fallback(a, b)
}

/// The shorter span from the first matched character to the last first.
fn run_length(a: &[Range<usize>], b: &[Range<usize>]) -> Ordering {
    match (a.first().zip(a.last()), b.first().zip(b.last())) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some((a0, a1)), Some((b0, b1))) => (a1.end - a0.start).cmp(&(b1.end - b0.start)),
    }
}

/// How far a match spreads, from its first character in the directory
/// or the name to its last in the name, the directory counted in.
fn distance(found: &Found) -> i64 {
    let s = &found.score;
    let start = s
        .directory
        .first()
        .or(s.name.first())
        .map_or(-1, |r| r.start as i64);
    let end = match (s.name.last(), s.directory.last()) {
        (Some(last), None) => last.end as i64,
        (Some(last), Some(_)) => last.end as i64 + found.lengths.2 as i64,
        (None, Some(last)) => last.end as i64,
        (None, None) => -1,
    };
    end - start
}

/// Equal matches: the shorter name and directory, the shorter path, then
/// the names in order, ignoring case.
fn fallback(a: &Found, b: &Found) -> Ordering {
    let length = |f: &Found| f.lengths.1 + f.lengths.2;
    length(a)
        .cmp(&length(b))
        .then_with(|| a.lengths.0.cmp(&b.lengths.0))
        .then_with(|| a.name.chars().map(lower).cmp(b.name.chars().map(lower)))
        .then_with(|| a.name.cmp(b.name))
        .then_with(|| a.path.cmp(b.path))
}

/// The best `n` in order.
fn best(mut found: Vec<Found>, n: usize) -> Vec<Found> {
    if found.len() > n {
        found.select_nth_unstable_by(n - 1, compare);
        found.truncate(n);
    }
    found.sort_by(compare);
    found
}

// ---------------------------------------------------------------------
// What the palette lists
// ---------------------------------------------------------------------

/// The parts of the list, each named on its first row as VS Code names
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Recent,
    Changed,
    Files,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Recent => "recently opened",
            Section::Changed => "changed files",
            Section::Files => "file results",
        }
    }
}

/// A file in the list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hit {
    /// What opening it opens: relative to the worktree, or absolute for a
    /// file outside it.
    pub path: String,
    pub name: String,
    /// Its directory in the worktree, empty at the top; for a file
    /// outside it, where it is from, as its tab's chip says.
    pub directory: String,
    /// The characters the query matched, as ranges of characters.
    pub name_matches: Vec<Range<usize>>,
    pub directory_matches: Vec<Range<usize>>,
    pub section: Section,
    /// The rail's status, for a changed file.
    pub status: Option<ChangeStatus>,
}

/// A row that Enter acts on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Pick {
    File(Hit),
    /// A line of the open file, from `:42`.
    Line {
        path: String,
        name: String,
        line: usize,
        column: Option<usize>,
    },
}

impl Pick {
    fn section(&self) -> Option<Section> {
        match self {
            Pick::File(hit) => Some(hit.section),
            Pick::Line { .. } => None,
        }
    }
}

/// What a query lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub picks: Vec<Pick>,
    /// What to say in place of rows: that nothing matched, or what `:`
    /// wants.
    pub note: Option<String>,
    /// Where in a picked file to land.
    pub at: Option<At>,
}

/// Every file the palette can go to, in a list to score and a set to
/// look one up in, made once each time the palette opens rather than for
/// every letter typed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Files {
    paths: Vec<String>,
    set: HashSet<String>,
}

impl Files {
    pub fn new(paths: Vec<String>) -> Files {
        let set = paths.iter().cloned().collect();
        Files { paths, set }
    }

    fn contains(&self, path: &str) -> bool {
        self.set.contains(path)
    }
}

/// What the list is made from.
pub struct Sources<'a> {
    /// Every file in the worktree, and the changed files it lacks, which
    /// are the deleted ones.
    pub files: &'a Files,
    /// The files the item has had open, the most recent first.
    pub recent: &'a [String],
    /// The changed files, in the rail's order.
    pub changed: &'a [ChangedFile],
    /// The open file and how many lines it has, when it is text.
    pub active: Option<(&'a str, usize)>,
}

/// Every file the palette can go to: the worktree's files in the tree's
/// order, then the changed files the tree lacks, which are the deleted
/// ones, and a changed directory, a submodule, which opens as a file.
pub fn candidates(tree: &[TreeEntry], changed: &[ChangedFile]) -> Vec<String> {
    let mut files: Vec<String> = tree
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.path.clone())
        .collect();
    let known: HashSet<String> = files.iter().cloned().collect();
    files.extend(
        changed
            .iter()
            .filter(|f| !known.contains(&f.path))
            .map(|f| f.path.clone()),
    );
    files
}

fn is_outside(path: &str) -> bool {
    std::path::Path::new(path).is_absolute()
}

/// A path's name and its directory, without the slash between.
fn parts(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[i + 1..], &path[..i]),
        None => (path, ""),
    }
}

/// What a query lists.
pub fn results(query: &str, sources: &Sources) -> Results {
    if let Some(at) = line_only(query) {
        return line(at, sources.active);
    }
    let (filter, at) = match parse(query) {
        (filter, Some(_)) if filter.trim().is_empty() => (query, None),
        parsed => parsed,
    };

    let query = Prepared::new(filter);
    let status: HashMap<&str, ChangeStatus> = sources
        .changed
        .iter()
        .map(|f| (f.path.as_str(), f.status))
        .collect();
    // A file that has gone since it was open is not offered.
    let recent: Vec<&str> = sources
        .recent
        .iter()
        .map(String::as_str)
        .filter(|p| is_outside(p) || sources.files.contains(p))
        .collect();
    let hit = |path: &str, section: Section, score: Score| {
        let (name, directory) = parts(path);
        let directory = match Origin::of_path(path) {
            Origin::Outside { label } => label,
            Origin::Worktree => directory.to_string(),
        };
        Pick::File(Hit {
            path: path.to_string(),
            name: name.to_string(),
            directory,
            name_matches: score.name,
            directory_matches: score.directory,
            section,
            status: status.get(path).copied(),
        })
    };

    if query.is_empty() {
        let listed: HashSet<&str> = recent.iter().copied().collect();
        let mut picks: Vec<Pick> = recent
            .iter()
            .map(|p| hit(p, Section::Recent, Score::default()))
            .collect();
        picks.extend(
            sources
                .changed
                .iter()
                .map(|f| f.path.as_str())
                .filter(|p| !listed.contains(p))
                .take(MAX_PICKS)
                .map(|p| hit(p, Section::Changed, Score::default())),
        );
        return Results {
            picks,
            note: None,
            at,
        };
    }

    let mut matcher = Matcher::default();
    // What was open matches as one run, and on its name unless the query
    // names a directory; a file outside the worktree on its name alone.
    let found: Vec<Found> = recent
        .iter()
        .filter_map(|&path| {
            let (name, directory) = parts(path);
            let directory = if query.separator && !is_outside(path) {
                directory
            } else {
                ""
            };
            let score = matcher.score(name, directory, path, &query, false)?;
            Some(Found::new(path, name, directory, score))
        })
        .collect();
    let found = best(found, MAX_PICKS);
    // A recent file that did not match as one might still match as any
    // other file does.
    let listed: HashSet<&str> = found.iter().map(|f| f.path).collect();
    let mut picks: Vec<Pick> = found
        .into_iter()
        .map(|f| hit(f.path, Section::Recent, f.score))
        .collect();
    let found: Vec<Found> = sources
        .files
        .paths
        .iter()
        .map(String::as_str)
        .filter(|p| !listed.contains(p))
        .filter_map(|path| {
            let (name, directory) = parts(path);
            let score = matcher.score(name, directory, path, &query, true)?;
            Some(Found::new(path, name, directory, score))
        })
        .collect();
    picks.extend(
        best(found, MAX_PICKS)
            .into_iter()
            .map(|f| hit(f.path, Section::Files, f.score)),
    );
    let note = picks.is_empty().then(|| "No matching files".to_string());
    Results { picks, note, at }
}

/// `:42`: line 42 of the open file, or what the palette wants first.
fn line(at: At, active: Option<(&str, usize)>) -> Results {
    let noted = |note: String| Results {
        picks: Vec::new(),
        note: Some(note),
        at: Some(at),
    };
    let Some((path, lines)) = active else {
        return noted("Open a file to go to a line in it".to_string());
    };
    let Some(line) = at.line else {
        return noted(format!("Type a line number between 1 and {lines}"));
    };
    Results {
        picks: vec![Pick::Line {
            path: path.to_string(),
            name: parts(path).0.to_string(),
            line,
            column: at.column,
        }],
        note: None,
        at: Some(at),
    }
}

// ---------------------------------------------------------------------
// State and actions
// ---------------------------------------------------------------------

/// The palette's state, in the review state. Signals, so it is `Copy`
/// like the state it is part of.
#[derive(Clone, Copy)]
pub struct GoTo {
    /// Whether the palette is up.
    pub open: RwSignal<bool>,
    /// What is typed in its box, empty each time it opens, as VS Code's
    /// is.
    pub query: RwSignal<String>,
    /// The row Enter opens: the first, whenever the list changes.
    pub selected: RwSignal<usize>,
    /// What the query lists, made by the palette while it is up.
    pub results: RwSignal<Arc<Results>>,
    /// Whatever had the keyboard when Cmd-P was pressed, for Escape to
    /// give it back to.
    pub opener: RwSignal<Option<floem::ViewId>>,
    /// The box while the palette is up, so a press on the palette can
    /// give the keyboard back to it.
    pub text_box: RwSignal<Option<floem::ViewId>>,
}

impl Default for GoTo {
    fn default() -> Self {
        Self::new()
    }
}

impl GoTo {
    pub fn new() -> Self {
        GoTo {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            selected: RwSignal::new(0),
            results: RwSignal::new(Arc::new(Results::default())),
            opener: RwSignal::new(None),
            text_box: RwSignal::new(None),
        }
    }

    /// Put away with the checkout it was over.
    pub fn reset(&self) {
        if self.open.get_untracked() {
            self.open.set(false);
        }
        self.opener.set(None);
    }
}

/// Whether Cmd-P has anything to go to: an item open with a checkout,
/// or an issue with the default branch to read.
pub fn available(state: ReviewState) -> bool {
    state.is_open()
        && matches!(state.phase.get_untracked(), Phase::Ready | Phase::Issue)
        && floem::reactive::Effect::untrack(|| state.files_root().is_some())
}

/// Cmd-P: the palette up, empty, with the keyboard, which goes back to
/// `opener` when Escape puts it away.
pub fn open(state: ReviewState, opener: Option<floem::ViewId>) {
    state.go_to.opener.set(opener);
    state.go_to.query.set(String::new());
    state.go_to.selected.set(0);
    state.go_to.open.set(true);
}

/// The palette away. With `restore_focus`, the keyboard goes back to
/// what had it; otherwise, or with nothing to go back to, it is nobody's
/// rather than the gone box's.
pub fn close(state: ReviewState, restore_focus: bool) {
    let text_box = state.go_to.text_box.get_untracked();
    let opener = state.go_to.opener.get_untracked();
    state.go_to.opener.set(None);
    state.go_to.open.set(false);
    match opener.filter(|_| restore_focus) {
        Some(opener) => opener.request_focus(),
        None => {
            if let Some(id) = text_box {
                id.clear_focus();
            }
        }
    }
}

/// Up and down the list, wrapping at the ends, as VS Code's does.
fn step(state: ReviewState, by: isize) {
    let n = state.go_to.results.with_untracked(|r| r.picks.len());
    if n == 0 {
        return;
    }
    let at = state.go_to.selected.get_untracked() as isize;
    state
        .go_to
        .selected
        .set((at + by).rem_euclid(n as isize) as usize);
}

/// A page up or down the list, stopping at its ends.
fn page(state: ReviewState, down: bool) {
    let n = state.go_to.results.with_untracked(|r| r.picks.len());
    if n == 0 {
        return;
    }
    let rows = (theme::GO_TO_LIST_HEIGHT / theme::GO_TO_ROW_HEIGHT) as usize;
    let at = state.go_to.selected.get_untracked();
    let to = if down {
        (at + rows).min(n - 1)
    } else {
        at.saturating_sub(rows)
    };
    state.go_to.selected.set(to);
}

/// Enter, or a click: open the row's file on Code, at the query's line
/// when it has one. The file on screen stays, and gets the keyboard back
/// when it had it; another file is another editor, and the keyboard is
/// nobody's until the reader clicks.
pub fn accept(state: ReviewState, open_file: &OpenFile, index: usize) {
    let Some((pick, at)) = state
        .go_to
        .results
        .with_untracked(|r| r.picks.get(index).map(|p| (p.clone(), r.at)))
    else {
        return;
    };
    let (path, line, column) = match pick {
        Pick::File(hit) => (hit.path, at.and_then(|a| a.line), at.and_then(|a| a.column)),
        Pick::Line {
            path, line, column, ..
        } => (path, Some(line), column),
    };
    let on_files = state.tab.get_untracked() == ItemTab::Code;
    let on_screen = state
        .active
        .with_untracked(|a| a.as_deref() == Some(path.as_str()));
    close(state, on_files && on_screen);
    state.one_move(|| {
        if !on_files {
            state.show_tab(ItemTab::Code);
        }
        match line {
            Some(line) => {
                state.navigate_to(&path, line.max(1) - 1, column.map_or(0, |c| c.max(1) - 1))
            }
            None if on_screen => {}
            None => open_file(&path),
        }
    });
}

/// The palette's keys, from the window while it is up: Escape, Enter,
/// the arrows, Ctrl-N and Ctrl-P, the page keys, and Cmd-P again, which
/// walks down the list as VS Code's does. Off macOS the command key is
/// Ctrl, so Ctrl-P is Cmd-P's and walks down, as it does in VS Code there,
/// and only the arrows and Page Up walk up. The rest are the box's.
pub fn key(state: ReviewState, open_file: &OpenFile, key: &KeyboardEvent) -> EventPropagation {
    // A key an input method is composing with is the method's: Enter
    // takes what it composed.
    if key.is_composing {
        return EventPropagation::Continue;
    }
    let plain = key.modifiers.is_empty();
    let ctrl = key.modifiers == Modifiers::CONTROL;
    match key.code {
        Code::Escape if plain => close(state, true),
        Code::Enter if plain => accept(state, open_file, state.go_to.selected.get_untracked()),
        Code::ArrowDown if plain => step(state, 1),
        Code::ArrowUp if plain => step(state, -1),
        Code::KeyP if key.modifiers == OS_MOD => step(state, 1),
        Code::KeyN if ctrl => step(state, 1),
        Code::KeyP if ctrl && cfg!(target_os = "macos") => step(state, -1),
        Code::PageDown if plain => page(state, true),
        Code::PageUp if plain => page(state, false),
        // Tab would walk the keyboard out of the box, and there is
        // nowhere else in the palette for it to go.
        Code::Tab => {}
        _ => return EventPropagation::Continue,
    }
    EventPropagation::Stop
}

/// The open file and how many lines its head has, when it is text on the
/// screen, for `:42`.
fn active_lines(state: ReviewState) -> Option<(String, usize)> {
    let file = state.active_file()?;
    if file.truncated || state.drawn_picture().is_some() {
        return None;
    }
    let lines = file.buffer.rows.iter().filter(|r| !r.is_deleted()).count();
    (lines > 0).then(|| (file.path.clone(), lines))
}

/// Keeps the list to the query while the palette is up, the first row
/// picked whenever it changes. The open file is read only for `:`, so a
/// file landing meanwhile does not move the pick back to the top.
fn wire(state: ReviewState) {
    let files = Memo::new(move |_| {
        let tree = state.tree.get();
        Arc::new(Files::new(
            state
                .changed_files
                .with(|changed| candidates(&tree, changed)),
        ))
    });
    Effect::new(move |_| {
        let query = state.go_to.query.get();
        let files = files.get();
        let recent = state.recent();
        let changed = state.changed_files.get();
        let active = line_only(&query).and_then(|_| active_lines(state));
        let results = results(
            &query,
            &Sources {
                files: &files,
                recent: &recent,
                changed: &changed,
                active: active.as_ref().map(|(p, n)| (p.as_str(), *n)),
            },
        );
        state.go_to.results.set(Arc::new(results));
        state.go_to.selected.set(0);
    });
}

// ---------------------------------------------------------------------
// The palette
// ---------------------------------------------------------------------

/// Where the palette sits across the window, as its left edge and its
/// width: centred over the main pane, as wide as VS Code's or as the pane
/// allows with room at its sides. Before the pane has been laid out, over
/// the window.
pub(crate) fn placement(pane: (f64, f64), window: f64) -> (f64, f64) {
    let (left, right) = if pane.1 > pane.0 { pane } else { (0.0, window) };
    let width = theme::GO_TO_WIDTH.min((right - left - 2.0 * GUTTER).max(0.0));
    (left + (right - left - width) / 2.0, width)
}

/// The palette over the window, while it is up. A press anywhere outside
/// it puts it away and gives the keyboard back, as a click away from VS
/// Code's does, and goes no further.
pub fn palette(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let open_file = crate::review::open_file(state, backend);
    // A memo: a write that leaves it up must not build it again, with an
    // empty box and no keyboard.
    let shown = Memo::new(move |_| state.go_to.open.get());
    dyn_container(
        move || shown.get(),
        move |open| {
            if !open {
                return Empty::new().into_any();
            }
            wire(state);
            Container::new(panel(app, state, open_file.clone()))
                .on_event_stop(el::PointerDown, move |_, _| close(state, true))
                .style(|s| s.size_full())
                .into_any()
        },
    )
    .debug_name("go-to")
    .style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(100)
            .apply_if(!shown.get(), |s| s.hide())
    })
}

fn panel(app: AppState, state: ReviewState, open_file: OpenFile) -> impl IntoView {
    Stack::vertical((query_box(state), list(state, open_file)))
        // A press on the palette is not one outside it, and gives the
        // keyboard back to the box, which a press on anything takes.
        .on_event_stop(el::PointerDown, move |_, _| {
            if let Some(id) = state.go_to.text_box.get_untracked() {
                id.request_focus();
            }
        })
        .debug_name("go-to-palette")
        .style(move |s| {
            let (left, width) = placement(app.main_pane.get(), app.window_width.get());
            s.position(Position::Absolute)
                .inset_top(theme::GO_TO_TOP)
                .inset_left(left)
                .width(width)
                .flex_col()
                .padding(theme::GO_TO_PADDING)
                .apply(crate::ui::menu::surface(floem::style::Style::new()))
        })
}

fn query_box(state: ReviewState) -> impl IntoView {
    let input = TextInput::new(state.go_to.query)
        .placeholder("Search files by name (append : to go to line)")
        .debug_name("go-to-box")
        .style(|s| {
            s.width_full()
                .height(theme::GO_TO_BOX_HEIGHT)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(8.0)
                .background(theme::canvas())
                .hover(|s| s.background(theme::canvas()))
                .border(1.0)
                .border_color(theme::border())
                .border_radius(crate::ui::menu::ROW_RADIUS)
                .focus(|s| s.border_color(theme::accent()))
                .set(SelectionColor, Brush::Solid(theme::selection_bg()))
                .class(PlaceholderTextClass, |s| s.color(theme::fg_subtle()))
        });
    let id = input.view_id();
    state.go_to.text_box.set(Some(id));
    // The box takes the keyboard once it is in the tree, where a request
    // can find it.
    let taken = RwSignal::new(false);
    input
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if !taken.get_untracked() {
                taken.set(true);
                id.request_focus();
            }
        })
        .on_cleanup(move || {
            if state.go_to.text_box.get_untracked() == Some(id) {
                state.go_to.text_box.set(None);
            }
        })
}

/// A row as the list draws it: which it is, the section it starts, if
/// any, and whether a rule sets it off from the section above.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Row {
    index: usize,
    heading: Option<Section>,
    rule: bool,
    pick: Pick,
}

fn rows(results: &Results) -> Arc<Vec<Row>> {
    let mut last = None;
    Arc::new(
        results
            .picks
            .iter()
            .enumerate()
            .map(|(index, pick)| {
                let section = pick.section();
                let heading = section.filter(|_| section != last);
                last = section;
                Row {
                    index,
                    heading,
                    rule: heading.is_some() && index > 0,
                    pick: pick.clone(),
                }
            })
            .collect(),
    )
}

/// The rows as the virtual stack reads them, a slice at a time.
struct Rows(Arc<Vec<Row>>);

impl VirtualVector<Row> for Rows {
    fn total_len(&self) -> usize {
        self.0.len()
    }

    fn slice(&self, range: Range<usize>) -> impl Iterator<Item = Row> {
        self.0[range].iter().cloned()
    }
}

/// The rows, or what to say in their place; nothing at all while there
/// is neither, as before anything is typed in an item with nothing open
/// and nothing changed. Only the rows in view are built, as VS Code
/// builds its: every letter typed is a new list, and a hundred rows of
/// labels are too many to build between two keys.
fn list(state: ReviewState, open_file: OpenFile) -> impl IntoView {
    let rows = Memo::new(move |_| state.go_to.results.with(|r| rows(r)));
    let note = Memo::new(move |_| state.go_to.results.with(|r| r.note.clone()));
    let empty = move || rows.with(|r| r.is_empty()) && note.with(Option::is_none);
    Stack::vertical((
        virtual_stack(
            move || Rows(rows.get()),
            |row| row.clone(),
            move |row| row_view(state, open_file.clone(), row),
        )
        .item_size_fixed(|| theme::GO_TO_ROW_HEIGHT)
        .style(|s| s.flex_col().width_full())
        .scroll()
        .ensure_visible(move || {
            let at = state.go_to.selected.get() as f64 * theme::GO_TO_ROW_HEIGHT;
            Rect::new(0.0, at, 1.0, at + theme::GO_TO_ROW_HEIGHT)
        })
        // floem keeps a bar's width whether or not anything overflows. The
        // rows never scroll sideways, so there is none under them, and the
        // scroll reaches into the palette's padding, where its bar runs,
        // so the rows are as wide as the box, as a card's scroll does.
        .style(|s| {
            s.max_height(theme::GO_TO_LIST_HEIGHT)
                .overflow_x(floem::taffy::Overflow::Hidden)
                .scrollbar_width(theme::GO_TO_PADDING)
                .margin_right(-theme::GO_TO_PADDING)
        }),
        Label::derived(move || note.get().unwrap_or_default())
            .debug_name("go-to-note")
            .style(move |s| {
                s.height(theme::GO_TO_ROW_HEIGHT)
                    .items_center()
                    .padding_horiz(8.0)
                    .font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .selectable(false)
                    .apply_if(note.with(Option::is_none), |s| s.hide())
            }),
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .margin_top(theme::GO_TO_PADDING)
            .apply_if(empty(), |s| s.hide())
    })
}

fn row_view(state: ReviewState, open_file: OpenFile, row: Row) -> impl IntoView {
    let index = row.index;
    let selected = move || state.go_to.selected.get() == index;
    let (name, content) = match &row.pick {
        Pick::File(hit) => (hit.path.clone(), file_row(hit, row.heading).into_any()),
        Pick::Line {
            name, line, column, ..
        } => (
            format!(":{line}"),
            line_row(name, *line, *column).into_any(),
        ),
    };
    let rule = row.rule;
    Container::new(Container::new(content).style(move |s| {
        let s = s
            .size_full()
            .items_center()
            .padding_horiz(8.0)
            .border_radius(crate::ui::menu::ROW_RADIUS);
        if selected() {
            s.background(theme::menu_hover())
        } else {
            s.hover(|s| s.background(theme::menu_pointed()))
        }
    }))
    .on_event_stop(el::Click, move |_, _| accept(state, &open_file, index))
    .debug_name(format!("go-to-row:{name}"))
    .style(move |s| {
        clickable(s)
            .width_full()
            .height(theme::GO_TO_ROW_HEIGHT)
            .flex_shrink(0.0_f32)
            .apply_if(rule, |s| {
                s.border_top(1.0).border_color(theme::border_muted())
            })
    })
}

/// A file's row: the tabs' dot for its language, its name, its directory
/// in the subtle colour, and at the right end the name of the section it
/// starts and its status letter when it is a change.
pub(crate) fn file_row(hit: &Hit, heading: Option<Section>) -> impl IntoView {
    let color = theme::language_color(perch_syntax::Language::from_path(&hit.name));
    Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(7.0, 7.0)
                .border_radius(4.0)
                .flex_shrink(0.0_f32)
                .margin_right(8.0)
                .background(color)
        }),
        // A name as long as the row gives way before it runs out of the
        // palette, the directory first.
        highlighted(&hit.name, &hit.name_matches, theme::TEXT_META, theme::fg)
            .clip()
            .style(|s| s.flex_shrink(0.0_f32).max_width_pct(80.0)),
        highlighted(
            &hit.directory,
            &hit.directory_matches,
            theme::TEXT_BADGE,
            theme::fg_subtle,
        )
        .clip()
        .style(|s| s.margin_left(6.0).min_width(0.0).flex_shrink(1.0_f32)),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        match heading {
            Some(section) => Label::new(section.label())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_subtle())
                        .margin_left(12.0)
                        .flex_shrink(0.0_f32)
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        match hit.status {
            Some(status) => crate::review::rail::status_letter(status)
                .style(|s| s.margin_left(8.0))
                .into_any(),
            None => Empty::new().into_any(),
        },
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// `:42`'s row: the line, and the name of the file it is in.
fn line_row(name: &str, line: usize, column: Option<usize>) -> impl IntoView {
    let text = match column {
        Some(column) => format!("Go to line {line}, column {column}"),
        None => format!("Go to line {line}"),
    };
    Stack::horizontal((
        Label::new(text).style(|s| s.font_size(theme::TEXT_META).color(theme::fg())),
        Label::new(name.to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .margin_left(6.0)
        }),
    ))
    .style(|s| s.items_center())
}

/// `text` cut at the edges of `matches`, each run with whether it
/// matched.
fn cut(text: &str, matches: &[Range<usize>]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let hit = matches.iter().any(|r| r.contains(&i));
        match out.last_mut() {
            Some((run, was)) if *was == hit => run.push(c),
            _ => out.push((c.to_string(), hit)),
        }
    }
    out
}

/// Text with the characters a query matched in the match colour and
/// heavier, as a row of labels, one to a run: floem's rich text wraps and
/// has no ellipsis, and a long directory wants one. The last run takes
/// it.
pub(crate) fn highlighted(
    text: &str,
    matches: &[Range<usize>],
    size: f32,
    color: impl theme::Ink,
) -> Stack {
    let runs = cut(text, matches);
    let last = runs.len().saturating_sub(1);
    Stack::horizontal_from_iter(runs.into_iter().enumerate().map(move |(i, (run, hit))| {
        Label::new(run).style(move |s| {
            let s = s
                .font_size(size)
                .color(if hit { theme::match_fg() } else { color() })
                .apply_if(hit, |s| s.font_weight(theme::weight_strong()));
            if i == last {
                s.min_width(0.0).text_ellipsis()
            } else {
                s.flex_shrink(0.0_f32)
            }
        })
    }))
    .style(|s| s.items_center().min_width(0.0))
}

// A match of one run is a list of one range.
#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    fn changed(path: &str, status: ChangeStatus) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status,
            additions: 1,
            deletions: 1,
            binary: false,
            head_blob: (status != ChangeStatus::Deleted).then(|| format!("blob-{path}")),
        }
    }

    fn listed(results: &Results) -> Vec<(&str, Section)> {
        results
            .picks
            .iter()
            .map(|pick| match pick {
                Pick::File(hit) => (hit.path.as_str(), hit.section),
                Pick::Line { .. } => panic!("a line among the files"),
            })
            .collect()
    }

    fn hit<'a>(results: &'a Results, path: &str) -> &'a Hit {
        results
            .picks
            .iter()
            .find_map(|pick| match pick {
                Pick::File(hit) if hit.path == path => Some(hit),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{path} is not listed"))
    }

    fn search(query: &str, files: &[String], recent: &[String]) -> Results {
        results(
            query,
            &Sources {
                files: &Files::new(files.to_vec()),
                recent,
                changed: &[],
                active: None,
            },
        )
    }

    fn at(line: Option<usize>, column: Option<usize>) -> Option<At> {
        Some(At { line, column })
    }

    #[test]
    fn a_place_is_parted_off_the_query_as_vs_code_parts_it() {
        assert_eq!(parse("config.rs"), ("config.rs", None));
        assert_eq!(parse("config.rs:42"), ("config.rs", at(Some(42), None)));
        assert_eq!(
            parse("config.rs:42:7"),
            ("config.rs", at(Some(42), Some(7)))
        );
        assert_eq!(
            parse("config.rs:42,7"),
            ("config.rs", at(Some(42), Some(7)))
        );
        assert_eq!(parse("config.rs#42"), ("config.rs", at(Some(42), None)));
        assert_eq!(
            parse("config.rs(42,7)"),
            ("config.rs", at(Some(42), Some(7)))
        );
        assert_eq!(parse("config.rs(42)"), ("config.rs", at(Some(42), None)));
        assert_eq!(parse("config.rs:42  "), ("config.rs", at(Some(42), None)));
        // A colon typed and no number yet: a place to come.
        assert_eq!(parse("config.rs:"), ("config.rs", at(None, None)));
        assert_eq!(parse("config.rs:42:"), ("config.rs", at(Some(42), None)));
        // No name: a line of the open file.
        assert_eq!(parse(":42"), ("", at(Some(42), None)));
        assert_eq!(parse(":"), ("", at(None, None)));
        // Only the end is a place.
        assert_eq!(parse("a:b:3"), ("a:b", at(Some(3), None)));
        assert_eq!(parse("a1:2"), ("a1", at(Some(2), None)));
        assert_eq!(parse("v1.2"), ("v1.2", None));
        assert_eq!(parse("2024"), ("2024", None));
        // A number too long to count is past the end of any file.
        assert_eq!(
            parse(":99999999999999999999999"),
            ("", at(Some(usize::MAX), None))
        );
    }

    /// The list the second of VS Code's pictures shows for `fi` in
    /// slatedb, row for row: the recently opened files that have it as one
    /// run, a name that starts with it first and then the shorter names,
    /// in no order of recency; then every other file, where the letters
    /// may be apart, so `favicon.ico` has them.
    #[test]
    fn fi_lists_what_vs_code_lists_in_its_order() {
        let recent = strings(&[
            "slatedb/src/config.rs",
            "slatedb/src/db/builder.rs",
            "slatedb-dst/src/prefix_extractor.rs",
            "specs/fizzbee/WalProtocol.fizz",
            "slatedb/src/compaction_filter.rs",
            "slatedb/tests/prefix_filter.rs",
            "AGENTS.md",
            "slatedb-dst/src/deterministic_local_filesystem.rs",
            "bindings/uniffi/src/config.rs",
            "slatedb/src/filter.rs",
            "slatedb/src/prefix_extractor.rs",
        ]);
        let mut files = strings(&[
            "README.md",
            "slatedb/src/db.rs",
            "specs/fizzbee/KeyValueStore.fizz",
            "slatedb/tests/range_filter.rs",
            "website/public/img/favicon.ico",
            "slatedb/src/filter_iterator.rs",
            "bindings/uniffi/src/filter_policy.rs",
            "slatedb/src/filter_policy.rs",
            "website/src/content/docs/docs/design/files.mdx",
            "slatedb/src/garbage_collector/filter.rs",
        ]);
        files.extend(recent.iter().cloned());

        let found = search("fi", &files, &recent);
        assert_eq!(
            listed(&found),
            [
                ("slatedb/src/filter.rs", Section::Recent),
                ("slatedb/tests/prefix_filter.rs", Section::Recent),
                ("specs/fizzbee/WalProtocol.fizz", Section::Recent),
                ("slatedb/src/compaction_filter.rs", Section::Recent),
                (
                    "slatedb-dst/src/deterministic_local_filesystem.rs",
                    Section::Recent
                ),
                ("slatedb/src/config.rs", Section::Recent),
                ("bindings/uniffi/src/config.rs", Section::Recent),
                ("slatedb/src/prefix_extractor.rs", Section::Recent),
                ("slatedb-dst/src/prefix_extractor.rs", Section::Recent),
                ("slatedb/src/garbage_collector/filter.rs", Section::Files),
                (
                    "website/src/content/docs/docs/design/files.mdx",
                    Section::Files
                ),
                ("slatedb/src/filter_policy.rs", Section::Files),
                ("bindings/uniffi/src/filter_policy.rs", Section::Files),
                ("slatedb/src/filter_iterator.rs", Section::Files),
                ("website/public/img/favicon.ico", Section::Files),
                ("slatedb/tests/range_filter.rs", Section::Files),
                ("specs/fizzbee/KeyValueStore.fizz", Section::Files),
            ]
        );
        assert_eq!(found.note, None);

        // The letters that matched, in the name.
        let filter = hit(&found, "slatedb/src/filter.rs");
        assert_eq!(
            (filter.name.as_str(), filter.directory.as_str()),
            ("filter.rs", "slatedb/src")
        );
        assert_eq!(filter.name_matches, [0..2]);
        assert!(filter.directory_matches.is_empty());
        assert_eq!(hit(&found, "slatedb/src/config.rs").name_matches, [3..5]);
        assert_eq!(
            hit(&found, "slatedb/tests/prefix_filter.rs").name_matches,
            [7..9]
        );
        assert_eq!(
            hit(&found, "website/public/img/favicon.ico").name_matches,
            [0..1, 8..9]
        );
    }

    #[test]
    fn a_name_the_query_starts_is_first_and_the_shorter_of_two_such_names() {
        let files = strings(&[
            "src/windowActions.ts",
            "src/old/window.ts",
            "src/myWindow.ts",
        ]);
        assert_eq!(
            listed(&search("window", &files, &[])),
            [
                ("src/old/window.ts", Section::Files),
                ("src/windowActions.ts", Section::Files),
                ("src/myWindow.ts", Section::Files),
            ]
        );
    }

    /// A slash in the query is a directory: the whole path is matched,
    /// and the letters are marked in both parts.
    #[test]
    fn a_query_with_a_slash_matches_the_path_and_marks_both_parts() {
        let files = strings(&["lib/src/filter.rs", "lib/tests/filter_test.rs", "src.rs"]);
        let found = search("src/fil", &files, &[]);
        assert_eq!(listed(&found), [("lib/src/filter.rs", Section::Files)]);
        let filter = hit(&found, "lib/src/filter.rs");
        assert_eq!(filter.directory_matches, [4..7], "src");
        assert_eq!(filter.name_matches, [0..3], "fil");
    }

    #[test]
    fn every_word_must_match_in_any_order() {
        let files = strings(&[
            "bindings/uniffi/src/filter_policy.rs",
            "slatedb/src/filter_policy.rs",
        ]);
        let found = search("policy bindings", &files, &[]);
        assert_eq!(
            listed(&found),
            [("bindings/uniffi/src/filter_policy.rs", Section::Files)],
            "slatedb's has no bindings"
        );
        let policy = hit(&found, "bindings/uniffi/src/filter_policy.rs");
        assert_eq!(policy.name_matches, [7..13]);
        assert_eq!(policy.directory_matches, [0..8]);
    }

    #[test]
    fn a_query_in_quotes_matches_as_one_run() {
        let files = strings(&["website/public/img/favicon.ico", "slatedb/src/filter.rs"]);
        assert_eq!(listed(&search("fi", &files, &[])).len(), 2);
        assert_eq!(
            listed(&search("\"fi\"", &files, &[])),
            [("slatedb/src/filter.rs", Section::Files)],
            "favicon.ico needs a gap"
        );
    }

    /// What was open matches only as one run: `fvc` finds `favicon.ico`
    /// among the other files, and not among the recent ones, though it
    /// was open.
    #[test]
    fn a_recent_file_matches_only_as_one_run() {
        let files = strings(&["website/public/img/favicon.ico", "src/fvc.rs"]);
        let recent = strings(&["website/public/img/favicon.ico", "src/fvc.rs"]);
        assert_eq!(
            listed(&search("fvc", &files, &recent)),
            [
                ("src/fvc.rs", Section::Recent),
                ("website/public/img/favicon.ico", Section::Files)
            ]
        );
    }

    #[test]
    fn a_query_that_is_a_whole_path_puts_that_file_first() {
        let files = strings(&["src/a/mod.rs", "src/mod.rs", "mod.rs"]);
        assert_eq!(
            listed(&search("src/a/mod.rs", &files, &[]))[0],
            ("src/a/mod.rs", Section::Files)
        );
    }

    /// Before anything is typed: what was open, the most recent first,
    /// then the changes that were not, in the rail's order, each with its
    /// letter. A file gone since is not offered; one outside the worktree
    /// is, with where it is from in place of its directory.
    #[test]
    fn with_nothing_typed_the_list_is_what_was_open_then_the_changes() {
        let tokio =
            "/home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.40.0/src/lib.rs";
        let files = Files::new(strings(&[
            "src/a.rs",
            "src/b.rs",
            "src/c.rs",
            "src/gone.rs",
        ]));
        let recent = strings(&["src/b.rs", tokio, "src/removed.rs", "src/c.rs"]);
        let changes = vec![
            changed("src/a.rs", ChangeStatus::Added),
            changed("src/b.rs", ChangeStatus::Modified),
            changed("src/gone.rs", ChangeStatus::Deleted),
        ];
        let found = results(
            "",
            &Sources {
                files: &files,
                recent: &recent,
                changed: &changes,
                active: None,
            },
        );
        assert_eq!(
            listed(&found),
            [
                ("src/b.rs", Section::Recent),
                (tokio, Section::Recent),
                ("src/c.rs", Section::Recent),
                ("src/a.rs", Section::Changed),
                ("src/gone.rs", Section::Changed),
            ]
        );
        assert_eq!(hit(&found, "src/b.rs").status, Some(ChangeStatus::Modified));
        assert_eq!(hit(&found, "src/c.rs").status, None);
        assert_eq!(
            hit(&found, "src/gone.rs").status,
            Some(ChangeStatus::Deleted)
        );
        let outside = hit(&found, tokio);
        assert_eq!(
            (outside.name.as_str(), outside.directory.as_str()),
            ("lib.rs", "tokio 1.40.0")
        );
        assert_eq!(found.note, None);

        // Typed, a file outside matches on its name.
        let found = results(
            "lib",
            &Sources {
                files: &files,
                recent: &recent,
                changed: &changes,
                active: None,
            },
        );
        assert_eq!(listed(&found), [(tokio, Section::Recent)]);
        assert!(hit(&found, tokio).directory_matches.is_empty());
    }

    #[test]
    fn nothing_matching_says_so() {
        let files = strings(&["src/a.rs"]);
        let found = search("zzz", &files, &[]);
        assert!(found.picks.is_empty());
        assert_eq!(found.note.as_deref(), Some("No matching files"));
        // Nothing typed and nothing to list is no note, and no rows.
        assert_eq!(search("", &files, &[]), Results::default());
    }

    #[test]
    fn a_line_after_the_name_rides_with_the_results() {
        let files = strings(&["src/config.rs"]);
        let found = search("config:42:3", &files, &[]);
        assert_eq!(listed(&found), [("src/config.rs", Section::Files)]);
        assert_eq!(found.at, at(Some(42), Some(3)));
    }

    #[test]
    fn a_colon_alone_is_a_line_of_the_open_file() {
        let files = Files::new(strings(&["src/config.rs"]));
        let open = |query: &str, active: Option<(&str, usize)>| {
            results(
                query,
                &Sources {
                    files: &files,
                    recent: &[],
                    changed: &[],
                    active,
                },
            )
        };
        let found = open(":42", Some(("src/config.rs", 120)));
        assert_eq!(
            found.picks,
            [Pick::Line {
                path: "src/config.rs".into(),
                name: "config.rs".into(),
                line: 42,
                column: None,
            }]
        );
        assert_eq!(
            open(":", Some(("src/config.rs", 120))).note.as_deref(),
            Some("Type a line number between 1 and 120")
        );
        // Only a colon goes to a line: `#42` is looked for as a name.
        let named = open("#42", Some(("src/config.rs", 120)));
        assert!(named.picks.is_empty());
        assert_eq!(named.note.as_deref(), Some("No matching files"));
        assert_eq!(named.at, None);
        let none = open(":42", None);
        assert!(none.picks.is_empty());
        assert_eq!(
            none.note.as_deref(),
            Some("Open a file to go to a line in it")
        );
    }

    #[test]
    fn the_candidates_are_the_files_and_the_deleted_changes() {
        let tree = vec![
            TreeEntry::new("src", true),
            TreeEntry::new("src/a.rs", false),
            TreeEntry::new("README.md", false),
        ];
        let changes = vec![
            changed("src/a.rs", ChangeStatus::Modified),
            changed("src/gone.rs", ChangeStatus::Deleted),
        ];
        assert_eq!(
            candidates(&tree, &changes),
            ["src/a.rs", "README.md", "src/gone.rs"]
        );
    }

    #[test]
    fn a_run_is_cut_where_the_matches_start_and_end() {
        assert_eq!(
            cut("favicon.ico", &[0..1, 8..9]),
            [
                ("f".to_string(), true),
                ("avicon.".to_string(), false),
                ("i".to_string(), true),
                ("co".to_string(), false),
            ]
        );
        assert_eq!(cut("abc", &[]), [("abc".to_string(), false)]);
        assert!(cut("", &[]).is_empty());
    }

    #[test]
    fn the_palette_is_centred_over_the_pane_and_fits_a_narrow_one() {
        assert_eq!(placement((442.0, 1400.0), 1400.0), (621.0, 600.0));
        assert_eq!(
            placement((0.0, 500.0), 500.0),
            (GUTTER, 500.0 - 2.0 * GUTTER)
        );
        // Before the pane is laid out, over the window.
        assert_eq!(placement((0.0, 0.0), 1000.0), (200.0, 600.0));
    }

    /// Every file that becomes the active one moves to the front of its
    /// item's list, from a tab or from closing the one in front of it,
    /// and each item keeps its own.
    #[test]
    fn the_recent_files_follow_the_active_file_per_item() {
        use perch_github::Item;
        let state = ReviewState::new();
        let item = |id: &str| {
            let mut item = Item::new("slatedb/slatedb", "2026-09-22T09:00:00Z".parse().unwrap());
            item.id = id.into();
            Some(Arc::new(item))
        };
        state.item.set(item("one"));
        state.open_tab("a.rs");
        state.open_tab("b.rs");
        state.open_tab("c.rs");
        assert_eq!(state.recent(), ["c.rs", "b.rs", "a.rs"]);
        state.open_tab("a.rs");
        assert_eq!(state.recent(), ["a.rs", "c.rs", "b.rs"]);
        // Closing the active tab shows the tab after it, which is on
        // screen now; the closed one stays in the list.
        state.close_tab("a.rs");
        assert_eq!(state.active.get().as_deref(), Some("b.rs"));
        assert_eq!(state.recent(), ["b.rs", "a.rs", "c.rs"]);

        state.item.set(item("two"));
        assert!(state.recent().is_empty());
        state.open_tab("z.rs");
        assert_eq!(state.recent(), ["z.rs"]);
        state.item.set(item("one"));
        assert_eq!(state.recent(), ["b.rs", "a.rs", "c.rs"]);
    }
}

/// The palette in a whole headless window, driven by its keys.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::*;
    use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
    use perch_git::{ChangeStatus, ChangedFile, TreeEntry};
    use perch_github::{Item, ItemCheckout};

    use super::{Pick, Section};
    use crate::review::model::{ItemTab, Phase, ReviewState};
    use crate::state::AppState;
    use crate::theme;

    /// The window under Cmd-P, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = harness().0;
        focus_the_code(&mut harness);
        cmd_p(&mut harness);
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

    /// The window on Code with `src/x.rs` open, a branch that changed it
    /// and deleted `src/gone.rs`, and a few other files in the tree.
    fn harness() -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-22T09:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "wt".into();
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.tab.set(ItemTab::Code);
        state.composer_autofocus.set(false);
        state.tree.set(Arc::new(
            [
                ("docs", true),
                ("docs/guide.md", false),
                ("src", true),
                ("src/lib", true),
                ("src/lib/config.rs", false),
                ("src/x.rs", false),
                ("src/y.rs", false),
                ("README.md", false),
            ]
            .into_iter()
            .map(|(path, dir)| TreeEntry::new(path, dir))
            .collect(),
        ));
        state.set_changed_files(Arc::new(vec![
            ChangedFile {
                path: "src/x.rs".into(),
                old_path: None,
                status: ChangeStatus::Modified,
                additions: 1,
                deletions: 1,
                binary: false,
                head_blob: Some("blob-x".into()),
            },
            ChangedFile {
                path: "src/gone.rs".into(),
                old_path: None,
                status: ChangeStatus::Deleted,
                additions: 0,
                deletions: 3,
                binary: false,
                head_blob: None,
            },
        ]));
        state.open_tab("src/x.rs");
        state.finish_load(
            "src/x.rs",
            Arc::new(crate::backend::build_loaded_file(
                "src/x.rs".into(),
                "fn alpha() {}\nfn beta() {\n    alpha();\n}\n".into(),
                "fn alpha() {}\nfn old() {\n    alpha();\n}\n".into(),
                Some(ChangeStatus::Modified),
                false,
                perch_git::Scope::Branch,
                String::new(),
            )),
        );

        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        settle(&mut harness);
        (harness, state)
    }

    /// Two passes: a box laid out in the first asks for the keyboard,
    /// which the second hands it.
    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
    }

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn key(code: Code, key: Key, modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    fn press(harness: &mut HeadlessHarness, code: Code, named_key: NamedKey) {
        harness.dispatch_event(key(code, Key::Named(named_key), Modifiers::empty()));
        settle(harness);
    }

    fn cmd_p(harness: &mut HeadlessHarness) {
        harness.dispatch_event(key(
            Code::KeyP,
            Key::Character("p".into()),
            crate::ui::OS_MOD,
        ));
        settle(harness);
    }

    fn typed(harness: &mut HeadlessHarness, text: &str) {
        for ch in text.chars() {
            harness.dispatch_event(key(
                Code::Unidentified,
                Key::Character(ch.to_string()),
                Modifiers::empty(),
            ));
        }
        settle(harness);
    }

    fn listed(state: ReviewState) -> Vec<(String, Option<Section>)> {
        state.go_to.results.with(|r| {
            r.picks
                .iter()
                .map(|pick| match pick {
                    Pick::File(hit) => (hit.path.clone(), Some(hit.section)),
                    Pick::Line { line, .. } => (format!(":{line}"), None),
                })
                .collect()
        })
    }

    /// Click the code, as a reader does before reading, and hand back the
    /// editor that took the keyboard.
    fn focus_the_code(harness: &mut HeadlessHarness) -> floem::ViewId {
        let code = named(harness.root_id(), "tinted").expect("no editor in the window");
        let rect = code.get_visual_rect_no_clip();
        let (x, y) = (rect.x0 + 200.0, rect.y0 + 40.0);
        harness.click(x, y);
        settle(harness);
        let editor = harness
            .element_id_at(x, y)
            .expect("nothing under the click")
            .owning_id();
        assert!(harness.is_focused(editor));
        editor
    }

    /// The editor's caret, as a byte offset in its text, and the offset
    /// of a row's start.
    fn caret(state: ReviewState) -> (usize, impl Fn(usize) -> usize) {
        let live = state
            .docs
            .get()
            .expect("an editor is open")
            .layout
            .head()
            .clone();
        let offset = live.cursor.with(|c| c.offset());
        let doc = live.doc;
        (offset, move |row| doc.with(|d| d.offset_of_row(row)))
    }

    #[test]
    fn cmd_p_puts_the_palette_up_over_the_pane_and_escape_gives_the_keyboard_back() {
        let (mut harness, state) = harness();
        let editor = focus_the_code(&mut harness);
        assert!(named(harness.root_id(), "go-to-palette").is_none());

        cmd_p(&mut harness);
        assert!(state.go_to.open.get());
        let text_box = named(harness.root_id(), "go-to-box").expect("the palette is up");
        assert!(harness.is_focused(text_box), "the box took the keyboard");
        // What was open, then the changes that were not.
        assert_eq!(
            listed(state),
            [
                ("src/x.rs".to_string(), Some(Section::Recent)),
                ("src/gone.rs".to_string(), Some(Section::Changed)),
            ]
        );
        assert_eq!(state.go_to.selected.get(), 0);

        // Centred over the pane, under the bar over the item.
        let palette = named(harness.root_id(), "go-to-palette")
            .unwrap()
            .get_visual_rect_no_clip();
        let pane = named(harness.root_id(), "main-pane")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(
            (palette.center().x - pane.center().x).abs() < 0.5,
            "{palette:?} over {pane:?}"
        );
        assert_eq!(palette.width(), theme::GO_TO_WIDTH);
        assert_eq!(palette.y0, theme::GO_TO_TOP);

        // `a` is the composer's key from the code; in the box it is a
        // letter, and `v` does not mark the file viewed.
        typed(&mut harness, "av");
        assert_eq!(state.go_to.query.get(), "av");
        assert!(state.composer.get().is_none());
        assert!(state.viewed.with(|v| v.is_empty()));

        press(&mut harness, Code::Escape, NamedKey::Escape);
        assert!(!state.go_to.open.get());
        assert!(named(harness.root_id(), "go-to-palette").is_none());
        assert!(harness.is_focused(editor), "the code has the keyboard back");
        assert_eq!(state.active.get().as_deref(), Some("src/x.rs"));

        // Up again, the box is empty.
        cmd_p(&mut harness);
        assert_eq!(state.go_to.query.get(), "");
    }

    /// Typed from Conversation, Enter opens the file on Code, shows it
    /// in the tree, and puts it first among what was open.
    #[test]
    fn typing_narrows_the_list_and_enter_opens_the_file_on_files() {
        let (mut harness, state) = harness();
        state.show_tab(ItemTab::Conversation);
        settle(&mut harness);

        cmd_p(&mut harness);
        assert!(state.go_to.open.get(), "Cmd-P works from Conversation");
        typed(&mut harness, "conf");
        assert_eq!(
            listed(state),
            [("src/lib/config.rs".to_string(), Some(Section::Files))]
        );
        let row = named(harness.root_id(), "go-to-row:src/lib/config.rs").expect("a row for it");
        assert_eq!(
            row.get_visual_rect_no_clip().height(),
            theme::GO_TO_ROW_HEIGHT
        );

        press(&mut harness, Code::Enter, NamedKey::Enter);
        assert!(!state.go_to.open.get());
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.active.get().as_deref(), Some("src/lib/config.rs"));
        assert!(
            state.expanded.with(|e| e.contains("src/lib")),
            "shown in the tree"
        );
        assert_eq!(state.recent(), ["src/lib/config.rs", "src/x.rs"]);
    }

    /// Ctrl-N walks down. Ctrl-P walks up on a Mac, as Emacs's keys do in
    /// any list there; elsewhere it is the command key's P, and walks down.
    #[test]
    fn ctrl_n_walks_down_and_ctrl_p_up_only_where_it_is_not_the_command_key() {
        let (mut harness, state) = harness();
        cmd_p(&mut harness);
        typed(&mut harness, "s");
        let ctrl = |harness: &mut HeadlessHarness, code: Code, ch: &str| {
            harness.dispatch_event(key(code, Key::Character(ch.into()), Modifiers::CONTROL));
            settle(harness);
        };
        ctrl(&mut harness, Code::KeyN, "n");
        ctrl(&mut harness, Code::KeyN, "n");
        assert_eq!(state.go_to.selected.get(), 2);
        ctrl(&mut harness, Code::KeyP, "p");
        let after = if cfg!(target_os = "macos") { 1 } else { 3 };
        assert_eq!(state.go_to.selected.get(), after);
        assert!(state.go_to.open.get());
    }

    #[test]
    fn the_arrows_walk_the_list_and_wrap_and_cmd_p_walks_down() {
        let (mut harness, state) = harness();
        cmd_p(&mut harness);
        typed(&mut harness, "s");
        let n = state.go_to.results.with(|r| r.picks.len());
        assert_eq!(n, 5, "{:?}", listed(state));

        press(&mut harness, Code::ArrowDown, NamedKey::ArrowDown);
        assert_eq!(state.go_to.selected.get(), 1);
        press(&mut harness, Code::ArrowUp, NamedKey::ArrowUp);
        press(&mut harness, Code::ArrowUp, NamedKey::ArrowUp);
        assert_eq!(
            state.go_to.selected.get(),
            n - 1,
            "up from the first is the last"
        );
        press(&mut harness, Code::ArrowDown, NamedKey::ArrowDown);
        assert_eq!(
            state.go_to.selected.get(),
            0,
            "down from the last is the first"
        );
        cmd_p(&mut harness);
        assert!(state.go_to.open.get(), "Cmd-P again leaves it up");
        assert_eq!(state.go_to.selected.get(), 1);

        // Another letter is another list, from its first row.
        typed(&mut harness, "r");
        assert_eq!(state.go_to.query.get(), "sr");
        assert_eq!(state.go_to.selected.get(), 0);
        assert_eq!(listed(state).len(), 4, "{:?}", listed(state));

        let picked = listed(state)[1].0.clone();
        press(&mut harness, Code::ArrowDown, NamedKey::ArrowDown);
        press(&mut harness, Code::Enter, NamedKey::Enter);
        assert_eq!(state.active.get(), Some(picked));
    }

    /// `x.rs:3` lands on the third line of the file on screen, which
    /// stays, with the keyboard back in it.
    #[test]
    fn a_line_after_the_name_puts_the_caret_on_it() {
        let (mut harness, state) = harness();
        let editor = focus_the_code(&mut harness);
        cmd_p(&mut harness);
        typed(&mut harness, "x.rs:3");
        assert_eq!(listed(state)[0].0, "src/x.rs");
        press(&mut harness, Code::Enter, NamedKey::Enter);
        assert!(!state.go_to.open.get());
        assert_eq!(state.active.get().as_deref(), Some("src/x.rs"));
        // The head's third line is the display's fourth: the removed line
        // is spliced in above it.
        let (offset, row) = caret(state);
        assert_eq!(offset, row(3));
        assert!(
            harness.is_focused(editor),
            "the same editor, with the keyboard"
        );
        // A jump the history remembers leaving from: the top of the file.
        let left = state.history.with(|h| h.clone()).back(None).map(|p| p.spot);
        assert_eq!(
            left,
            Some(crate::review::model::Spot::Line {
                path: "src/x.rs".into(),
                line: 0,
            })
        );
    }

    #[test]
    fn a_colon_alone_goes_to_a_line_of_the_open_file() {
        let (mut harness, state) = harness();
        cmd_p(&mut harness);
        typed(&mut harness, ":");
        assert!(listed(state).is_empty());
        assert_eq!(
            state.go_to.results.with(|r| r.note.clone()).as_deref(),
            Some("Type a line number between 1 and 4")
        );
        assert!(named(harness.root_id(), "go-to-note").is_some());
        typed(&mut harness, "2");
        assert_eq!(listed(state), [(":2".to_string(), None)]);
        press(&mut harness, Code::Enter, NamedKey::Enter);
        let (offset, row) = caret(state);
        assert_eq!(offset, row(2), "fn beta, under the removed line");
    }

    #[test]
    fn a_press_outside_puts_it_away_and_a_click_on_a_row_opens_it() {
        let (mut harness, state) = harness();
        cmd_p(&mut harness);
        harness.click(700.0, 850.0);
        settle(&mut harness);
        assert!(!state.go_to.open.get(), "a press outside put it away");
        assert_eq!(state.active.get().as_deref(), Some("src/x.rs"));

        cmd_p(&mut harness);
        // A press on the palette itself, beside the box, keeps it up and
        // the keyboard in the box.
        let palette = named(harness.root_id(), "go-to-palette")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.click(palette.x0 + 3.0, palette.y0 + 3.0);
        settle(&mut harness);
        assert!(state.go_to.open.get());
        assert!(harness.is_focused(named(harness.root_id(), "go-to-box").unwrap()));

        let row = named(harness.root_id(), "go-to-row:src/gone.rs")
            .expect("the deleted change is listed")
            .get_visual_rect_no_clip();
        harness.click(row.center().x, row.center().y);
        settle(&mut harness);
        assert!(!state.go_to.open.get());
        assert_eq!(state.active.get().as_deref(), Some("src/gone.rs"));
    }

    #[test]
    fn cmd_p_does_nothing_without_files() {
        let (mut harness, state) = harness();
        state.item.update(|item| {
            let item = Arc::make_mut(item.as_mut().unwrap());
            item.checkout = None;
        });
        state.set_phase(Phase::Issue);
        settle(&mut harness);
        cmd_p(&mut harness);
        assert!(!state.go_to.open.get());
    }

    #[test]
    fn leaving_the_item_puts_the_palette_away() {
        let (mut harness, state) = harness();
        cmd_p(&mut harness);
        assert!(state.go_to.open.get());
        state.close();
        settle(&mut harness);
        assert!(!state.go_to.open.get());
        assert!(named(harness.root_id(), "go-to-palette").is_none());
    }
}
