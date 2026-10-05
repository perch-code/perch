//! Telling the editor how to draw a file.
//!
//! Two hooks do all the work. `apply_attr_styles` colours the text from
//! tree-sitter spans, and `apply_layout_styles` paints the tint behind
//! every other use of the symbol the caret rests on, and behind every
//! match of the find bar's query, on the range rather than the line.
//! Both are handed a row of the editor, so anything that needs a real
//! file position goes through the document's rows first, and from there
//! through the buffer's row table.
//!
//! The diff tint behind a whole line is not painted here. The editor
//! paints a line's extra styles after the selection, so a tint among them
//! would cover the selection on every changed line; the tint is a layer
//! under the editor instead, in `editor`.

use std::borrow::Cow;
use std::sync::Arc;

use floem::reactive::{RwSignal, SignalGet, SignalWith};
use floem::text::{Affinity, Attrs, AttrsList, FamilyOwned, FontWeight};
use floem::views::editor::id::EditorId;
use floem::views::editor::layout::{LineExtraStyle, TextLayoutLine};
use floem::views::editor::text::Styling;
use floem::views::editor::EditorStyle;
use perch_syntax::Span;

use crate::review::doc::{Doc, DocRow, Tint};
use crate::review::editor::Side;
use crate::review::find::Match;
use crate::review::model::LiveFile;
use crate::theme;

pub struct PerchStyling {
    /// Read live, as the document is: a reload is edited in place under
    /// the editor, and its spans come with it.
    file: LiveFile,
    /// The document the editor is laying out. Read live, never copied:
    /// the document is edited in place under the editor, and a row asked
    /// about after an edit is the edited one.
    doc: RwSignal<Arc<Doc>>,
    /// Set for one half of a split: the base side's document is over the
    /// base file, and its spans are that file's.
    side: Option<Side>,
    /// The uses of the caret's symbol, as head positions. Read live: the
    /// editor's layouts are dropped when they change, and rebuilt through
    /// this.
    highlights: RwSignal<Arc<Vec<(usize, usize, usize)>>>,
    /// The find bar's matches, as display lines, and which of them the
    /// bar is on. Read live the same way.
    find: RwSignal<Arc<Vec<Match>>>,
    find_current: RwSignal<Option<usize>>,
    families: Vec<FamilyOwned>,
    /// Cache key. Bumped per loaded file so the editor drops layouts from the
    /// file before; without this a second file reuses the first one's colours.
    id: u64,
}

impl PerchStyling {
    /// The stacked view: the document over the unified buffer.
    pub fn new(
        file: LiveFile,
        doc: RwSignal<Arc<Doc>>,
        highlights: RwSignal<Arc<Vec<(usize, usize, usize)>>>,
        find: RwSignal<Arc<Vec<Match>>>,
        find_current: RwSignal<Option<usize>>,
        id: u64,
    ) -> Self {
        Self {
            file,
            doc,
            side: None,
            highlights,
            find,
            find_current,
            families: FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect(),
            id,
        }
    }

    /// One half of a split.
    pub fn side(
        file: LiveFile,
        side: Side,
        doc: RwSignal<Arc<Doc>>,
        highlights: RwSignal<Arc<Vec<(usize, usize, usize)>>>,
        find: RwSignal<Arc<Vec<Match>>>,
        find_current: RwSignal<Option<usize>>,
        id: u64,
    ) -> Self {
        Self {
            side: Some(side),
            ..Self::new(file, doc, highlights, find, find_current, id)
        }
    }

    /// The columns of the caret's symbol's uses on a row: those on the
    /// row's head line, clamped to the line. A row that is no head line,
    /// a ghost or a base side's line, has none; a use inside a fold is on
    /// no row, and is not drawn until a reveal puts it on one.
    fn highlights_for(&self, row: usize) -> Vec<(usize, usize)> {
        let Some(head) = self.doc.with_untracked(|d| d.head_line(row)) else {
            return Vec::new();
        };
        let len = self.line_len(row);
        self.highlights.with_untracked(|h| {
            h.iter()
                .filter(|(line, _, _)| *line == head)
                .map(|(_, start, end)| ((*start).min(len), (*end).min(len)))
                .filter(|(start, end)| start < end)
                .collect()
        })
    }

