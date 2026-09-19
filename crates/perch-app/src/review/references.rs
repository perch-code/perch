//! Find references: the panel under the editor that lists where a symbol
//! is used, and the asking that opens it.
//!
//! The answer comes from rust-analyzer through the backend, which reads
//! each location's line on the way so the panel needs no file loads. The
//! rows are the changed list's rows with the line's text beside the line
//! number, grouped by file, and a file the scope changed carries the same
//! status letter here as in the rail.

use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::text::{Attrs, AttrsList, FamilyOwned, FontWeight, LineHeightValue};
use floem::views::{
    ClipExt, Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack, rich_text,
};
use perch_git::ChangeStatus;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{Reference, References, ReviewState};
use crate::review::OpenFile;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

// ---------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------

/// Ask about the symbol on a head line of the active file: the one at
/// `column` when given, else the first on the line. The panel opens at
/// once, empty, and fills when the answer comes back with the same token.
/// A column on nothing, such as a space or a bracket, asks nothing.
pub fn find(state: ReviewState, backend: &Backend, head_line: usize, column: Option<usize>) {
    let Some(file) = state.active_file() else { return };
    let Some(display_line) = file.buffer.display_line(head_line) else { return };
    let Some(text) = file.buffer.text.lines().nth(display_line) else { return };
    let found = match column {
        Some(column) => identifier_at(text, column),
        None => first_identifier(text),
    };
    let Some((start, symbol)) = found else { return };

    let token = state.next_token();
    state.references.set(Some(Arc::new(References {
        symbol: symbol.to_string(),
        locations: Vec::new(),
        elsewhere: 0,
        pending: true,
        token,
    })));
    backend.references(token, &file.path, head_line as u32, start as u32);
}

/// Find references from the keyboard. The symbol is the one under the
/// last click when that click was on the cursor's line, else the first on
/// the line: the editor's own cursor column is not reachable from the
/// window's key handler, and the change nav moves the line without a
/// click. A removed line has no place in the head file and asks nothing.
pub fn find_at_cursor(state: ReviewState, backend: &Backend) {
    let Some(file) = state.active_file() else { return };
    let Some(head_line) = file.buffer.head_line(state.cursor_line.get()) else { return };
    let column = state
        .last_position
        .get()
        .filter(|(line, _)| *line == head_line)
        .map(|(_, column)| column);
    find(state, backend, head_line, column);
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier around a byte column of a line, and the column it starts
/// at. The column is a caret's: between characters, so a column just past
/// the end of a word is still on it, the way the editor's cursor and the
/// language server both read it. A column on nothing but spaces and
/// punctuation finds nothing.
pub fn identifier_at(line: &str, column: usize) -> Option<(usize, &str)> {
    let column = column.min(line.len());
    if !line.is_char_boundary(column) {
        return None;
    }
    let before: usize = line[..column]
        .chars()
        .rev()
        .take_while(|c| is_identifier_char(*c))
        .map(char::len_utf8)
        .sum();
    let after: usize = line[column..]
        .chars()
        .take_while(|c| is_identifier_char(*c))
        .map(char::len_utf8)
        .sum();
    if before + after == 0 {
        return None;
    }
    let start = column - before;
    Some((start, &line[start..column + after]))
}

/// The first identifier on a line, and where it starts.
pub fn first_identifier(line: &str) -> Option<(usize, &str)> {
    let start = line.char_indices().find(|(_, c)| is_identifier_char(*c)).map(|(i, _)| i)?;
    identifier_at(line, start)
}

// ---------------------------------------------------------------------
// The answer, shaped for the panel
// ---------------------------------------------------------------------

/// The locations by file, files in the order they first appear. The
/// backend sorts them by path and line, so that is path order with each
/// file's lines ascending. Two references on one line are one entry: the
/// row shows the line once, with both occurrences emphasised.
pub fn group_by_file(locations: &[Reference]) -> Vec<(String, Vec<Reference>)> {
    let mut groups: Vec<(String, Vec<Reference>)> = Vec::new();
    for reference in locations {
        match groups.iter_mut().find(|(path, _)| *path == reference.path) {
            Some((_, lines)) => {
                if !lines.iter().any(|r| r.line == reference.line) {
                    lines.push(reference.clone());
                }
            }
            None => groups.push((reference.path.clone(), vec![reference.clone()])),
        }
    }
    groups
}

/// "12 in 4 files", and "and 3 elsewhere" when some were in a dependency
/// the panel cannot open. Empty while the answer is still on its way.
pub fn summary(references: &References) -> String {
    let groups = group_by_file(&references.locations);
    let lines: usize = groups.iter().map(|(_, lines)| lines.len()).sum();
    let here = match (lines, groups.len()) {
        (0, _) => String::new(),
        (n, 1) => format!("{n} in 1 file"),
        (n, files) => format!("{n} in {files} files"),
    };
    match (here.is_empty(), references.elsewhere) {
        (_, 0) => here,
        (true, n) => format!("{n} elsewhere"),
        (false, n) => format!("{here} and {n} elsewhere"),
    }
}

/// Byte ranges of `symbol` in `text` as a whole identifier, so `token`
/// in `next_token` is left alone.
pub fn occurrences(text: &str, symbol: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    if symbol.is_empty() {
        return out;
    }
    let mut from = 0;
    while let Some(i) = text[from..].find(symbol) {
        let start = from + i;
        let end = start + symbol.len();
        let before = text[..start].chars().next_back().is_some_and(is_identifier_char);
        let after = text[end..].chars().next().is_some_and(is_identifier_char);
        if !before && !after {
            out.push(start..end);
        }
        from = end;
    }
    out
}

/// One row of the panel's list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Row {
    /// One line while the answer is on its way, or when nothing here was
    /// found.
    Notice(String),
    /// A file's heading: the path and how many lines under it.
    File { path: String, count: usize, first: bool },
    /// A line of a file. Carries the symbol, so asking about another
    /// symbol on the same line draws the row again with the new emphasis.
    Location { path: String, line: usize, text: String, symbol: String },
}

