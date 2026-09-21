//! Markdown styling for the composer while it is being typed into.
//!
//! The editor asks for a line's attributes on every layout, so the whole
//! text is re-highlighted whenever it changes and cached until it does. A
//! question is a few lines; tree-sitter does it in microseconds.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use floem::text::{Attrs, AttrsList, FamilyOwned, FontStyle};
use floem::views::editor::id::EditorId;
use floem::views::editor::text::{Document, Styling};
use floem::views::editor::EditorStyle;
use perch_syntax::{capture_name, highlight_markdown, LineSpans};

use crate::theme;

pub struct ComposerStyling {
    doc: Rc<dyn Document>,
    families: Vec<FamilyOwned>,
    code_families: Vec<FamilyOwned>,
    cache: RefCell<(String, LineSpans)>,
    id: u64,
}

impl ComposerStyling {
    pub fn new(doc: Rc<dyn Document>, id: u64) -> Self {
        Self {
            doc,
            families: FamilyOwned::parse_list(theme::FONT_STACK).collect(),
            code_families: FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect(),
            cache: RefCell::new((String::new(), LineSpans::default())),
            id,
        }
    }

    fn spans_for(&self, line: usize, f: impl FnOnce(&[perch_syntax::Span])) {
        let text = self.doc.text().to_string();
        let mut cache = self.cache.borrow_mut();
        if cache.0 != text {
            let spans = highlight_markdown(&text);
            *cache = (text.clone(), LineSpans::new(&text, &spans));
        }
        f(cache.1.line(line));
    }
}

impl Styling for ComposerStyling {
    fn id(&self) -> u64 {
        self.id
    }

    fn font_size(&self, _edid: EditorId, _line: usize) -> usize {
        theme::TEXT_BODY as usize
    }

    fn line_height(&self, _edid: EditorId, _line: usize) -> f32 {
        theme::TEXT_BODY * 1.5
    }

    fn font_family(&self, _edid: EditorId, _line: usize) -> Cow<'_, [FamilyOwned]> {
        Cow::Borrowed(&self.families)
    }

    fn tab_width(&self, _edid: EditorId, _line: usize) -> usize {
        4
    }

    fn apply_attr_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        default: Attrs,
        attrs: &mut AttrsList,
    ) {
        self.spans_for(line, |spans| {
            for span in spans {
                if span.start >= span.end {
                    continue;
                }
                let mut a = default.clone();
                match capture_name(span.capture) {
                    "text.strong" | "text.title" => a = a.weight(theme::weight_strong()),
                    "text.emphasis" => a = a.font_style(FontStyle::Italic),
                    "text.literal" => {
                        a = a.family(&self.code_families).color(theme::SYNTAX_STRING)
                    }
                    "text.uri" | "text.reference" => a = a.color(theme::ACCENT),
                    "punctuation.special" | "punctuation.delimiter" => a = a.color(theme::FG_SUBTLE),
                    _ => continue,
                }
                attrs.add_span(span.start..span.end, a);
            }
        });
    }
}