    /// The columns of the find bar's matches on a row, each with whether
    /// it is the one the bar is on: those on the row's display line,
    /// clamped to the line. A match is a line of the unified buffer, so a
    /// ghost's are drawn where the ghost is, and a split's base side,
    /// whose lines are the base file's own, draws none. A row that is no
    /// line has none, and a match inside a fold is on no row.
    fn matches_for(&self, row: usize) -> Vec<(usize, usize, bool)> {
        if self.side == Some(Side::Base) {
            return Vec::new();
        }
        let Some(display) = self.doc.with_untracked(|d| d.display_line(row)) else {
            return Vec::new();
        };
        let len = self.line_len(row);
        let current = self.find_current.get_untracked();
        self.find.with_untracked(|matches| {
            // In order down the file, so the row's are one run of it.
            let first = matches.partition_point(|m| m.display < display);
            matches[first..]
                .iter()
                .take_while(|m| m.display == display)
                .enumerate()
                .map(|(i, m)| (m.start.min(len), m.end.min(len), current == Some(first + i)))
                .filter(|(start, end, _)| start < end)
                .collect()
        })
    }

    fn row(&self, row: usize) -> Option<DocRow> {
        self.doc.with_untracked(|d| d.rows.get(row).cloned())
    }

    /// Highlight spans for a row, taken from whichever side of the diff
    /// its line came from. A row that is no line has none.
    fn spans_for(&self, row: usize) -> Vec<Span> {
        let Some(DocRow::Line { display, .. }) = self.row(row) else {
            return Vec::new();
        };
        self.file.with_untracked(|file| {
            match self.side {
                // The base side's document is over the base file, whose
                // display lines are its own.
                Some(Side::Base) => file.base_spans.line(display),
                Some(Side::Head) | None => match file.buffer.row(display) {
                    Some(perch_git::Row::Head { head_line, .. }) => file.head_spans.line(head_line),
                    Some(perch_git::Row::Deleted { base_line }) => file.base_spans.line(base_line),
                    None => &[],
                },
            }
            .to_vec()
        })
    }

    /// Whether a row is a ghost of the stacked view: a removed line spliced
    /// in among the head's. A split's left is the old file whole, and reads
    /// in full colour.
    fn is_ghost(&self, row: usize) -> bool {
        self.side.is_none()
            && matches!(
                self.row(row),
                Some(DocRow::Line {
                    tint: Tint::Deleted,
                    ..
                })
            )
    }

    fn line_len(&self, row: usize) -> usize {
        self.doc.with_untracked(|d| d.line(row).len())
    }
}

impl Styling for PerchStyling {
    fn id(&self) -> u64 {
        theme::styling_id(self.id)
    }

    fn font_size(&self, _edid: EditorId, _line: usize) -> usize {
        theme::CODE_FONT_SIZE as usize
    }

    fn line_height(&self, _edid: EditorId, _line: usize) -> f32 {
        // Fixed, because the gutter places its numbers by multiplying this.
        theme::CODE_LINE_HEIGHT
    }

