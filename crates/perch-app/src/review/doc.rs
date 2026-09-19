//! The document the code pane draws.
//!
//! The editor draws a text. This is that text and a table with one entry per
//! line of it saying what the line is: a display line of the buffer, a
//! filler where the other side of a split has a line and this one does not,
//! a divider standing for a run of unchanged lines hidden behind it, or a
//! spacer reserved under a line for a thread's card. The gutter, the
//! styling, the cursor and the jumps all speak the document's rows and
//! translate through it, so folding and cards are one transform over the
//! buffer here and nothing else in the pane learns about them.
//!
//! A document is over one `DisplayBuffer`: the unified buffer for the
//! stacked layout and the head side of a split, and the base buffer for the
//! base side, whose display lines are its own lines. Every builder is pure.

use std::collections::BTreeSet;

use perch_git::{DisplayBuffer, LineStatus, Row};

use crate::review::hover;

/// How many lines of context stay in view on each side of a change or an
/// anchor. A run that is not longer than twice this is shown whole: a
/// divider hiding two lines is worse than the two lines.
pub const CONTEXT_LINES: usize = 3;

/// How many lines a divider's Up or Down reveals at one end of its run.
pub const REVEAL_STEP: usize = 20;

/// A run of this many lines or fewer offers All instead of Up and Down,
/// since two clicks would do it anyway.
pub const REVEAL_ALL_AT: usize = 40;

/// How a line is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tint {
    Plain,
    Added,
    Modified,
    Deleted,
}

/// Which thread a card is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(not(test), expect(dead_code, reason = "the cards are drawn from step 5 of milestone 11"))]
pub enum CardId {
    /// The reader's own, by thread id.
    Thread(String),
    /// One of GitHub's review comment threads, by its first comment's id.
    GitHub(u64),
    /// The one being written.
    Composer,
}

/// A card's place in the document: the display line it sits under, and how
/// many rows it takes there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub id: CardId,
    /// The display line the card is under. A card under a line whose last
    /// ghost is spliced under it goes after the ghosts, so this is the last
    /// of those.
    pub after_display_line: usize,
    pub rows: usize,
}

/// One row of the document. Hashed, because the gutter keys its rows on
/// what they are as well as where, so a row that is the same thing after
/// an edit keeps its view and one that is not is rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocRow {
    /// A display line of the buffer the document is over.
    Line { display: usize, tint: Tint },
    /// Split only: the other side has a line here and this one does not.
    Filler,
    /// A run of unchanged lines hidden behind this row, by index into the
    /// folds the document was built with.
    Divider { run: usize },
    /// One of the rows reserved under a line for a card; `row` is which of
    /// the card's this is.
    Spacer { card: CardId, row: usize },
}

/// The text the editor lays out and what each line of it is.
///
/// The tables from a display line and a head line to a row are built once,
/// here, so a lookup is not a scan of the rows: a jump on a twelve thousand
/// line file asks for one, and the gutter asks per visible row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub text: String,
    pub rows: Vec<DocRow>,
    /// The folds the document was built with, which a divider row indexes.
    pub folds: Vec<Run>,
    /// The head line each row is, for the rows that are one. A ghost, a
    /// filler, a divider and a spacer are none.
    heads: Vec<Option<usize>>,
    /// The byte offset each row's text starts at.
    starts: Vec<usize>,
    /// The row each display line of the buffer is on, by display line;
    /// `None` inside a fold.
    display_rows: Vec<Option<usize>>,
    /// The row each head line is on, by head line; `None` inside a fold,
    /// and for every line of a base side, which has no head lines.
    head_rows: Vec<Option<usize>>,
}

impl Doc {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The text of one row, without its newline.
    pub fn line(&self, row: usize) -> &str {
        let start = self.offset_of_row(row);
        let end = self.offset_of_row(row + 1);
        self.text[start..end].trim_end_matches('\n')
    }

    /// The byte offset a row's text starts at; the text's length for the
    /// row past the last, so a row's bytes are from its start to the next.
    pub fn offset_of_row(&self, row: usize) -> usize {
        self.starts.get(row).copied().unwrap_or(self.text.len())
    }

    /// The run a divider row stands for, if the row is one.
    pub fn run_of(&self, row: usize) -> Option<Run> {
        match self.rows.get(row)? {
            DocRow::Divider { run } => self.folds.get(*run).copied(),
            _ => None,
        }
    }

    /// How many lines the dividers hide between them.
    pub fn hidden_lines(&self) -> usize {
        self.folds.iter().map(Run::len).sum()
    }

    /// Whether a row is the same thing, with the same text, as a row of
    /// another document. A divider is the same when the run behind it is,
    /// wherever it sits in either document's folds.
    fn same_row(&self, row: usize, other: &Doc, other_row: usize) -> bool {
        if self.line(row) != other.line(other_row) {
            return false;
        }
        match (&self.rows[row], &other.rows[other_row]) {
            (DocRow::Divider { run: a }, DocRow::Divider { run: b }) => self.folds.get(*a) == other.folds.get(*b),
            (a, b) => a == b,
        }
    }

    /// The display line a row is. `None` for a filler, a divider or a
    /// spacer, which is no line of anything.
    pub fn display_line(&self, row: usize) -> Option<usize> {
        match self.rows.get(row)? {
            DocRow::Line { display, .. } => Some(*display),
            _ => None,
        }
    }

    /// The row a display line is on. `None` when the line is folded away.
    #[cfg_attr(not(test), expect(dead_code, reason = "the jump into a fold, step 2 of milestone 11, looks rows up by display line"))]
    pub fn row_of_display(&self, display: usize) -> Option<usize> {
        self.display_rows.get(display).copied().flatten()
    }

    /// The head line a row is. `None` for a ghost, and for every row that
    /// is no line.
    pub fn head_line(&self, row: usize) -> Option<usize> {
        self.heads.get(row).copied().flatten()
    }

    /// The row a head line is on. `None` when the line is folded away.
    pub fn row_of_head(&self, head_line: usize) -> Option<usize> {
        self.head_rows.get(head_line).copied().flatten()
    }

    /// Where a byte offset in the text lands in the head file, as line and
    /// column, if it lands in it at all: a ghost, a filler, a divider or a
    /// spacer is nowhere.
    pub fn head_position(&self, offset: usize) -> Option<(usize, usize)> {
        let (row, column) = hover::line_col_of_offset(&self.text, offset);
        Some((self.head_line(row)?, column))
    }

