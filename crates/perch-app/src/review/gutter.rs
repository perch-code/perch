//! The diff gutter.
//!
//! floem's built-in gutter renders `line + 1` and offers no extension point,
//! which is wrong here twice over: the buffer is a spliced diff, so a display
//! line is not a file line, and a reviewer needs the change bars. So the
//! built-in one is hidden and this view is drawn beside the editor.
//!
//! Line wrapping is turned off on the editor, which makes the geometry exact:
//! display line N sits at `N * line_height`. That means this view needs only
//! the viewport to place itself, and one reactive offset moves the whole
//! column rather than each row carrying a position of its own. An earlier
//! version gave every row an absolute `inset_top` captured when the row was
//! built; rows survive scrolling, so those positions went stale and the
//! numbers piled up on each other.

use std::sync::Arc;

use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet};
use floem::style::Position;
use floem::views::{ClipExt, Container, Decorators, Empty, Label, Stack, dyn_stack};
use perch_git::{LineStatus, Row};

use crate::review::model::LoadedFile;
use crate::theme;

/// Extra rows rendered past the viewport, so a partially visible line at
/// either edge is still numbered.
const OVERSCAN: usize = 2;

/// Which display lines fall inside the viewport.
///
/// Pure, because this is the arithmetic most likely to be wrong and the
/// hardest to see wrong: an off-by-one here shifts every number in the
/// gutter against the code beside it.
fn visible_lines(scroll_top: f64, height: f64, line_height: f64, total: usize) -> Vec<usize> {
    if height <= 0.0 || line_height <= 0.0 || total == 0 {
        return Vec::new();
    }
    let first = (scroll_top / line_height).floor().max(0.0) as usize;
    if first >= total {
        return Vec::new();
    }
    let count = (height / line_height).ceil() as usize + OVERSCAN;
    (first..(first + count).min(total)).collect()
}

/// How far the column slides to line up with a part-scrolled first row.
fn column_offset(scroll_top: f64, line_height: f64) -> f64 {
    if line_height <= 0.0 {
        return 0.0;
    }
    -(scroll_top.rem_euclid(line_height))
}

pub fn diff_gutter(file: Arc<LoadedFile>, viewport: RwSignal<Rect>) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;
    let total = file.buffer.len();
    let rows = file.clone();

    let visible = move || {
        let vp = viewport.get();
        visible_lines(vp.y0, vp.height(), line_height, total)
    };

    let column = dyn_stack(visible, |line| *line, move |line| gutter_row(rows.clone(), line))
        .style(move |s| {
            // The whole column slides by the sub-line remainder of the
            // scroll. Reading the viewport here, in a style closure, is what
            // keeps it live: style closures re-run when their signals change,
            // whereas a value captured when a row was built does not.
            let offset = column_offset(viewport.get().y0, line_height);
            s.absolute()
                .inset_top(offset)
                .width_full()
                .flex_col()
        });

    Container::new(column)
        // This inner box is what the column positions against, so it needs
        // the size and the relative position. Styling the clip wrapper
        // instead would leave it shrinking to its content.
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        .style(|s| {
            s.width(theme::GUTTER_WIDTH)
                .height_full()
                .flex_shrink(0.0_f32)
                .background(theme::GUTTER_BG)
                .border_right(1.0)
                .border_color(theme::BORDER_MUTED)
        })
}

fn gutter_row(file: Arc<LoadedFile>, line: usize) -> impl IntoView {
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
                    .selectable(false)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::CODE_FONT_SIZE - 1.0)
                    .color(if dimmed { theme::DIFF_DELETED_BAR } else { theme::GUTTER_FG })
                    .text_align(floem::text::Alignment::End)
                    .padding_right(9.0)
            },
        ),
        change_bar(bar),
    ))
    .style(move |s| {
        s.width_full()
            .height(theme::CODE_LINE_HEIGHT as f64)
            .flex_shrink(0.0_f32)
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

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 19.0;

    #[test]
    fn an_unscrolled_viewport_starts_at_the_first_line() {
        let lines = visible_lines(0.0, 190.0, H, 1000);
        assert_eq!(lines[0], 0);
        // Ten lines fit, plus the overscan that covers a partial row.
        assert_eq!(lines.len(), 10 + OVERSCAN);
    }

    #[test]
    fn scrolling_a_whole_number_of_lines_shifts_the_first_line() {
        let lines = visible_lines(H * 5.0, 190.0, H, 1000);
        assert_eq!(lines[0], 5);
        assert_eq!(column_offset(H * 5.0, H), 0.0, "no sub-line remainder");
    }

    #[test]
    fn a_part_scrolled_line_stays_visible_and_the_column_slides_up() {
        // Two thirds of the way through line 5.
        let scroll = H * 5.0 + 12.0;
        let lines = visible_lines(scroll, 190.0, H, 1000);
        assert_eq!(lines[0], 5, "the partly visible line is still numbered");
        assert_eq!(column_offset(scroll, H), -12.0);
    }

    #[test]
    fn the_last_screen_does_not_run_past_the_end_of_the_file() {
        let total = 20;
        let lines = visible_lines(H * 15.0, 190.0, H, total);
        assert_eq!(*lines.last().unwrap(), total - 1);
        assert!(lines.iter().all(|l| *l < total));
    }

    #[test]
    fn scrolling_past_the_end_yields_nothing_rather_than_panicking() {
        assert!(visible_lines(H * 500.0, 190.0, H, 20).is_empty());
    }

    #[test]
    fn a_collapsed_viewport_renders_no_rows() {
        assert!(visible_lines(0.0, 0.0, H, 100).is_empty());
        assert!(visible_lines(0.0, 190.0, H, 0).is_empty());
    }

    #[test]
    fn degenerate_line_heights_do_not_divide_by_zero() {
        assert!(visible_lines(0.0, 190.0, 0.0, 100).is_empty());
        assert_eq!(column_offset(50.0, 0.0), 0.0);
    }

    #[test]
    fn the_offset_is_never_a_whole_line() {
        // Otherwise the column would be shifted by a full row and every
        // number would sit against the wrong line.
        for scroll in [0.0, 1.0, 18.9, 19.0, 100.0, 1234.5] {
            let offset = column_offset(scroll, H);
            assert!(offset > -H && offset <= 0.0, "scroll {scroll} gave {offset}");
        }
    }

    #[test]
    fn every_visible_line_is_consecutive() {
        let lines = visible_lines(H * 7.5, 400.0, H, 1000);
        for pair in lines.windows(2) {
            assert_eq!(pair[1], pair[0] + 1);
        }
    }
}
