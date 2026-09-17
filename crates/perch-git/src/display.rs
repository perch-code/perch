//! The unified diff as a buffer the editor can render directly.
//!
//! Floem's editor cannot attach background colour to phantom text, and
//! injecting ghost lines needs a custom `Document` implementation. Splicing
//! the removed lines into one synthetic buffer sidesteps both: the editor
//! renders plain text, and a side table says what each display line really
//! is. Everything that needs a real file position, above all the language
//! server, translates through that table first.

use serde::{Deserialize, Serialize};

use crate::diff::{FileDiff, LineStatus};

/// What a single rendered line actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Row {
    /// A line of the head file, at `head_line` (zero based).
    Head { head_line: usize, status: LineStatus },
    /// A line the pull request removed, at `base_line` in the base file.
    Deleted { base_line: usize },
}

impl Row {
    pub fn head_line(self) -> Option<usize> {
        match self {
            Row::Head { head_line, .. } => Some(head_line),
            Row::Deleted { .. } => None,
        }
    }

    pub fn is_deleted(self) -> bool {
        matches!(self, Row::Deleted { .. })
    }

    pub fn status(self) -> Option<LineStatus> {
        match self {
            Row::Head { status, .. } => Some(status),
            Row::Deleted { .. } => None,
        }
    }
}

/// A file laid out for display, with the mapping back to real positions.
#[derive(Debug, Clone, Default)]
pub struct DisplayBuffer {
    /// The text handed to the editor.
    pub text: String,
    /// One entry per line of `text`.
    pub rows: Vec<Row>,
    /// Display lines that begin a change, for next and previous change.
    pub change_starts: Vec<usize>,
}

impl DisplayBuffer {
    /// A file with no diff: every line is itself.
    pub fn plain(text: &str) -> Self {
        let rows = (0..line_count(text))
            .map(|head_line| Row::Head { head_line, status: LineStatus::Unchanged })
            .collect();
        Self { text: text.to_string(), rows, change_starts: Vec::new() }
    }

    /// Splice the removed lines of `diff` into `head`.
    ///
    /// `base` supplies the text of the removed lines; when it is empty the
    /// ghost lines are still placed, just blank.
    pub fn unified(head: &str, base: &str, diff: &FileDiff) -> Self {
        let head_lines: Vec<&str> = head.lines().collect();
        let base_lines: Vec<&str> = base.lines().collect();

        // Where each removed run was anchored, and which base line it began
        // at, so ghost lines can be highlighted with the base file's spans.
        let mut deletions_at: Vec<Vec<(usize, &str)>> = vec![Vec::new(); head_lines.len() + 1];
        let mut consumed = 0usize;
        for run in &diff.deleted {
            let anchor = run.before_line.min(head_lines.len());
            // Find where this run sits in the base file so the ghost lines
            // can carry real base line numbers.
            let base_start = base_lines
                .windows(run.lines.len().max(1))
                .enumerate()
                .skip(consumed)
                .find(|(_, w)| w.iter().zip(&run.lines).all(|(a, b)| a == b))
                .map(|(i, _)| i)
                .unwrap_or(consumed);
            consumed = base_start + run.lines.len();

            for (offset, line) in run.lines.iter().enumerate() {
                deletions_at[anchor].push((base_start + offset, line.as_str()));
            }
        }

        let mut text = String::with_capacity(head.len() + base.len() / 4);
        let mut rows = Vec::with_capacity(head_lines.len() + diff.deleted.len());
        let mut change_starts = Vec::new();
        let mut previous_changed = false;

        let push = |text: &mut String, rows: &mut Vec<Row>, line: &str, row: Row| {
            if !rows.is_empty() {
                text.push('\n');
            }
            text.push_str(line);
            rows.push(row);
        };

        for (head_line, line) in head_lines.iter().enumerate() {
            for (base_line, ghost) in &deletions_at[head_line] {
                let changed = true;
                if !previous_changed {
                    change_starts.push(rows.len());
                }
                previous_changed = changed;
                push(&mut text, &mut rows, ghost, Row::Deleted { base_line: *base_line });
            }

            let status = diff.status_at(head_line);
            let changed = status != LineStatus::Unchanged;
            if changed && !previous_changed {
                change_starts.push(rows.len());
            }
            previous_changed = changed;
            push(&mut text, &mut rows, line, Row::Head { head_line, status });
        }

        // Anything removed from the end of the file.
        for (base_line, ghost) in &deletions_at[head_lines.len()] {
            if !previous_changed {
                change_starts.push(rows.len());
            }
            previous_changed = true;
            push(&mut text, &mut rows, ghost, Row::Deleted { base_line: *base_line });
        }

        if !rows.is_empty() {
            text.push('\n');
        }

        Self { text, rows, change_starts }
    }

    pub fn row(&self, display_line: usize) -> Option<Row> {
        self.rows.get(display_line).copied()
    }

    /// Translate a display line to a head-file line.
    ///
    /// `None` for a ghost line, which has no position in the head file. Every
    /// language server request must go through this.
    pub fn head_line(&self, display_line: usize) -> Option<usize> {
        self.row(display_line)?.head_line()
    }

    /// Translate a head-file line back to where it is displayed.
    pub fn display_line(&self, head_line: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.head_line() == Some(head_line))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn next_change(&self, from: usize) -> Option<usize> {
        self.change_starts.iter().copied().find(|s| *s > from)
    }

