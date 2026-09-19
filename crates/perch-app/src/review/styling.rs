//! Telling the editor how to draw a file.
//!
//! Two hooks do all the work. `apply_attr_styles` colours the text from
//! tree-sitter spans, and `apply_layout_styles` paints the diff background
//! behind a whole line. Both are handed a row of the editor, so anything
//! that needs a real file position goes through the document's rows first,
//! and from there through the buffer's row table.

use std::borrow::Cow;
use std::sync::Arc;

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalWith};
use floem::text::{Attrs, AttrsList, FamilyOwned, FontWeight};
use floem::views::editor::id::EditorId;
use floem::views::editor::layout::{LineExtraStyle, TextLayoutLine};
use floem::views::editor::text::Styling;
use floem::views::editor::EditorStyle;
use perch_syntax::Span;

use crate::review::doc::{Doc, DocRow, Tint};
use crate::review::editor::Side;
use crate::review::model::LoadedFile;
use crate::theme;

pub struct PerchStyling {
    file: Arc<LoadedFile>,
    /// The document the editor is laying out. Read live, never copied:
    /// the document is edited in place under the editor, and a row asked
    /// about after an edit is the edited one.
    doc: RwSignal<Arc<Doc>>,
    /// Set for one half of a split: the base side's document is over the
    /// base file, and its spans are that file's.
    side: Option<Side>,
    families: Vec<FamilyOwned>,
    /// Cache key. Bumped per loaded file so the editor drops layouts from the
    /// file before; without this a second file reuses the first one's colours.
    id: u64,
}

impl PerchStyling {
    /// The stacked view: the document over the unified buffer.
    pub fn new(file: Arc<LoadedFile>, doc: RwSignal<Arc<Doc>>, id: u64) -> Self {
        Self {
            file,
            doc,
            side: None,
            families: FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect(),
            id,
        }
    }

    /// One half of a split.
    pub fn side(file: Arc<LoadedFile>, side: Side, doc: RwSignal<Arc<Doc>>, id: u64) -> Self {
        Self { side: Some(side), ..Self::new(file, doc, id) }
    }

    fn row(&self, row: usize) -> Option<DocRow> {
        self.doc.with_untracked(|d| d.rows.get(row).cloned())
    }

    /// Highlight spans for a row, taken from whichever side of the diff
    /// its line came from. A row that is no line has none.
    fn spans_for(&self, row: usize) -> &[Span] {
        let Some(DocRow::Line { display, .. }) = self.row(row) else {
            return &[];
        };
        match self.side {
            // The base side's document is over the base file, whose display
            // lines are its own.
            Some(Side::Base) => self.file.base_spans.line(display),
            Some(Side::Head) | None => match self.file.buffer.row(display) {
                Some(perch_git::Row::Head { head_line, .. }) => self.file.head_spans.line(head_line),
                Some(perch_git::Row::Deleted { base_line }) => self.file.base_spans.line(base_line),
                None => &[],
            },
        }
    }

    fn background_for(&self, row: usize) -> Option<Color> {
        match self.row(row)? {
            DocRow::Line { tint, .. } => match tint {
                Tint::Plain => None,
                Tint::Added => Some(theme::DIFF_ADDED_BG),
                Tint::Modified => Some(theme::DIFF_MODIFIED_BG),
                Tint::Deleted => Some(theme::DIFF_DELETED_BG),
            },
            DocRow::Filler => Some(theme::DIFF_FILLER_BG),
            DocRow::Divider { .. } => Some(theme::DIFF_DIVIDER_BG),
            // The card is drawn over its rows, so they carry nothing.
            DocRow::Spacer { .. } => None,
        }
    }

    /// Whether a row is a ghost of the stacked view: a removed line spliced
    /// in among the head's. A split's left is the old file whole, and reads
    /// in full colour.
    fn is_ghost(&self, row: usize) -> bool {
        self.side.is_none() && matches!(self.row(row), Some(DocRow::Line { tint: Tint::Deleted, .. }))
    }

    fn line_len(&self, row: usize) -> usize {
        self.doc.with_untracked(|d| d.text.lines().nth(row).map(str::len).unwrap_or(0))
    }
}

impl Styling for PerchStyling {
    fn id(&self) -> u64 {
        self.id
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
                attrs.add_span(0..span_len, default.clone().color(theme::DIFF_DELETED_FG));
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
        let Some(bg) = self.background_for(line) else {
            return;
        };
        layout_line.extra_style.push(LineExtraStyle {
            x: 0.0,
            y: 0.0,
            // None means the full viewport width, so the tint runs to the
            // right edge rather than stopping at the end of the text.
            width: None,
            height: theme::CODE_LINE_HEIGHT as f64,
            bg_color: Some(bg),
            under_line: None,
            wave_line: None,
        });
    }

    fn weight(&self, _edid: EditorId, _line: usize) -> FontWeight {
        FontWeight::NORMAL
    }
}
