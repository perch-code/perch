//! The tour's text: plain text with `code` spans in backticks, drawn in
//! the prose font with the spans in the code font. Also the small pieces
//! of text and the row look that every part of the tour shares.

use std::ops::Range;

use floem::peniko::Color;
use floem::prelude::*;
use floem::style::Style;
use floem::text::{Attrs, AttrsList, FamilyOwned};
use floem::views::{Container, Decorators, Label};

use crate::review::linked_text::{linked_line, linked_text};
use crate::theme;
use crate::ui::widgets::{clickable, underlined_link};

/// How a piece of text is set.
#[derive(Clone, Copy)]
pub struct Ink {
    pub size: f32,
    pub weight: fn() -> floem::text::FontWeight,
    pub color: fn() -> Color,
}

/// Body text.
pub const BODY: Ink = Ink {
    size: theme::TEXT_BODY,
    weight: theme::weight_read,
    color: theme::fg,
};

/// Body text, quieter.
pub const MUTED: Ink = Ink {
    size: theme::TEXT_BODY,
    weight: theme::weight_read,
    color: theme::fg_muted,
};

/// Body text, heavier: a question, a flag's title, an element's.
pub const STRONG: Ink = Ink {
    size: theme::TEXT_BODY,
    weight: theme::weight_emphasis,
    color: theme::fg,
};

/// A stop's title.
pub const TITLE: Ink = Ink {
    size: theme::TEXT_SECTION_TITLE,
    weight: theme::weight_strong,
    color: theme::fg,
};

/// A chapter's title.
pub const HEADING: Ink = Ink {
    size: 20.0,
    weight: theme::weight_strong,
    color: theme::fg,
};

/// Text with `code` spans, wrapped to the width it has.
pub fn paragraph(text: &str, ink: Ink) -> impl IntoView {
    let (plain, code) = backticks(text);
    let len = plain.len();
    linked_text(plain, Vec::new(), move || {
        (attrs(ink, len, &code), Vec::new())
    })
    .style(|s| s.width_full())
}

/// Text with `code` spans on one line, cut short where it runs out of
/// room.
pub fn line(text: &str, ink: Ink) -> impl IntoView {
    let (plain, code) = backticks(text);
    let len = plain.len();
    linked_line(plain, Vec::new(), move || {
        (attrs(ink, len, &code), Vec::new())
    })
    .style(|s| s.min_width(0.0).flex_shrink(1.0_f32))
}

/// The text without its backticks, and the byte ranges that were in
/// them. An unclosed backtick stays.
fn backticks(text: &str) -> (String, Vec<Range<usize>>) {
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    let mut open: Option<usize> = None;
    for ch in text.chars() {
        if ch == '`' {
            match open.take() {
                Some(start) => code.push(start..plain.len()),
                None => open = Some(plain.len()),
            }
            continue;
        }
        plain.push(ch);
    }
    if let Some(start) = open {
        plain.insert(start, '`');
    }
    (plain, code)
}

/// The text without its backticks, for a place that draws plain text.
pub fn plain(text: &str) -> String {
    backticks(text).0
}

/// A small muted label: a count, a heading over a list.
pub fn meta(text: impl std::fmt::Display) -> Label {
    Label::new(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_muted())
            .selectable(false)
    })
}

/// A small link that does `go` when clicked.
pub fn link(text: impl std::fmt::Display, go: impl Fn() + 'static) -> Container {
    Container::new(Label::new(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::link())
            .selectable(false)
    }))
    .on_event_stop(floem::event::listener::Click, move |_, _| go())
    .style(|s| clickable(underlined_link(s)))
}

/// A row that goes somewhere when clicked, lit under the pointer.
pub fn hover_row(s: Style) -> Style {
    clickable(s.width_full().border_radius(theme::RADIUS))
        .hover(|s| s.background(theme::canvas_hover()))
}

fn attrs(ink: Ink, len: usize, code: &[Range<usize>]) -> AttrsList {
    let prose: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::FONT_STACK).collect();
    let mono: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let mut list = AttrsList::new(
        Attrs::new()
            .family(&prose)
            .font_size(ink.size)
            .weight((ink.weight)())
            .line_height(floem::text::LineHeightValue::Normal(
                theme::PROSE_LINE_HEIGHT,
            ))
            .color((ink.color)()),
    );
    // Code a size under the prose's, as `PROSE_CODE_FONT_SIZE` is under
    // the body's.
    let code_size = ink.size - (theme::TEXT_BODY - theme::PROSE_CODE_FONT_SIZE);
    for range in code.iter().filter(|r| r.end <= len) {
        list.add_span(
            range.clone(),
            Attrs::new()
                .family(&mono)
                .font_size(code_size)
                .weight((ink.weight)())
                .color((ink.color)()),
        );
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backticks_become_code_spans() {
        let (plain, code) = backticks("a `b` c");
        assert_eq!(plain, "a b c");
        assert_eq!((code.len(), code[0].clone()), (1, 2..3));
        assert_eq!(
            backticks("one ` alone"),
            ("one ` alone".to_string(), vec![])
        );
    }
}
