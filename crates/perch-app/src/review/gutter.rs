//! The diff gutter.
//!
//! floem's built-in gutter renders `line + 1` and offers no extension point,
//! which is wrong here twice over: the buffer is a spliced diff, so a display
//! line is not a file line, and a reviewer needs the change bars. So the
//! built-in one is hidden and this view is placed beside the editor, tracking
//! the same `screen_lines` signal the editor lays out from.

use std::sync::Arc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet};
use floem::style::Position;
use floem::views::editor::view::ScreenLines;
use floem::views::{Container, Decorators, Empty, Label, Stack, dyn_stack};
use perch_git::{LineStatus, Row};

use crate::review::model::LoadedFile;
use crate::theme;

/// Line numbers and change bars, positioned to match the editor's own layout.
pub fn diff_gutter(
    file: Arc<LoadedFile>,
    screen_lines: RwSignal<ScreenLines>,
    viewport: RwSignal<floem::kurbo::Rect>,
) -> impl IntoView {
    let rows = file.clone();

    dyn_stack(
        move || {
            // floem places these y values in document space; the editor's own
            // gutter subtracts the viewport origin, and so must this one.
            let top = viewport.get().y0;
            screen_lines
                .get()
                .iter_lines_y()
                .map(|(line, y)| (line, y - top))
                .collect::<Vec<_>>()
        },
        |(line, _)| *line,
        move |(line, y)| gutter_row(rows.clone(), line, y),
    )
    .style(|s| {
        s.width(theme::GUTTER_WIDTH)
            .height_full()
            .position(Position::Relative)
            .background(theme::GUTTER_BG)
            .border_right(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

fn gutter_row(file: Arc<LoadedFile>, line: usize, y: f64) -> impl IntoView {
    let row = file.buffer.row(line);

    // A ghost line shows the base file's number; a real line shows its own.
    // Both are one-based, as every other tool displays them.
    let number = match row {
        Some(Row::Head { head_line, .. }) => Some(head_line + 1),
        Some(Row::Deleted { base_line }) => Some(base_line + 1),
        None => None,
    };

    let bar = match row {
        Some(Row::Deleted { .. }) => Some(theme::DIFF_DELETED_BAR),
        Some(Row::Head { status, .. }) => match status {
            LineStatus::Added => Some(theme::DIFF_ADDED_BAR),
            LineStatus::Modified => Some(theme::DIFF_MODIFIED_BAR),
            LineStatus::Unchanged => None,
        },
        None => None,
    };

    let dimmed = matches!(row, Some(Row::Deleted { .. }));

    Stack::horizontal((
        Label::derived(move || number.map(|n| n.to_string()).unwrap_or_default()).style(
            move |s| {
                s.flex_grow(1.0_f32)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::CODE_FONT_SIZE - 0.5)
                    .color(if dimmed { theme::DIFF_DELETED_BAR } else { theme::GUTTER_FG })
                    .text_align(floem::text::Alignment::End)
                    .padding_right(9.0)
            },
        ),
        change_bar(bar),
    ))
    .style(move |s| {
        s.absolute()
            .inset_top(y)
            .width_full()
            .height(theme::CODE_LINE_HEIGHT as f64)
            .items_center()
    })
}

fn change_bar(color: Option<Color>) -> impl IntoView {
    Container::new(Empty::new().style(move |s| {
        let base = s.width(3.0).height_full();
        match color {
            Some(c) => base.background(c),
            None => base,
        }
    }))
    .style(|s| s.width(3.0).height_full().flex_shrink(0.0_f32))
}