    /// The display line nearest a row, looking down the file first, for a
    /// cursor that lands on a row that is no line.
    pub fn nearest_display_line(&self, row: usize) -> Option<usize> {
        (row..self.len())
            .find_map(|r| self.display_line(r))
            .or_else(|| (0..row.min(self.len())).rev().find_map(|r| self.display_line(r)))
    }
}

/// A document under construction: the lines joined as the editor wants
/// them, with the head line of each row noted for the tables.
struct Builder {
    text: String,
    rows: Vec<DocRow>,
    folds: Vec<Run>,
    heads: Vec<Option<usize>>,
    starts: Vec<usize>,
}

impl Builder {
    fn with_capacity(rows: usize, folds: &[Run]) -> Self {
        Self {
            text: String::new(),
            rows: Vec::with_capacity(rows),
            folds: folds.to_vec(),
            heads: Vec::with_capacity(rows),
            starts: Vec::with_capacity(rows),
        }
    }

    fn push(&mut self, row: DocRow, line: &str, head: Option<usize>) {
        if !self.rows.is_empty() {
            self.text.push('\n');
        }
        self.starts.push(self.text.len());
        self.text.push_str(line);
        self.rows.push(row);
        self.heads.push(head);
    }

    /// The rows a card takes, under the line just pushed.
    fn push_cards(&mut self, cards: &[Card], display: usize) {
        for card in cards.iter().filter(|c| c.after_display_line == display) {
            for row in 0..card.rows {
                self.push(DocRow::Spacer { card: card.id.clone(), row }, "", None);
            }
        }
    }

    /// Ends the text in a newline, as the buffer's does, and builds the
    /// tables.
    fn finish(mut self) -> Doc {
        if !self.rows.is_empty() {
            self.text.push('\n');
        }
        let mut display_rows = Vec::new();
        for (row, entry) in self.rows.iter().enumerate() {
            if let DocRow::Line { display, .. } = entry {
                if *display >= display_rows.len() {
                    display_rows.resize(display + 1, None);
                }
                display_rows[*display] = Some(row);
            }
        }
        let mut head_rows = Vec::new();
        for (row, head) in self.heads.iter().enumerate() {
            if let Some(head) = head {
                if *head >= head_rows.len() {
                    head_rows.resize(head + 1, None);
                }
                head_rows[*head] = Some(row);
            }
        }
        Doc {
            text: self.text,
            rows: self.rows,
            folds: self.folds,
            heads: self.heads,
            starts: self.starts,
            display_rows,
            head_rows,
        }
    }
}

/// The tint of a row of the unified buffer.
fn tint_of(row: Row) -> Tint {
    match row {
        Row::Deleted { .. } => Tint::Deleted,
        Row::Head { status: LineStatus::Added, .. } => Tint::Added,
        Row::Head { status: LineStatus::Modified, .. } => Tint::Modified,
        Row::Head { status: LineStatus::Unchanged, .. } => Tint::Plain,
    }
}

/// The display line of each head line, by head line. The buffer keeps the
/// head's lines in order, so the table is the head rows' positions.
fn display_of_heads(buffer: &DisplayBuffer) -> Vec<usize> {
    buffer.rows.iter().enumerate().filter_map(|(display, row)| row.head_line().map(|_| display)).collect()
}

/// The document of the stacked layout: the unified buffer with each fold
/// in `folds` replaced by a divider and each card's rows reserved under its
/// line. The folds are in display lines, in order and apart; a card under
/// a folded line is not placed, since its line is not shown.
pub fn build(buffer: &DisplayBuffer, folds: &[Run], cards: &[Card]) -> Doc {
    let lines: Vec<&str> = buffer.text.lines().collect();
    let reserved: usize = cards.iter().map(|c| c.rows).sum();
    let mut out = Builder::with_capacity(buffer.len() + reserved, folds);
    let mut next_fold = 0;

    let mut display = 0;
    while display < buffer.len() {
        if let Some(run) = folds.get(next_fold).filter(|r| r.first == display) {
            out.push(DocRow::Divider { run: next_fold }, "", None);
            next_fold += 1;
            display = run.last + 1;
            continue;
        }
        let row = buffer.rows[display];
        out.push(
            DocRow::Line { display, tint: tint_of(row) },
            lines.get(display).copied().unwrap_or(""),
            row.head_line(),
        );
        out.push_cards(cards, display);
        display += 1;
    }
    out.finish()
}

/// The two documents of a split, laid out row for row along `alignment`,
/// whose rows are one-based base and head lines with `None` where a side
/// has nothing. A divider or a spacer goes into both at the same row, so
/// the two stay the same height.
///
/// The left is the old file, so a line of it that the head replaced or
/// dropped is tinted as removed, whichever it was; the right carries the
/// head's own status, so a rewritten line and a new one keep the colours
/// the stacked layout gives them. The head's text and statuses are read
/// off the unified buffer, which also says where each head line is
/// displayed: the folds and the cards are in the buffer's display lines,
/// and so is the right side's `display`.
pub fn build_split(
    alignment: &[(Option<usize>, Option<usize>)],
    base: &str,
    buffer: &DisplayBuffer,
    folds: &[Run],
    cards: &[Card],
) -> (Doc, Doc) {
    let base_lines: Vec<&str> = base.lines().collect();
    let head_lines: Vec<&str> = buffer.text.lines().collect();
    let display_of_head = display_of_heads(buffer);
    // The display line of each ghost, by base line, so a card under a ghost
    // finds the row the ghost's base line is on.
    let mut display_of_ghost = std::collections::HashMap::new();
    for (display, row) in buffer.rows.iter().enumerate() {
        if let Row::Deleted { base_line } = row {
            display_of_ghost.insert(*base_line, display);
        }
    }

    let reserved: usize = cards.iter().map(|c| c.rows).sum();
    let mut left = Builder::with_capacity(alignment.len() + reserved, folds);
    let mut right = Builder::with_capacity(alignment.len() + reserved, folds);
    let mut next_fold = 0;
    // The last display line of the fold being skipped, while one is.
    let mut skip_to: Option<usize> = None;

    for &(base_line, head_line) in alignment {
        let base_line = base_line.and_then(|n| n.checked_sub(1));
        let head_line = head_line.and_then(|n| n.checked_sub(1));
        let display = head_line.and_then(|h| display_of_head.get(h).copied());

        if let Some(display) = display {
            if skip_to.is_some_and(|last| display <= last) {
                continue;
            }
            if let Some(run) = folds.get(next_fold).filter(|r| r.first == display) {
                left.push(DocRow::Divider { run: next_fold }, "", None);
                right.push(DocRow::Divider { run: next_fold }, "", None);
                next_fold += 1;
                skip_to = Some(run.last);
                continue;
            }
        }

        let head_status = display.and_then(|d| buffer.row(d)).and_then(|r| r.status());
        match base_line {
            None => left.push(DocRow::Filler, "", None),
            Some(b) => {
                let tint = match head_status {
                    None | Some(LineStatus::Modified) => Tint::Deleted,
                    Some(_) => Tint::Plain,
                };
                left.push(DocRow::Line { display: b, tint }, base_lines.get(b).copied().unwrap_or(""), None);
            }
        }
        match (display, head_status) {
            (Some(d), Some(status)) => {
                let tint = match status {
                    LineStatus::Added => Tint::Added,
                    LineStatus::Modified => Tint::Modified,
                    LineStatus::Unchanged => Tint::Plain,
                };
                right.push(DocRow::Line { display: d, tint }, head_lines.get(d).copied().unwrap_or(""), head_line);
            }
            _ => right.push(DocRow::Filler, "", None),
        }

        // A card under this row's head line, or under the ghost this row's
        // base line is in the unified buffer, takes its rows on both sides.
        let under = display.or_else(|| base_line.and_then(|b| display_of_ghost.get(&b).copied()));
        if let Some(under) = under {
            left.push_cards(cards, under);
            right.push_cards(cards, under);
        }
    }

    (left.finish(), right.finish())
}