fn rows(references: &References) -> Vec<Row> {
    let groups = group_by_file(&references.locations);
    if groups.is_empty() {
        let text = match references.elsewhere {
            _ if references.pending => "Looking\u{2026}".to_string(),
            0 => "None found.".to_string(),
            n => format!("None in this worktree; {n} elsewhere."),
        };
        return vec![Row::Notice(text)];
    }
    let mut out = Vec::new();
    for (i, (path, lines)) in groups.into_iter().enumerate() {
        out.push(Row::File { path, count: lines.len(), first: i == 0 });
        out.extend(lines.into_iter().map(|r| Row::Location {
            path: r.path,
            line: r.line,
            text: r.text,
            symbol: references.symbol.clone(),
        }));
    }
    out
}

// ---------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------

/// The panel under the editor, while there is an answer to show.
pub fn panel(state: ReviewState, open_file: OpenFile) -> impl IntoView {
    // Keyed on whether there is a panel, not on what is in it: the answer
    // landing must not rebuild the handle under a drag.
    let open = Memo::new(move |_| state.references.with(Option::is_some));
    dyn_container(
        move || open.get(),
        move |open| {
            if open {
                body(state, open_file.clone()).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.width_full().flex_shrink(0.0_f32))
}

fn body(state: ReviewState, open_file: OpenFile) -> impl IntoView {
    let resizing = RwSignal::new(false);
    // The handle and the sheet are placed over the panel's edges, so the
    // panel is their positioning box.
    Stack::vertical((
        resize_handle(state, resizing),
        header(state),
        list(state, open_file),
        resize_cursor_sheet(resizing),
    ))
    .style(move |s| {
        s.width_full()
            .height(state.references_height.get())
            .flex_shrink(0.0_f32)
            .flex_col()
            .position(Position::Relative)
            .background(theme::CANVAS)
            .border_top(1.0)
            .border_color(theme::BORDER)
    })
}

/// "References to `symbol`", the count, and the close glyph.
fn header(state: ReviewState) -> impl IntoView {
    let symbol = move || state.references.with(|r| r.as_ref().map(|r| r.symbol.clone()).unwrap_or_default());
    let count = move || state.references.with(|r| r.as_ref().map(|r| summary(r)).unwrap_or_default());

    Stack::horizontal((
        Label::derived(|| "References to".to_string()).style(|s| {
            s.font_size(theme::TEXT_META).color(theme::FG_MUTED).selectable(false).flex_shrink(0.0_f32)
        }),
        Label::derived(symbol).style(|s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .margin_left(5.0)
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
        }),
        Label::derived(count).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_MUTED)
                .margin_left(10.0)
                .flex_grow(1.0_f32)
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
        }),
        close_button(state),
    ))
    .style(|s| {
        s.height(theme::REFERENCES_HEADER_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(12.0)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn close_button(state: ReviewState) -> impl IntoView {
    Container::new(sidebar_glyph(SidebarIcon::Close, theme::FG_SUBTLE).style(|s| s.size(11.0, 11.0)))
        .on_event_stop(el::Click, move |_, _| state.references.set(None))
        .style(|s| {
            clickable(s)
                .size(18.0, 18.0)
                .items_center()
                .justify_center()
                .border_radius(3.0)
                .hover(|s| s.background(theme::BADGE_BG))
        })
}

fn list(state: ReviewState, open_file: OpenFile) -> impl IntoView {
    dyn_stack(
        move || state.references.with(|r| r.as_ref().map(|r| rows(r)).unwrap_or_default()),
        |row| row.clone(),
        move |row| match row {
            Row::Notice(text) => notice(text).into_any(),
            Row::File { path, count, first } => file_row(state, open_file.clone(), path, count, first).into_any(),
            Row::Location { path, line, text, symbol } => {
                location_row(state, open_file.clone(), path, line, text, symbol).into_any()
            }
        },
    )
    .style(|s| s.flex_col().width_full().padding_bottom(6.0))
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

fn notice(text: String) -> impl IntoView {
    Label::derived(move || text.clone()).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .selectable(false)
    })
}

/// A file's heading: the rail's marker and path, with the count of lines
/// under it. Click opens the file at the top.
fn file_row(state: ReviewState, open_file: OpenFile, path: String, count: usize, first: bool) -> impl IntoView {
    let (directory, name) = match path.rfind('/') {
        Some(i) => (path[..=i].to_string(), path[i + 1..].to_string()),
        None => (String::new(), path.clone()),
    };
    let status_path = path.clone();
    let status = move || state.status_of(&status_path);

    Stack::horizontal((
        dyn_container(status, |status| match status {
            Some(status) => status_letter(status).into_any(),
            // Keep the column, so the paths line up whether or not the
            // file changed.
            None => Empty::new()
                .style(|s| s.width(14.0).margin_right(6.0).flex_shrink(0.0_f32))
                .into_any(),
        }),
        Label::derived(move || directory.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
        }),
        Label::derived(move || name.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        Label::derived(move || count.to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG_SUBTLE)
                .margin_left(8.0)
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| open_file(&path))
    .style(move |s| {
        clickable(s)
            .height(theme::REFERENCES_GROUP_HEIGHT)
            .width_full()
            .items_center()
            .padding_left(10.0)
            .padding_right(12.0)
            // A hairline between groups, and none above the first: the
            // header's border is already there.
            .apply_if(!first, |s| s.border_top(1.0).border_color(theme::BORDER_MUTED))
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

/// The rail's status letter, drawn the same way here so a file reads the
/// same in both lists.
fn status_letter(status: ChangeStatus) -> impl IntoView {
    let color = match status {
        ChangeStatus::Added => theme::STATUS_ADDED,
        ChangeStatus::Modified | ChangeStatus::TypeChanged => theme::STATUS_MODIFIED,
        ChangeStatus::Deleted => theme::STATUS_DELETED,
        ChangeStatus::Renamed | ChangeStatus::Copied => theme::STATUS_RENAMED,
    };
    Label::derived(move || status.letter()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(color)
            .width(14.0)
            .flex_shrink(0.0_f32)
            .text_align(floem::text::Alignment::Center)
            .margin_right(6.0)
            .selectable(false)
    })
}

/// One line: its number, right-aligned in a column of its own, then its
/// text with the symbol emphasised. Click opens the file there.
fn location_row(
    state: ReviewState,
    open_file: OpenFile,
    path: String,
    line: usize,
    text: String,
    symbol: String,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || line.to_string()).style(|s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::CODE_FONT_SIZE)
                .color(theme::FG_SUBTLE)
                .width(theme::REFERENCES_LINE_WIDTH)
                .flex_shrink(0.0_f32)
                .text_align(floem::text::Alignment::Right)
                .selectable(false)
        }),
        line_text(text, symbol),
    ))
    .on_event_stop(el::Click, move |_, _| {
        // Opening the active file again would rebuild its editor for
        // nothing; the jump alone moves it.
        if state.active.with(|a| a.as_deref() != Some(path.as_str())) {
            open_file(&path);
        }
        state.jump_to_line.set(Some(line.saturating_sub(1)));
    })
    .style(|s| {
        clickable(s)
            .height(theme::REFERENCES_ROW_HEIGHT)
            .width_full()
            .items_center()
            .padding_left(10.0)
            .padding_right(12.0)
            .gap(12.0)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

/// The line's text in the code font, every whole-word occurrence of the
/// symbol in the accent colour and semibold. Rich text wraps, and a long
/// line must not; the clip is one row tall, so only the first line shows
/// and the row keeps its height.
fn line_text(text: String, symbol: String) -> impl IntoView {
    let (text, attrs) = layout(&text, &symbol);
    let again = (text.clone(), attrs.clone());
    rich_text(text, attrs, move || again.clone())
        .style(|s| s.width_full())
        .clip()
        .style(|s| s.height(theme::REFERENCES_ROW_HEIGHT).flex_grow(1.0_f32).min_width(0.0))
}

fn layout(text: &str, symbol: &str) -> (String, AttrsList) {
    let code: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let base = || {
        Attrs::new()
            .color(theme::FG)
            .font_size(theme::CODE_FONT_SIZE)
            .family(&code)
            .line_height(LineHeightValue::Pt(theme::REFERENCES_ROW_HEIGHT as f32))
    };
    let mut list = AttrsList::new(base());
    for range in occurrences(text, symbol) {
        list.add_span(range, base().weight(FontWeight::SEMI_BOLD).color(theme::ACCENT));
    }
    (text.to_string(), list)
}

// ---------------------------------------------------------------------
// Resizing
// ---------------------------------------------------------------------

/// The strip along the panel's top edge that drags it taller: the thread
/// column's handle turned sideways.
fn resize_handle(state: ReviewState, resizing: RwSignal<bool>) -> impl IntoView {
    use floem::event::DragConfig;

    let grab = RwSignal::new(0.0_f64);

    Empty::new()
        .on_event_stop(el::PointerDown, move |cx, e: &PointerButtonEvent| {
            grab.set(state.references_height.get());
            if let Some(pointer) = e.pointer.pointer_id {
                if pointer.is_primary_pointer() {
                    cx.request_pointer_capture(pointer);
                }
            }
        })
        .on_event_stop(el::GainedPointerCapture, |cx, token| {
            cx.start_drag(
                *token,
                DragConfig {
                    threshold: 1.0,
                    animation_duration: std::time::Duration::ZERO,
                    easing: std::rc::Rc::new(floem::easing::Linear),
                    custom_data: None,
                    track_targets: false,
                },
                false,
            );
        })
        .on_event_stop(el::DragStart, move |_, _: &floem::event::DragStartEvent| {
            resizing.set(true);
        })
        .on_event_stop(el::DragMove, move |_, e: &floem::event::DragMoveEvent| {
            // The panel's top edge moves with the pointer, so its height is
            // what it was at the grab plus how far up the pointer went.
            let dy = e.start_state.logical_point().y - e.current_state.logical_point().y;
            let height = (grab.get() + dy).clamp(theme::REFERENCES_MIN_HEIGHT, theme::REFERENCES_MAX_HEIGHT);
            state.references_height.set(height);
        })
        .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
            resizing.set(false);
        })
        .on_event_stop(el::DragCancel, move |_, _: &floem::event::DragCancelEvent| {
            resizing.set(false);
        })
        .style(|s| {
            s.position(Position::Absolute)
                .inset_left(0.0)
                .inset_right(0.0)
                .inset_top(-4.0)
                .height(7.0)
                .z_index(10)
                .hover(|s| s.cursor(CursorStyle::RowResize))
        })
}

/// A sheet over the editor while the handle is being dragged, so the
/// cursor stays the row-resize one. The cursor belongs to whatever is
/// under the pointer, and on a fast drag, or past either height limit,
/// that is the code above or a row below rather than the handle, which
/// is only there once layout has caught up. The sheet reaches up as far
/// as the panel can grow, which is as far as the pointer can take it.
fn resize_cursor_sheet(resizing: RwSignal<bool>) -> impl IntoView {
    Empty::new().style(move |s| {
        s.position(Position::Absolute)
            .inset_left(0.0)
            .inset_right(0.0)
            .inset_bottom(0.0)
            .inset_top(-theme::REFERENCES_MAX_HEIGHT)
            .z_index(100)
            .cursor(CursorStyle::RowResize)
            .apply_if(!resizing.get(), |s| s.hide())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(path: &str, line: usize, character: usize, text: &str) -> Reference {
        Reference { path: path.into(), line, character, text: text.into() }
    }

    #[test]
    fn the_identifier_under_a_caret_is_found_from_either_end_and_inside() {
        let line = "let next_token = state.next_token();";
        assert_eq!(identifier_at(line, 4), Some((4, "next_token")), "at the start");
        assert_eq!(identifier_at(line, 14), Some((4, "next_token")), "just past the end");
        assert_eq!(identifier_at(line, 9), Some((4, "next_token")), "inside");
        assert_eq!(identifier_at(line, 23), Some((23, "next_token")), "after a dot");
        assert_eq!(identifier_at(line, 0), Some((0, "let")));
    }

    #[test]
    fn a_caret_on_punctuation_or_past_the_line_finds_nothing_or_the_last_word() {
        assert_eq!(identifier_at("a + b", 2), None, "on the plus");
        assert_eq!(identifier_at("foo(bar)", 8), None, "after the closing bracket");
        assert_eq!(identifier_at("   ", 1), None);
        assert_eq!(identifier_at("", 0), None);
        // A caret past the end is treated as at the end.
        assert_eq!(identifier_at("foo", 99), Some((0, "foo")));
    }

    #[test]
    fn identifiers_are_bytes_so_wide_characters_do_not_split_them() {
        let line = "let café = 1;";
        assert_eq!(identifier_at(line, 6), Some((4, "café")));
        // A column inside the two-byte é is not a caret position.
        assert_eq!(identifier_at(line, 8), None);
    }

    #[test]
    fn the_first_identifier_skips_leading_punctuation() {
        assert_eq!(first_identifier("    (foo, bar)"), Some((5, "foo")));
        assert_eq!(first_identifier("// note"), Some((3, "note")));
        assert_eq!(first_identifier("}"), None);
    }

    #[test]
    fn locations_are_grouped_by_file_in_the_order_they_arrive() {
        let locations = vec![
            reference("a.rs", 3, 0, "x"),
            reference("a.rs", 9, 0, "y"),
            reference("b/c.rs", 1, 0, "z"),
        ];
        let groups = group_by_file(&locations);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "a.rs");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "b/c.rs");
        assert_eq!(groups[1].1, vec![reference("b/c.rs", 1, 0, "z")]);
    }

    #[test]
    fn two_references_on_one_line_make_one_row() {
        let locations = vec![reference("a.rs", 3, 4, "foo(foo)"), reference("a.rs", 3, 8, "foo(foo)")];
        let groups = group_by_file(&locations);
        assert_eq!(groups[0].1.len(), 1);
        let rows = rows(&References { symbol: "foo".into(), locations, elsewhere: 0, pending: false, token: 0 });
        assert_eq!(rows.len(), 2, "a heading and one line");
        assert_eq!(rows[0], Row::File { path: "a.rs".into(), count: 1, first: true });
    }

    #[test]
    fn the_summary_counts_lines_files_and_the_rest() {
        let one = References { symbol: "s".into(), locations: vec![reference("a.rs", 1, 0, "")], elsewhere: 0, pending: false, token: 0 };
        assert_eq!(summary(&one), "1 in 1 file");

        let many = References {
            symbol: "s".into(),
            locations: vec![
                reference("a.rs", 1, 0, ""),
                reference("a.rs", 2, 0, ""),
                reference("b.rs", 1, 0, ""),
            ],
            elsewhere: 3,
            pending: false,
            token: 0,
        };
        assert_eq!(summary(&many), "3 in 2 files and 3 elsewhere");

        let looking = References { symbol: "s".into(), locations: vec![], elsewhere: 0, pending: true, token: 0 };
        assert_eq!(summary(&looking), "");
        assert_eq!(rows(&looking), vec![Row::Notice("Looking\u{2026}".into())]);

        let none = References { symbol: "s".into(), locations: vec![], elsewhere: 0, pending: false, token: 0 };
        assert_eq!(rows(&none), vec![Row::Notice("None found.".into())]);

        let away = References { symbol: "s".into(), locations: vec![], elsewhere: 2, pending: false, token: 0 };
        assert_eq!(summary(&away), "2 elsewhere");
    }

    #[test]
    fn only_whole_words_are_emphasised() {
        assert_eq!(occurrences("let token = next_token(token);", "token"), vec![4..9, 23..28]);
        assert!(occurrences("no match here", "token").is_empty());
        assert!(occurrences("x", "").is_empty());
        assert!(occurrences("aaa", "aa").is_empty(), "one identifier, not two");
    }

    #[test]
    fn rows_carry_the_symbol_so_a_new_question_redraws_them() {
        let locations = vec![reference("a.rs", 1, 0, "a b")];
        let a = rows(&References { symbol: "a".into(), locations: locations.clone(), elsewhere: 0, pending: false, token: 0 });
        let b = rows(&References { symbol: "b".into(), locations, elsewhere: 0, pending: false, token: 0 });
        assert_ne!(a[1], b[1]);
    }
}

/// The panel in a headless window: the keys that open and close it, the
/// handle, and a row.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::{Code, KeyboardEvent, Modifiers};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::{Item, ItemCheckout, ItemSubject, PullRequest};

    use super::*;
    use crate::state::AppState;

    const WIDTH: f64 = 1400.0;
    const HEIGHT: f64 = 900.0;

    /// The whole window on Files, with one file open and loaded, and the
    /// thread column closed so the editor has the width.
    fn app_harness() -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let pr = PullRequest {
            number: 812,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "f".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        };
        let now = "2026-09-18T09:59:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(crate::review::model::Phase::Ready);
        state.tab.set(crate::review::model::ItemTab::Files);
        state.pull_request.set(Some(Arc::new(pr)));
        state.composer_autofocus.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, WIDTH, HEIGHT);
        harness.rebuild();
        harness.paint();

        // One rebuild per write, as the window gets.
        state.open_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        state.finish_load("src/x.rs", Arc::new(loaded("src/x.rs")));
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    fn loaded(path: &str) -> crate::review::model::LoadedFile {
        crate::backend::build_loaded_file(
            path.into(),
            "fn alpha() {}\nfn beta() {\n    alpha();\n}\n".to_string(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        )
    }

    fn key(code: Code, modifiers: Modifiers) -> floem::event::Event {
        let key = match code {
            Code::Escape => floem::prelude::Key::Named(floem::prelude::NamedKey::Escape),
            _ => floem::prelude::Key::Named(floem::prelude::NamedKey::F12),
        };
        floem::event::Event::Key(KeyboardEvent {
            state: floem::prelude::KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    fn answered(symbol: &str) -> Arc<References> {
        let reference = |path: &str, line: usize, text: &str| Reference {
            path: path.into(),
            line,
            character: 0,
            text: text.into(),
        };
        Arc::new(References {
            symbol: symbol.into(),
            locations: vec![
                reference("src/x.rs", 1, "fn alpha() {}"),
                reference("src/x.rs", 3, "alpha();"),
                reference("src/y.rs", 7, "let a = alpha();"),
            ],
            elsewhere: 1,
            pending: false,
            token: 0,
        })
    }

    /// Where the panel's top edge sits: the pane ends above the status
    /// bar, and the panel is its own height up from there.
    fn panel_top() -> f64 {
        HEIGHT - crate::theme::STATUS_HEIGHT - crate::theme::REFERENCES_HEIGHT
    }

    #[test]
    fn shift_f12_opens_the_panel_on_the_cursors_line_and_escape_closes_it() {
        let (mut harness, state) = app_harness();
        assert!(state.references.get().is_none());

        // Nothing has been clicked, so the first identifier on line 0.
        harness.dispatch_event(key(Code::F12, Modifiers::SHIFT));
        harness.rebuild();
        harness.paint();
        let open = state.references.get().expect("the panel opened");
        assert_eq!(open.symbol, "fn");
        assert!(open.locations.is_empty(), "empty until the answer lands");

        // The answer lands, and the panel draws its rows.
        state.references.set(Some(answered("alpha")));
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(key(Code::Escape, Modifiers::empty()));
        harness.rebuild();
        harness.paint();
        assert!(state.references.get().is_none(), "escape did not close the panel");
    }

    #[test]
    fn a_click_on_a_line_is_asked_about_from_the_keyboard() {
        let (mut harness, state) = app_harness();
        // As the editor's cursor effect leaves them after a click on
        // `beta` in line 1.
        state.cursor_line.set(1);
        state.last_position.set(Some((1, 4)));
        harness.dispatch_event(key(Code::F12, Modifiers::SHIFT));
        assert_eq!(state.references.get().map(|r| r.symbol.clone()).as_deref(), Some("beta"));

        // The change nav moved the cursor line without a click: the
        // click's column is not that line's, so its first identifier.
        state.references.set(None);
        state.cursor_line.set(2);
        harness.dispatch_event(key(Code::F12, Modifiers::SHIFT));
        assert_eq!(state.references.get().map(|r| r.symbol.clone()).as_deref(), Some("alpha"));
    }

    #[test]
    fn escape_leaves_the_panel_to_an_open_composer() {
        let (mut harness, state) = app_harness();
        state.references.set(Some(answered("alpha")));
        state.open_composer(None, None, String::new(), (perch_agent::Backend::ClaudeCode, "sonnet".into(), perch_agent::Effort::Medium));
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(key(Code::Escape, Modifiers::empty()));
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none(), "the composer takes the first escape");
        assert!(state.references.get().is_some(), "and the panel stays");
    }

    #[test]
    fn the_handle_drags_the_panel_taller_within_its_limits() {
        let (mut harness, state) = app_harness();
        state.references.set(Some(answered("alpha")));
        harness.rebuild();
        harness.paint();

        let x = 800.0;
        let y = panel_top();
        harness.pointer_down(x, y);
        harness.pointer_move(x, y - 40.0);
        harness.rebuild();
        harness.paint();
        harness.pointer_move(x, y - 80.0);
        harness.rebuild();
        harness.paint();
        harness.pointer_up(x, y - 80.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.references_height.get(), crate::theme::REFERENCES_HEIGHT + 80.0);

        // Past the limit, it stops at the limit.
        let y = panel_top() - 80.0;
        harness.pointer_down(x, y);
        harness.pointer_move(x, y - 2000.0);
        harness.rebuild();
        harness.paint();
        harness.pointer_up(x, y - 2000.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.references_height.get(), crate::theme::REFERENCES_MAX_HEIGHT);
    }

    #[test]
    fn a_row_opens_its_file() {
        let (mut harness, state) = app_harness();
        state.references.set(Some(answered("alpha")));
        harness.rebuild();
        harness.paint();

        // Under the border and the header: x.rs's heading, its two lines,
        // y.rs's heading, then its line.
        let rows_top = panel_top() + 1.0 + crate::theme::REFERENCES_HEADER_HEIGHT;
        let y = rows_top
            + 2.0 * crate::theme::REFERENCES_GROUP_HEIGHT
            + 2.0 * crate::theme::REFERENCES_ROW_HEIGHT
            + crate::theme::REFERENCES_ROW_HEIGHT / 2.0;
        let x = 800.0;
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            state.active.get().as_deref(),
            Some("src/y.rs"),
            "click at ({x}, {y}) over {under:?} did not open the file"
        );
        assert!(state.references.get().is_some(), "the panel stays open");
    }
}