    fn font_family(&self, _edid: EditorId, _line: usize) -> Cow<'_, [FamilyOwned]> {
        Cow::Borrowed(&self.families)
    }

    fn tab_width(&self, _edid: EditorId, _line: usize) -> usize {
        4
    }

    /// Nothing can be typed, so a blinking caret would only be noise.
    fn paint_caret(&self, _edid: EditorId, _line: usize) -> bool {
        false
    }

    fn apply_attr_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        default: Attrs,
        attrs: &mut AttrsList,
    ) {
        // Removed code is context, not the thing under review, so it is
        // dimmed rather than fully coloured.
        if self.is_ghost(line) {
            let span_len = self.line_len(line);
            if span_len > 0 {
                attrs.add_span(0..span_len, default.clone().color(theme::diff_deleted_fg()));
            }
            return;
        }

        for span in self.spans_for(line) {
            if span.start >= span.end {
                continue;
            }
            attrs.add_span(
                span.start..span.end,
                default.clone().color(theme::syntax_color(span.capture)),
            );
        }
    }

    fn apply_layout_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        layout_line: &mut TextLayoutLine,
    ) {
        // The symbol's uses, then the find bar's matches over them: the
        // range's columns as the layout places them.
        let uses = self
            .highlights_for(line)
            .into_iter()
            .map(|(start, end)| (start, end, theme::symbol_highlight_bg()));
        let matches = self
            .matches_for(line)
            .into_iter()
            .map(|(start, end, current)| {
                let color = if current {
                    theme::find_current_bg()
                } else {
                    theme::find_match_bg()
                };
                (start, end, color)
            });
        for (start, end, color) in uses.chain(matches) {
            let x0 = layout_line.text.cursor_point(start, Affinity::Downstream).x;
            let x1 = layout_line.text.cursor_point(end, Affinity::Upstream).x;
            if x1 <= x0 {
                continue;
            }
            layout_line.extra_style.push(LineExtraStyle {
                x: x0,
                y: 0.0,
                width: Some(x1 - x0),
                height: theme::CODE_LINE_HEIGHT as f64,
                bg_color: Some(color),
                under_line: None,
                wave_line: None,
            });
        }
    }

    fn weight(&self, _edid: EditorId, _line: usize) -> FontWeight {
        theme::weight_body()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::doc::{self, Run};
    use floem::reactive::SignalUpdate;
    use perch_git::DisplayBuffer;

    fn styling(text: &str, folds: Vec<Run>) -> PerchStyling {
        let file = Arc::new(crate::backend::build_loaded_file(
            "src/x.rs".into(),
            text.into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));
        let doc = RwSignal::new(Arc::new(doc::build(
            &DisplayBuffer::plain(text),
            &folds,
            &[],
        )));
        PerchStyling::new(
            RwSignal::new(file),
            doc,
            RwSignal::new(Arc::new(Vec::new())),
            RwSignal::new(Arc::new(Vec::new())),
            RwSignal::new(None),
            1,
        )
    }

    #[test]
    fn the_uses_on_a_row_are_its_head_lines_and_a_folded_one_is_on_no_row() {
        let text = "let path = 1;\nlet a = path;\nlet b = path;\nlet c = path;\nlet d = path;\nlet e = path;\nlet f = 2;\n";
        // Display lines 2 to 4 folded away: rows are lines 0, 1, a
        // divider, then 5 and 6.
        let styling = styling(text, vec![Run::new(2, 4, 2)]);
        styling.highlights.set(Arc::new(vec![
            (0, 4, 8),
            (1, 8, 12),
            (3, 8, 12),
            (5, 8, 12),
            (5, 40, 44),
        ]));

        assert_eq!(styling.highlights_for(0), vec![(4, 8)]);
        assert_eq!(styling.highlights_for(1), vec![(8, 12)]);
        assert!(
            styling.highlights_for(2).is_empty(),
            "the divider is no head line"
        );
        assert_eq!(
            styling.highlights_for(3),
            vec![(8, 12)],
            "row 3 is head line 5, past the fold"
        );
        assert!(styling.highlights_for(4).is_empty());
        // Head line 3's use is inside the fold: on no row, so not drawn.
        assert!(
            !(0..5).any(|row| styling.highlights_for(row).contains(&(8, 12))
                && styling.doc.with_untracked(|d| d.head_line(row)) == Some(3))
        );

        // Revealed, the fold's lines have rows, and the use is drawn.
        let whole = doc::build(&DisplayBuffer::plain(text), &[], &[]);
        styling.doc.set(Arc::new(whole));
        assert_eq!(styling.highlights_for(3), vec![(8, 12)]);
        assert_eq!(styling.doc.with_untracked(|d| d.head_line(3)), Some(3));

        // A range past the line is clamped to it, and one on nothing is
        // dropped.
        assert_eq!(styling.highlights_for(5), vec![(8, 12)]);
    }

    #[test]
    fn the_matches_on_a_row_are_its_display_lines_and_the_current_one_is_marked() {
        let m = |display, start, end| Match {
            display,
            start,
            end,
        };
        let text = "let path = 1;\nlet a = path;\nlet b = path;\nlet c = path;\nlet d = path;\nlet e = path;\nlet f = 2;\n";
        // Display lines 2 to 4 folded away, as above.
        let styling = styling(text, vec![Run::new(2, 4, 2)]);
        styling.find.set(Arc::new(vec![
            m(0, 4, 8),
            m(1, 8, 12),
            m(3, 8, 12),
            m(5, 8, 12),
            m(5, 40, 44),
        ]));
        styling.find_current.set(Some(3));

        assert_eq!(styling.matches_for(0), vec![(4, 8, false)]);
        assert_eq!(styling.matches_for(1), vec![(8, 12, false)]);
        assert!(styling.matches_for(2).is_empty(), "the divider is no line");
        assert_eq!(
            styling.matches_for(3),
            vec![(8, 12, true)],
            "row 3 is line 5, the current match; the range past the line is dropped"
        );
        assert!(styling.matches_for(4).is_empty(), "line 6 has none");
        assert!(
            styling.matches_for(9).is_empty(),
            "a row the document does not have"
        );

        // Revealed, line 3's match has a row, and is not the current one.
        let whole = doc::build(&DisplayBuffer::plain(text), &[], &[]);
        styling.doc.set(Arc::new(whole));
        assert_eq!(styling.matches_for(3), vec![(8, 12, false)]);
        assert_eq!(styling.matches_for(5), vec![(8, 12, true)]);
        styling.find_current.set(None);
        assert_eq!(styling.matches_for(5), vec![(8, 12, false)]);

        // A split's base side is over the base file, whose lines are not
        // the unified buffer's: nothing is drawn there.
        let base = PerchStyling {
            side: Some(Side::Base),
            ..styling
        };
        assert!(base.matches_for(0).is_empty());
        let head = PerchStyling {
            side: Some(Side::Head),
            ..base
        };
        assert_eq!(head.matches_for(0), vec![(4, 8, false)]);
    }
}