// ---------------------------------------------------------------------------
// Folds
// ---------------------------------------------------------------------------

/// A run of unchanged display lines a divider stands for. The run has no
/// ghost in it, so its head lines are as consecutive as its display lines,
/// and `first_head` places it in the head file for the reveals, which are
/// in head lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Run {
    /// First and last display line hidden, inclusive.
    pub first: usize,
    pub last: usize,
    /// The head line `first` is.
    pub first_head: usize,
    /// The head lines of the run as the folds have it by default, which
    /// this is a piece of once the reveals have cut it: what Reset folds
    /// back to, and what a reveal has to overlap for Reset to be offered,
    /// so that the divider above a run revealed in the middle offers it.
    pub default_heads: (usize, usize),
}

impl Run {
    /// A run of the default folds, which is its own default.
    pub fn new(first: usize, last: usize, first_head: usize) -> Self {
        let last_head = first_head + (last - first);
        Self { first, last, first_head, default_heads: (first_head, last_head) }
    }

    /// The head line `last` is.
    pub fn last_head(&self) -> usize {
        self.first_head + (self.last - self.first)
    }

    /// How many lines the run hides.
    pub fn len(&self) -> usize {
        self.last - self.first + 1
    }

    /// Whether the run's divider offers All rather than Up and Down.
    pub fn reveals_whole(&self) -> bool {
        self.len() <= REVEAL_ALL_AT
    }

    /// The head lines Up reveals: the first `REVEAL_STEP` of the run.
    pub fn top(&self) -> (usize, usize) {
        (self.first_head, (self.first_head + REVEAL_STEP - 1).min(self.last_head()))
    }

    /// The head lines Down reveals: the last `REVEAL_STEP` of the run.
    pub fn bottom(&self) -> (usize, usize) {
        ((self.last_head() + 1).saturating_sub(REVEAL_STEP).max(self.first_head), self.last_head())
    }

    /// The head lines All reveals: every one.
    pub fn whole(&self) -> (usize, usize) {
        (self.first_head, self.last_head())
    }
}

/// What a divider says: `47 unchanged lines`.
pub fn divider_label(hidden: usize) -> String {
    if hidden == 1 {
        "1 unchanged line".to_string()
    } else {
        format!("{hidden} unchanged lines")
    }
}

/// The default folds of a buffer: every run of unchanged display lines
/// longer than twice the context, less the context at each end that
/// touches something. The run at the top of the file keeps its context
/// before the first change and folds from line one; the run at the bottom
/// keeps its context after the last and folds to the end. A buffer that is
/// one run has nothing to fold around and gives none.
///
/// What a run is folded around is a change, a ghost, or a line in
/// `anchors`, which are inclusive head line ranges, zero based: the
/// reader's threads from their first line to their last, and GitHub's
/// review comments. A line an anchor sits on is never folded.
pub fn runs(buffer: &DisplayBuffer, anchors: &[(usize, usize)]) -> Vec<Run> {
    let len = buffer.len();
    let display_of_head = display_of_heads(buffer);
    let mut anchored = vec![false; len];
    for &(first, last) in anchors {
        for head in first..=last.max(first) {
            if let Some(&display) = display_of_head.get(head) {
                anchored[display] = true;
            }
        }
    }
    let foldable = |display: usize| {
        buffer.rows[display].status() == Some(LineStatus::Unchanged) && !anchored[display]
    };

    let mut out = Vec::new();
    let mut display = 0;
    while display < len {
        if !foldable(display) {
            display += 1;
            continue;
        }
        let start = display;
        while display < len && foldable(display) {
            display += 1;
        }
        let end = display - 1;

        let at_top = start == 0;
        let at_bottom = end + 1 == len;
        if at_top && at_bottom {
            continue;
        }
        // Not longer than twice the context: shown whole.
        if end - start < 2 * CONTEXT_LINES {
            continue;
        }
        let first = if at_top { start } else { start + CONTEXT_LINES };
        let last = if at_bottom { end } else { end - CONTEXT_LINES };
        if first > last {
            continue;
        }
        let Some(first_head) = buffer.head_line(first) else { continue };
        out.push(Run::new(first, last, first_head));
    }
    out
}

/// The head line ranges the reader asked to see, inclusive and zero based.
/// Kept as they were added rather than merged, so the set here is the set
/// the store has, and a reset removes the same rows from both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reveals(BTreeSet<(usize, usize)>);

impl Reveals {
    pub fn add(&mut self, first: usize, last: usize) {
        self.0.insert((first.min(last), first.max(last)));
    }

