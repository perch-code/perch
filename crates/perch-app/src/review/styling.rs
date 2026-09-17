//! Telling the editor how to draw a file.
//!
//! Two hooks do all the work. `apply_attr_styles` colours the text from
//! tree-sitter spans, and `apply_layout_styles` paints the diff background
//! behind a whole line. Both are handed a *display* line, so anything that
//! needs a real file position goes through the buffer's row table first.

use std::borrow::Cow;
use std::sync::Arc;

use floem::peniko::Color;
use floem::text::{Attrs, AttrsList, FamilyOwned, FontWeight};
use floem::views::editor::id::EditorId;
use floem::views::editor::layout::{LineExtraStyle, TextLayoutLine};
use floem::views::editor::text::Styling;
use floem::views::editor::EditorStyle;
use perch_git::LineStatus;
use perch_syntax::Span;

use crate::review::model::LoadedFile;
use crate::theme;

pub struct PerchStyling {
    file: Arc<LoadedFile>,
    families: Vec<FamilyOwned>,
    /// Cache key. Bumped per loaded file so the editor drops layouts from the
    /// file before; without this a second file reuses the first one's colours.
    id: u64,
}

impl PerchStyling {
    pub fn new(file: Arc<LoadedFile>, id: u64) -> Self {
        Self {
            file,
            families: FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect(),
            id,
        }
    }

    /// Highlight spans for a display line, taken from whichever side of the
    /// diff that line came from.
    fn spans_for(&self, display_line: usize) -> &[Span] {
        match self.file.buffer.row(display_line) {
            Some(perch_git::Row::Head { head_line, .. }) => self.file.head_spans.line(head_line),
            Some(perch_git::Row::Deleted { base_line }) => self.file.base_spans.line(base_line),
            None => &[],
        }
    }

    fn background_for(&self, display_line: usize) -> Option<Color> {
        match self.file.buffer.row(display_line)? {
            perch_git::Row::Deleted { .. } => Some(theme::DIFF_DELETED_BG),
            perch_git::Row::Head { status, .. } => match status {
                LineStatus::Added => Some(theme::DIFF_ADDED_BG),
                LineStatus::Modified => Some(theme::DIFF_MODIFIED_BG),
                LineStatus::Unchanged => None,
            },
        }
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
        let deleted = self
            .file
            .buffer
            .row(line)
            .map(|r| r.is_deleted())
            .unwrap_or(false);

        // Removed code is context, not the thing under review, so it is
        // dimmed rather than fully coloured.
        if deleted {
            let span_len = self.file.buffer.text.lines().nth(line).map(str::len).unwrap_or(0);
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