    pub fn previous_change(&self, from: usize) -> Option<usize> {
        self.change_starts.iter().copied().rev().find(|s| *s < from)
    }
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::file_diff;

    #[test]
    fn a_plain_file_maps_every_line_to_itself() {
        let b = DisplayBuffer::plain("a\nb\nc\n");
        assert_eq!(b.len(), 3);
        assert_eq!(b.head_line(0), Some(0));
        assert_eq!(b.head_line(2), Some(2));
        assert!(b.change_starts.is_empty());
    }

    #[test]
    fn an_unchanged_file_renders_exactly_its_own_text() {
        let head = "a\nb\nc\n";
        let d = file_diff(head, head);
        let b = DisplayBuffer::unified(head, head, &d);
        assert_eq!(b.text, head);
        assert!(b.rows.iter().all(|r| !r.is_deleted()));
    }

    #[test]
    fn removed_lines_appear_above_the_line_that_replaced_them() {
        let base = "a\nb\nc\n";
        let head = "a\nB\nc\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);

        // a, ghost(b), B, c
        assert_eq!(b.len(), 4);
        assert_eq!(b.row(0), Some(Row::Head { head_line: 0, status: LineStatus::Unchanged }));
        assert!(b.row(1).unwrap().is_deleted());
        assert_eq!(b.row(2).unwrap().head_line(), Some(1));
        assert_eq!(b.text.lines().nth(1), Some("b"), "the ghost carries the old text");
        assert_eq!(b.text.lines().nth(2), Some("B"));
    }

    #[test]
    fn a_ghost_line_has_no_head_position() {
        let base = "a\nb\nc\n";
        let head = "a\nc\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);

        let ghost = b.rows.iter().position(|r| r.is_deleted()).unwrap();
        assert_eq!(
            b.head_line(ghost),
            None,
            "a language server must never be asked about a deleted line"
        );
    }

    #[test]
    fn ghost_lines_carry_their_base_line_number() {
        let base = "a\nb\nc\nd\n";
        let head = "a\nd\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);

        let ghosts: Vec<Row> = b.rows.iter().copied().filter(|r| r.is_deleted()).collect();
        assert_eq!(ghosts.len(), 2);
        assert_eq!(ghosts[0], Row::Deleted { base_line: 1 });
        assert_eq!(ghosts[1], Row::Deleted { base_line: 2 });
    }

    #[test]
    fn head_lines_round_trip_through_the_display_mapping() {
        let base = "a\nb\nc\nd\ne\n";
        let head = "a\nX\nc\nY\ne\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);

        for head_line in 0..head.lines().count() {
            let display = b.display_line(head_line).expect("every head line is displayed");
            assert_eq!(b.head_line(display), Some(head_line));
        }
    }

    #[test]
    fn every_head_line_survives_the_splice() {
        let base = "one\ntwo\nthree\n";
        let head = "one\ntwo prime\nthree\nfour\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);

        let rendered: Vec<&str> = b.text.lines().collect();
        for line in head.lines() {
            assert!(rendered.contains(&line), "{line:?} vanished from the buffer");
        }
    }

    #[test]
    fn the_row_table_matches_the_rendered_line_count() {
        let base = "a\nb\nc\n";
        let head = "a\nX\nY\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);
        assert_eq!(b.rows.len(), b.text.lines().count());
    }

    #[test]
    fn a_new_file_is_all_additions_with_no_ghosts() {
        let head = "a\nb\n";
        let d = file_diff("", head);
        let b = DisplayBuffer::unified(head, "", &d);
        assert!(b.rows.iter().all(|r| r.status() == Some(LineStatus::Added)));
        assert!(!b.rows.iter().any(|r| r.is_deleted()));
    }

    #[test]
    fn a_deleted_file_is_all_ghosts() {
        let base = "a\nb\n";
        let d = file_diff(base, "");
        let b = DisplayBuffer::unified("", base, &d);
        assert_eq!(b.len(), 2);
        assert!(b.rows.iter().all(|r| r.is_deleted()));
        assert_eq!(b.text, "a\nb\n");
    }

    #[test]
    fn changes_are_navigable_forward_and_back() {
        let base = "a\nb\nc\nd\ne\n";
        let head = "a\nB\nc\nD\ne\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);

        assert_eq!(b.change_starts.len(), 2, "two separate changed regions");
        let first = b.change_starts[0];
        let second = b.change_starts[1];
        assert_eq!(b.next_change(0), Some(first).filter(|f| *f > 0).or(Some(second)));
        assert_eq!(b.next_change(first), Some(second));
        assert_eq!(b.next_change(second), None);
        assert_eq!(b.previous_change(second), Some(first));
        assert_eq!(b.previous_change(0), None);
    }

    #[test]
    fn a_removal_at_end_of_file_still_renders() {
        let base = "a\nb\nc\n";
        let head = "a\n";
        let d = file_diff(base, head);
        let b = DisplayBuffer::unified(head, base, &d);
        assert_eq!(b.len(), 3);
        assert!(b.rows[1].is_deleted());
        assert!(b.rows[2].is_deleted());
    }

    #[test]
    fn an_empty_file_produces_an_empty_buffer() {
        let b = DisplayBuffer::plain("");
        assert!(b.is_empty());
        assert_eq!(b.head_line(0), None);
    }
}