    /// Forget every reveal that overlaps a range: a divider's Reset.
    pub fn remove_overlapping(&mut self, first: usize, last: usize) {
        self.0.retain(|&(a, b)| b < first || a > last);
    }

    /// Whether any reveal overlaps a range: whether a divider offers Reset.
    pub fn overlaps(&self, first: usize, last: usize) -> bool {
        self.0.iter().any(|&(a, b)| a <= last && b >= first)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "the reveals are written to the store from step 4 of milestone 11"))]
    pub fn iter(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.0.iter().copied()
    }

    /// The runs with the reveals cut out of them: a run a reveal overlaps
    /// is cut back to what the reveal leaves, on either side of it, and a
    /// run a reveal covers is gone.
    pub fn apply(&self, runs: &[Run]) -> Vec<Run> {
        let mut out = Vec::with_capacity(runs.len());
        for run in runs {
            let mut pieces = vec![(run.first_head, run.last_head())];
            for &(a, b) in &self.0 {
                pieces = pieces
                    .into_iter()
                    .flat_map(|(p0, p1)| {
                        if b < p0 || a > p1 {
                            return vec![(p0, p1)];
                        }
                        let mut left = Vec::with_capacity(2);
                        if a > p0 {
                            left.push((p0, a - 1));
                        }
                        if b < p1 {
                            left.push((b + 1, p1));
                        }
                        left
                    })
                    .collect();
            }
            for (p0, p1) in pieces {
                out.push(Run {
                    first: run.first + (p0 - run.first_head),
                    last: run.first + (p1 - run.first_head),
                    first_head: p0,
                    default_heads: run.default_heads,
                });
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// The edit from one document to the next
// ---------------------------------------------------------------------------

/// What changes between one document and the next, as one region: the
/// rows after the longest common prefix and before the longest common
/// suffix, in both, and the same as bytes of each text, whole rows with
/// their newlines. Replacing the old bytes with the new is the one edit
/// that turns the old text into the new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub old_rows: std::ops::Range<usize>,
    pub new_rows: std::ops::Range<usize>,
    pub old_bytes: std::ops::Range<usize>,
    pub new_bytes: std::ops::Range<usize>,
}

impl Region {
    /// How many rows the edit adds, negative when it takes some away.
    pub fn row_delta(&self) -> isize {
        self.new_rows.len() as isize - self.old_rows.len() as isize
    }

    /// How many bytes the edit adds, negative when it takes some away.
    pub fn byte_delta(&self) -> isize {
        self.new_bytes.len() as isize - self.old_bytes.len() as isize
    }
}

/// The region that turns `old` into `new`, or `None` when the two read the
/// same. Rows are compared as what they are and what they say, so a
/// divider that moved is not taken for the line now in its place, and a
/// divider whose run is the same is the same row wherever the run sits
/// in the folds.
pub fn changed_region(old: &Doc, new: &Doc) -> Option<Region> {
    let shorter = old.len().min(new.len());
    let mut prefix = 0;
    while prefix < shorter && old.same_row(prefix, new, prefix) {
        prefix += 1;
    }
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let mut suffix = 0;
    while suffix < shorter - prefix && old.same_row(old.len() - 1 - suffix, new, new.len() - 1 - suffix) {
        suffix += 1;
    }
    let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
    Some(Region {
        old_rows: prefix..old_end,
        new_rows: prefix..new_end,
        old_bytes: old.offset_of_row(prefix)..old.offset_of_row(old_end),
        new_bytes: new.offset_of_row(prefix)..new.offset_of_row(new_end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_git::file_diff;

    fn line(display: usize, tint: Tint) -> DocRow {
        DocRow::Line { display, tint }
    }

    /// The unified buffer for a base and a head.
    fn buffer(base: &str, head: &str) -> DisplayBuffer {
        DisplayBuffer::unified(head, base, &file_diff(base, head))
    }

    /// Both sides of a split for a base and a head, as the view builds them.
    fn split(base: &str, head: &str) -> (Doc, Doc) {
        let diff = file_diff(base, head);
        let alignment = perch_git::alignment(&diff, base, head);
        let buffer = DisplayBuffer::unified(head, base, &diff);
        build_split(&alignment, base, &buffer, &[], &[])
    }

    fn lines(doc: &Doc) -> Vec<&str> {
        doc.text.lines().collect()
    }

    /// A head of `before` unchanged lines, then a changed one, then `after`
    /// unchanged lines, with the base the same but for the changed line.
    fn one_change(before: usize, after: usize) -> DisplayBuffer {
        let context = |from: usize, n: usize| (from..from + n).map(|i| format!("line {i}\n")).collect::<String>();
        let base = format!("{}old\n{}", context(0, before), context(before, after));
        let head = format!("{}new\n{}", context(0, before), context(before, after));
        buffer(&base, &head)
    }

    /// A changed first line, `between` unchanged lines, and a changed last
    /// line, so the only run is the one between the two.
    fn two_changes(between: usize) -> DisplayBuffer {
        let context: String = (0..between).map(|i| format!("line {i}\n")).collect();
        let base = format!("old top\n{context}old bottom\n");
        let head = format!("new top\n{context}new bottom\n");
        buffer(&base, &head)
    }

    // The stacked document over the buffer as it is.

    #[test]
    fn the_stacked_document_is_the_buffer_with_a_tint_per_row() {
        // a, ghost(b), B, c, ghost(d).
        let b = buffer("a\nb\nc\nd\n", "a\nB\nc\n");
        let doc = build(&b, &[], &[]);
        assert_eq!(doc.text, b.text);
        assert_eq!(
            doc.rows,
            vec![line(0, Tint::Plain), line(1, Tint::Deleted), line(2, Tint::Modified), line(3, Tint::Plain), line(4, Tint::Deleted)]
        );
        for display in 0..b.len() {
            assert_eq!(doc.display_line(display), Some(display));
            assert_eq!(doc.row_of_display(display), Some(display));
            assert_eq!(doc.head_line(display), b.head_line(display));
        }
        for head in 0..3 {
            assert_eq!(doc.row_of_head(head), b.display_line(head), "head line {head}");
        }
        assert_eq!(doc.row_of_head(3), None, "no fourth head line");
    }

    #[test]
    fn a_click_on_a_real_line_maps_to_the_head_file_and_a_ghost_to_nothing() {
        // Display: a, ghost(b), B, c. Offset 0 is the first head line; the
        // ghost is nowhere; the line after the ghost is head line 1, not
        // display line 2.
        let doc = build(&buffer("a\nb\nc\n", "a\nB\nc\n"), &[], &[]);
        assert_eq!(doc.head_position(0), Some((0, 0)));
        let ghost: usize = doc.text.lines().take(1).map(|l| l.len() + 1).sum();
        assert_eq!(doc.head_position(ghost), None, "a deleted line has no position in the head file");
        let after: usize = doc.text.lines().take(2).map(|l| l.len() + 1).sum();
        assert_eq!(doc.head_position(after), Some((1, 0)));
        assert_eq!(doc.head_position(after + 1), Some((1, 1)), "columns are byte offsets within the line");
    }

    #[test]
    fn an_empty_buffer_is_an_empty_document() {
        let doc = build(&DisplayBuffer::plain(""), &[], &[]);
        assert_eq!(doc.len(), 0);
        assert_eq!(doc.text, "");
        assert_eq!(doc.nearest_display_line(0), None);
        assert_eq!(doc.head_position(0), None);
    }

    // The split.

    #[test]
    fn the_two_sides_have_the_same_rows_and_an_unchanged_file_is_untinted() {
        let (left, right) = split("a\nb\nc\n", "a\nb\nc\n");
        assert_eq!(lines(&left), lines(&right));
        assert_eq!(left.rows, vec![line(0, Tint::Plain), line(1, Tint::Plain), line(2, Tint::Plain)]);
        assert_eq!(right.rows, left.rows);
    }

    #[test]
    fn an_added_line_leaves_a_filler_on_the_left() {
        let (left, right) = split("a\nb\n", "a\nx\nb\n");
        assert_eq!(lines(&left), vec!["a", "", "b"]);
        assert_eq!(lines(&right), vec!["a", "x", "b"]);
        assert_eq!(left.rows, vec![line(0, Tint::Plain), DocRow::Filler, line(1, Tint::Plain)]);
        assert_eq!(right.rows, vec![line(0, Tint::Plain), line(1, Tint::Added), line(2, Tint::Plain)]);
        assert_eq!(left.display_line(1), None, "a filler is no line of the base");
        assert_eq!(left.display_line(2), Some(1), "the base's second line is on the third row");
    }

    #[test]
    fn a_removed_line_leaves_a_filler_on_the_right() {
        let (left, right) = split("a\nb\nc\n", "a\nc\n");
        assert_eq!(lines(&left), vec!["a", "b", "c"]);
        assert_eq!(lines(&right), vec!["a", "", "c"]);
        assert_eq!(left.rows, vec![line(0, Tint::Plain), line(1, Tint::Deleted), line(2, Tint::Plain)]);
        // The right's lines are the unified buffer's display lines, which
        // has the ghost of b at display line 1: c is display line 2.
        assert_eq!(right.rows, vec![line(0, Tint::Plain), DocRow::Filler, line(2, Tint::Plain)]);
        assert_eq!(right.head_line(2), Some(1));
        assert_eq!(right.row_of_head(1), Some(2));
    }

    #[test]
    fn a_rewritten_line_is_old_on_the_left_and_new_on_the_right() {
        let (left, right) = split("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(lines(&left), vec!["a", "b", "c"]);
        assert_eq!(lines(&right), vec!["a", "B", "c"]);
        assert_eq!(left.rows[1], line(1, Tint::Deleted));
        assert_eq!(right.rows[1], line(2, Tint::Modified));
    }

    #[test]
    fn a_modification_that_grew_pairs_what_it_can_and_fills_the_rest() {
        // One line became three: the first is beside the old one, the other
        // two sit against fillers.
        let (left, right) = split("a\nb\nc\n", "a\nB\nB2\nB3\nc\n");
        assert_eq!(lines(&left), vec!["a", "b", "", "", "c"]);
        assert_eq!(lines(&right), vec!["a", "B", "B2", "B3", "c"]);
        assert_eq!(
            left.rows,
            vec![line(0, Tint::Plain), line(1, Tint::Deleted), DocRow::Filler, DocRow::Filler, line(2, Tint::Plain)]
        );
        assert_eq!(
            right.rows,
            vec![line(0, Tint::Plain), line(2, Tint::Modified), line(3, Tint::Modified), line(4, Tint::Modified), line(5, Tint::Plain)]
        );
    }

    #[test]
    fn every_line_of_each_side_appears_once_in_order() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let head = "one\n2\nthree\nfour and a half\nfive\nsix\n";
        let (left, right) = split(base, head);
        let left_text: Vec<&str> = lines(&left).into_iter().filter(|l| !l.is_empty()).collect();
        let right_text: Vec<&str> = lines(&right).into_iter().filter(|l| !l.is_empty()).collect();
        assert_eq!(left_text, base.lines().collect::<Vec<_>>());
        assert_eq!(right_text, head.lines().collect::<Vec<_>>());
        let base_lines: Vec<usize> = (0..left.len()).filter_map(|r| left.display_line(r)).collect();
        assert_eq!(base_lines, (0..5).collect::<Vec<_>>());
        let head_lines: Vec<usize> = (0..right.len()).filter_map(|r| right.head_line(r)).collect();
        assert_eq!(head_lines, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn the_documents_end_with_a_newline_like_the_unified_buffer() {
        let (left, right) = split("a\n", "a\nb\n");
        assert_eq!(left.text, "a\n\n");
        assert_eq!(right.text, "a\nb\n");
        assert_eq!(left.text.lines().count(), left.len());
        assert_eq!(right.text.lines().count(), right.len());
    }

    #[test]
    fn the_head_side_maps_rows_back_to_display_lines() {
        // base a b c d; head a B c: b rewritten, d removed. The unified
        // buffer is a, ghost(b), B, c, ghost(d): head line 1 is display
        // line 2, and the split's fourth row is a filler.
        let (_, right) = split("a\nb\nc\nd\n", "a\nB\nc\n");
        assert_eq!(right.display_line(1), Some(2));
        assert_eq!(right.display_line(3), None);
        assert_eq!(right.nearest_display_line(3), Some(3), "the row above, c, when nothing follows");
        assert_eq!(right.row_of_head(2), Some(2));
        assert_eq!(right.row_of_display(2), Some(1));
        assert_eq!(right.row_of_display(1), None, "the ghost is on no row of the head side");
        // A click on the filler row is nowhere; on B it is head line 1.
        let filler: usize = right.text.lines().take(3).map(|l| l.len() + 1).sum();
        assert_eq!(right.head_position(filler), None);
        assert_eq!(right.head_position(2), Some((1, 0)));
    }

    // Dividers and spacers in the built documents.

    #[test]
    fn a_fold_is_one_divider_row_and_its_lines_are_on_no_row() {
        let b = two_changes(20);
        // Hide display lines 5 to 10.
        let fold = Run::new(5, 10, 4);
        let doc = build(&b, &[fold], &[]);
        assert_eq!(doc.len(), b.len() - 6 + 1);
        assert_eq!(doc.rows[5], DocRow::Divider { run: 0 });
        assert_eq!(doc.text.lines().nth(5), Some(""), "the divider's row is blank text");
        assert_eq!(doc.rows[6], line(11, Tint::Plain));
        assert_eq!(doc.row_of_display(7), None);
        assert_eq!(doc.row_of_display(11), Some(6));
        assert_eq!(doc.row_of_head(10), Some(6), "head line 10 is display line 11");
        assert_eq!(doc.display_line(5), None);
        assert_eq!(doc.nearest_display_line(5), Some(11));
        assert_eq!(doc.head_line(5), None);
    }

    #[test]
    fn a_card_reserves_its_rows_under_its_line() {
        let b = two_changes(4);
        let card = Card { id: CardId::Thread("t1".into()), after_display_line: 2, rows: 3 };
        let doc = build(&b, &[], std::slice::from_ref(&card));
        assert_eq!(doc.len(), b.len() + 3);
        assert_eq!(doc.rows[2], line(2, Tint::Plain));
        for row in 0..3 {
            assert_eq!(doc.rows[3 + row], DocRow::Spacer { card: card.id.clone(), row });
            assert_eq!(doc.text.lines().nth(3 + row), Some(""));
        }
        assert_eq!(doc.rows[6], line(3, Tint::Plain));
        assert_eq!(doc.row_of_display(3), Some(6));
        assert_eq!(doc.nearest_display_line(4), Some(3));
    }

    #[test]
    fn the_split_puts_a_divider_and_a_spacer_on_both_sides_at_the_same_row() {
        let base = "old\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nold end\n";
        let head = "new\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nnew end\n";
        let diff = file_diff(base, head);
        let alignment = perch_git::alignment(&diff, base, head);
        let b = DisplayBuffer::unified(head, base, &diff);
        // Display: ghost(old), new, l1..l10, ghost(old end), new end. Fold
        // l4 to l7, and put a card under l2.
        let fold = Run::new(5, 8, 4);
        let card = Card { id: CardId::GitHub(7), after_display_line: 3, rows: 2 };
        let (left, right) = build_split(&alignment, base, &b, &[fold], &[card]);
        assert_eq!(left.len(), right.len());
        let divider = right.rows.iter().position(|r| matches!(r, DocRow::Divider { .. })).unwrap();
        assert_eq!(left.rows[divider], DocRow::Divider { run: 0 });
        let spacer = right.rows.iter().position(|r| matches!(r, DocRow::Spacer { .. })).unwrap();
        assert_eq!(left.rows[spacer], DocRow::Spacer { card: CardId::GitHub(7), row: 0 });
        assert_eq!(left.rows[spacer + 1], DocRow::Spacer { card: CardId::GitHub(7), row: 1 });
        assert_eq!(right.rows[spacer - 1], line(3, Tint::Plain), "the card is under l2");
        // The folded lines are on no row, and the ones around them are.
        assert_eq!(right.row_of_head(5), None);
        assert_eq!(right.row_of_head(2), Some(spacer - 1));
        assert_eq!(right.row_of_head(3), Some(divider - 1));
        assert_eq!(right.row_of_head(8), Some(divider + 1));
        assert_eq!(left.display_line(divider + 1), Some(8));
    }

    #[test]
    fn a_card_under_a_ghost_goes_after_the_ghost_on_both_sides() {
        // base a b c; head a: b and c removed under a. The card is under
        // the last ghost, display line 2.
        let base = "a\nb\nc\n";
        let head = "a\n";
        let diff = file_diff(base, head);
        let alignment = perch_git::alignment(&diff, base, head);
        let b = DisplayBuffer::unified(head, base, &diff);
        let card = Card { id: CardId::Composer, after_display_line: 2, rows: 1 };
        let (left, right) = build_split(&alignment, base, &b, &[], &[card]);
        assert_eq!(
            left.rows,
            vec![line(0, Tint::Plain), line(1, Tint::Deleted), line(2, Tint::Deleted), DocRow::Spacer { card: CardId::Composer, row: 0 }]
        );
        assert_eq!(
            right.rows,
            vec![line(0, Tint::Plain), DocRow::Filler, DocRow::Filler, DocRow::Spacer { card: CardId::Composer, row: 0 }]
        );
        let stacked = build(&b, &[], &[Card { id: CardId::Composer, after_display_line: 2, rows: 1 }]);
        assert_eq!(stacked.rows[3], DocRow::Spacer { card: CardId::Composer, row: 0 });
    }

    // Runs.

    #[test]
    fn two_hunks_forty_lines_apart_leave_one_run_with_three_lines_off_each_end() {
        let b = two_changes(40);
        // Display: ghost, new top, line 0..39 at display 2 to 41, ghost,
        // new bottom.
        let runs = runs(&b, &[]);
        assert_eq!(runs, vec![Run::new(5, 38, 4)]);
        assert_eq!(runs[0].last_head(), 37);
    }

    #[test]
    fn a_run_of_six_is_no_run_and_a_run_of_seven_hides_one_line() {
        assert!(runs(&two_changes(6), &[]).is_empty());
        let seven = runs(&two_changes(7), &[]);
        assert_eq!(seven, vec![Run::new(5, 5, 4)]);
    }

    #[test]
    fn a_run_an_anchor_sits_in_is_two_runs_each_with_context_on_the_anchors_side() {
        let b = two_changes(40);
        // Head line 20 is "line 19", display line 21.
        let runs = runs(&b, &[(20, 20)]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], Run::new(5, 17, 4), "18, 19 and 20 stay before the anchor");
        assert_eq!(runs[1], Run::new(25, 38, 24), "22, 23 and 24 stay after it");
    }

    #[test]
    fn an_anchor_over_several_lines_keeps_them_all() {
        let b = two_changes(40);
        let runs = runs(&b, &[(10, 30)]);
        assert_eq!(runs, vec![Run::new(5, 7, 4), Run::new(35, 38, 34)]);
    }

    #[test]
    fn the_first_and_last_runs_keep_three_lines_toward_the_change_and_fold_from_line_one_or_to_the_end() {
        // Display: line 0..19 at 0 to 19, ghost(old) at 20, new at 21,
        // line 20..39 at 22 to 41.
        let b = one_change(20, 20);
        let runs = runs(&b, &[]);
        assert_eq!(runs, vec![Run::new(0, 16, 0), Run::new(25, 41, 24)]);
    }

    #[test]
    fn a_short_run_at_either_end_is_shown_whole() {
        assert!(runs(&one_change(6, 6), &[]).is_empty());
        assert_eq!(runs(&one_change(7, 7), &[]).len(), 2);
    }

    #[test]
    fn a_buffer_that_is_one_run_has_nothing_to_fold_around() {
        let b = buffer("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n", "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
        assert!(runs(&b, &[]).is_empty());
        assert!(runs(&DisplayBuffer::plain("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n"), &[]).is_empty());
    }

    #[test]
    fn an_anchor_on_a_changed_line_changes_nothing() {
        let b = two_changes(40);
        assert_eq!(runs(&b, &[(0, 0), (41, 41)]), runs(&b, &[]));
    }

    // Reveals.

    fn two_runs() -> Vec<Run> {
        vec![Run::new(5, 38, 4), Run::new(45, 60, 43)]
    }

    /// A piece of the first of `two_runs`, which folds back to head lines
    /// 4 to 37.
    fn piece(first: usize, last: usize, first_head: usize) -> Run {
        Run { first, last, first_head, default_heads: (4, 37) }
    }

    #[test]
    fn a_reveal_cuts_a_run_back_to_what_it_leaves() {
        let mut reveals = Reveals::default();
        // Up on the first run: its first twenty head lines, 4 to 23.
        reveals.add(4, 23);
        assert_eq!(reveals.apply(&two_runs()), vec![piece(25, 38, 24), Run::new(45, 60, 43)]);
        // Down on the second, its last twenty, is more than it has: it
        // is gone.
        reveals.add(39, 58);
        assert_eq!(reveals.apply(&two_runs()), vec![piece(25, 38, 24)]);
    }

    #[test]
    fn a_reveal_inside_a_run_leaves_a_piece_on_each_side_and_both_fold_back_to_the_whole() {
        let mut reveals = Reveals::default();
        reveals.add(10, 12);
        let pieces = reveals.apply(&two_runs()[..1]);
        assert_eq!(pieces, vec![piece(5, 10, 4), piece(14, 38, 13)]);
        // Either piece's Reset undoes the reveal between them.
        for p in &pieces {
            assert!(reveals.overlaps(p.default_heads.0, p.default_heads.1));
            let mut after = reveals.clone();
            after.remove_overlapping(p.default_heads.0, p.default_heads.1);
            assert!(after.is_empty());
        }
    }

    #[test]
    fn a_reveal_that_covers_a_run_removes_it() {
        let mut reveals = Reveals::default();
        reveals.add(0, 100);
        assert!(reveals.apply(&two_runs()).is_empty());
        let mut exact = Reveals::default();
        exact.add(4, 37);
        assert_eq!(exact.apply(&two_runs()), vec![Run::new(45, 60, 43)]);
    }

    #[test]
    fn a_reveal_that_straddles_two_runs_cuts_both() {
        let mut reveals = Reveals::default();
        reveals.add(30, 50);
        assert_eq!(
            reveals.apply(&two_runs()),
            vec![piece(5, 30, 4), Run { first: 53, last: 60, first_head: 51, default_heads: (43, 58) }]
        );
    }

    #[test]
    fn no_reveals_leave_the_runs_as_they_were() {
        let reveals = Reveals::default();
        assert!(reveals.is_empty());
        assert_eq!(reveals.apply(&two_runs()), two_runs());
    }

    #[test]
    fn a_runs_reveal_ranges_are_its_ends_and_the_whole_of_it() {
        let run = Run::new(5, 45, 4);
        assert_eq!(run.len(), 41);
        assert!(!run.reveals_whole(), "forty-one lines is Up and Down");
        assert_eq!(run.top(), (4, 23));
        assert_eq!(run.bottom(), (25, 44));
        assert_eq!(run.whole(), (4, 44));
        let short = Run::new(5, 44, 4);
        assert!(short.reveals_whole(), "forty lines is All");
        let tiny = Run::new(5, 9, 4);
        assert_eq!(tiny.top(), (4, 8), "a step never reaches past the run");
        assert_eq!(tiny.bottom(), (4, 8));
        assert_eq!(divider_label(47), "47 unchanged lines");
        assert_eq!(divider_label(1), "1 unchanged line");
    }

    #[test]
    fn reset_forgets_the_reveals_overlapping_a_run_and_no_other() {
        let mut reveals = Reveals::default();
        reveals.add(4, 23);
        reveals.add(43, 62);
        reveals.add(23, 4);
        assert_eq!(reveals.iter().collect::<Vec<_>>(), vec![(4, 23), (43, 62)], "a range is one however it was given");
        assert!(reveals.overlaps(4, 37));
        assert!(!reveals.overlaps(24, 42));
        reveals.remove_overlapping(4, 37);
        assert_eq!(reveals.iter().collect::<Vec<_>>(), vec![(43, 62)]);
        assert!(!reveals.overlaps(4, 37));
    }

    // The builder over the folds the reveals leave.

    /// The stacked document of a buffer folded around its changes, less
    /// the reveals.
    fn folded(b: &DisplayBuffer, reveals: &Reveals) -> Doc {
        build(b, &reveals.apply(&runs(b, &[])), &[])
    }

    #[test]
    fn the_builder_hides_the_runs_and_shows_what_a_reveal_asks_for() {
        // Display: ghost, new top, line 0..39 at 2 to 41, ghost, new
        // bottom: 44 rows, and one run of 34 behind a divider at row 5.
        let b = two_changes(40);
        let run = runs(&b, &[])[0];
        let doc = folded(&b, &Reveals::default());
        assert_eq!(doc.len(), 44 - 34 + 1);
        assert_eq!(doc.run_of(5), Some(run));
        assert_eq!(doc.run_of(4), None);
        assert_eq!(doc.hidden_lines(), 34);
        assert_eq!(doc.line(5), "", "the divider's text is blank");
        assert_eq!(doc.line(6), "line 37", "display line 39 follows the divider");
        assert_eq!(doc.offset_of_row(6), doc.offset_of_row(5) + 1, "the divider is one newline");
        assert_eq!(doc.offset_of_row(doc.len()), doc.text.len());

        // Down: the last twenty head lines of the run, 18 to 37, come
        // back, and the divider hides fourteen.
        let mut reveals = Reveals::default();
        let (a, z) = run.bottom();
        reveals.add(a, z);
        let doc = folded(&b, &reveals);
        assert_eq!(doc.len(), 11 + 20);
        assert_eq!(doc.run_of(5), Some(Run { first: 5, last: 18, first_head: 4, default_heads: (4, 37) }));
        assert_eq!(doc.hidden_lines(), 14);
        assert_eq!(doc.row_of_head(18), Some(6));
        assert_eq!(doc.row_of_head(17), None);

        // All: no divider, and the document is the buffer.
        let (a, z) = run.whole();
        reveals.add(a, z);
        let doc = folded(&b, &reveals);
        assert_eq!(doc.hidden_lines(), 0);
        assert_eq!(doc.text, b.text);
        assert_eq!(doc.len(), b.len());

        // Reset: back to the default.
        reveals.remove_overlapping(run.first_head, run.last_head());
        assert_eq!(folded(&b, &reveals), folded(&b, &Reveals::default()));
    }

    #[test]
    fn a_line_an_anchor_sits_on_stays_in_view_with_its_context() {
        let b = two_changes(40);
        let doc = build(&b, &runs(&b, &[(20, 20)]), &[]);
        // Two dividers, with head lines 17 to 23 between them.
        let dividers: Vec<usize> = (0..doc.len()).filter(|r| doc.run_of(*r).is_some()).collect();
        assert_eq!(dividers.len(), 2);
        for head in 17..=23 {
            assert!(doc.row_of_head(head).is_some(), "head line {head} is shown");
        }
        assert_eq!(doc.row_of_head(16), None);
        assert_eq!(doc.row_of_head(24), None);
    }

    // The region an edit replaces.

    /// The text after replacing the region's old bytes with the new.
    fn edited(old: &Doc, new: &Doc, region: &Region) -> String {
        let mut text = old.text.clone();
        text.replace_range(region.old_bytes.clone(), &new.text[region.new_bytes.clone()]);
        text
    }

    #[test]
    fn a_down_reveal_is_one_divider_row_replaced_by_twenty_lines_and_a_divider() {
        let b = two_changes(40);
        let run = runs(&b, &[])[0];
        let old = folded(&b, &Reveals::default());
        let mut reveals = Reveals::default();
        let (a, z) = run.bottom();
        reveals.add(a, z);
        let new = folded(&b, &reveals);

        let region = changed_region(&old, &new).expect("the documents differ");
        assert_eq!(region.old_rows, 5..6);
        assert_eq!(region.new_rows, 5..26);
        assert_eq!(region.row_delta(), 20);
        assert_eq!(new.rows[5], DocRow::Divider { run: 0 });
        assert_eq!(new.rows[6], line(19, Tint::Plain));
        assert_eq!(new.rows[25], line(38, Tint::Plain));
        assert_eq!(region.old_bytes.len(), 1, "the divider's blank line and its newline");
        assert_eq!(region.byte_delta() as usize, region.new_bytes.len() - 1);
        assert_eq!(edited(&old, &new, &region), new.text);
    }

    #[test]
    fn a_reset_is_the_same_region_the_other_way_round() {
        let b = two_changes(40);
        let run = runs(&b, &[])[0];
        let mut reveals = Reveals::default();
        let (a, z) = run.bottom();
        reveals.add(a, z);
        let old = folded(&b, &reveals);
        let new = folded(&b, &Reveals::default());

        let region = changed_region(&old, &new).expect("the documents differ");
        assert_eq!(region.old_rows, 5..26);
        assert_eq!(region.new_rows, 5..6);
        assert_eq!(region.row_delta(), -20);
        assert_eq!(region.new_bytes.len(), 1);
        assert_eq!(edited(&old, &new, &region), new.text);
    }

    #[test]
    fn the_same_document_twice_has_no_region() {
        let b = two_changes(40);
        let doc = folded(&b, &Reveals::default());
        assert_eq!(changed_region(&doc, &doc.clone()), None);
        let empty = build(&DisplayBuffer::plain(""), &[], &[]);
        assert_eq!(changed_region(&empty, &empty.clone()), None);
        // From nothing to something is everything.
        let region = changed_region(&empty, &doc).unwrap();
        assert_eq!(region.old_rows, 0..0);
        assert_eq!(region.new_rows, 0..doc.len());
        assert_eq!(edited(&empty, &doc, &region), doc.text);
    }

    #[test]
    fn a_divider_whose_run_moved_in_the_folds_is_still_the_same_row() {
        // Three changes, two runs. Revealing the whole of the first run
        // takes its divider out of the folds, so the second run becomes
        // run 0; the divider standing for it is the same divider.
        let context = |n: usize| (0..n).map(|i| format!("line {i}\n")).collect::<String>();
        let base = format!("old top\n{}old mid\n{}old bottom\n", context(20), context(20));
        let head = format!("new top\n{}new mid\n{}new bottom\n", context(20), context(20));
        let b = buffer(&base, &head);
        let all = runs(&b, &[]);
        assert_eq!(all.len(), 2);
        let old = build(&b, &all, &[]);
        let mut reveals = Reveals::default();
        let (a, z) = all[0].whole();
        reveals.add(a, z);
        let new = build(&b, &reveals.apply(&all), &[]);
        assert_eq!(new.folds, vec![all[1]]);

        let region = changed_region(&old, &new).unwrap();
        assert_eq!(region.old_rows, 5..6, "the first divider alone");
        assert_eq!(region.new_rows, 5..(5 + all[0].len()));
        assert_eq!(edited(&old, &new, &region), new.text);
        let second = (0..new.len()).find(|r| new.run_of(*r).is_some()).unwrap();
        assert_eq!(new.rows[second], DocRow::Divider { run: 0 });
        assert!(second >= region.new_rows.end, "the second divider is in the common suffix");
    }
}
