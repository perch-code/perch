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
//!
//! A split has a gutter on each side, over that side's rows: base numbers
//! on the left, head numbers on the right, and nothing on a filler. Only
//! the right one carries thread markers and the `+`, because only the head
//! can be anchored on.
//!
//! The rows are the document's, not the buffer's: a row that is a line is
//! numbered and barred from the buffer's row table, a divider is tinted as
//! one, and a spacer under a card is blank.

use std::sync::Arc;

use floem::event::PointerScrollEventExt;
use floem::kurbo::{Rect, Size, Vec2};
use floem::peniko::Color;
use floem::prelude::*;
use floem::AnyView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{ClipExt, Container, Decorators, Empty, Label, Stack, dyn_container, dyn_stack};
use perch_git::Row;

use crate::icons::SidebarIcon;
use crate::review::doc::{Doc, DocRow, Tint};
use crate::review::editor::Side;
use crate::review::model::{LoadedFile, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::sidebar_glyph;

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

/// The gutter of the stacked layout, over the document the editor beside
/// it is laying out.
pub fn diff_gutter(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
) -> impl IntoView {
    column(viewport, scroll_delta, doc, move |row| gutter_row(state, app, file.clone(), doc, row))
}

/// One side of a split. `scroll_delta` is the editor a wheel over the
/// gutter should move, which for the left is the right's: one scroll.
pub fn side_gutter(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    side: Side,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
) -> impl IntoView {
    column(viewport, scroll_delta, doc, move |row| side_row(state, app, file.clone(), side, doc, row))
}

/// The rows in view, slid to the scroll, in a column beside the editor.
fn column<V: IntoView + 'static>(
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    doc: RwSignal<Arc<Doc>>,
    row: impl Fn(usize) -> V + 'static,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;

    let visible = move || {
        let vp = viewport.get();
        // The document's length is read here, tracked, so a document that
        // grows or shrinks under the editor moves the column's end with it.
        let total = doc.with(|d| d.len());
        visible_lines(vp.y0, vp.height(), line_height, total)
    };

    let column = dyn_stack(visible, |line| *line, row)
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
        // The wheel over the numbers scrolls the code, as it does over the
        // code itself. Down the file only: nothing here goes sideways.
        .on_event_stop(el::PointerWheel, move |_, wheel| {
            let size = viewport.get_untracked().size();
            let delta = wheel.resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
            scroll_delta.set(Vec2::new(0.0, -delta.y));
        })
        .style(|s| {
            s.width(theme::GUTTER_WIDTH)
                .height_full()
                .flex_shrink(0.0_f32)
                .background(theme::GUTTER_BG)
                .border_right(1.0)
                .border_color(theme::BORDER_MUTED)
        })
}

/// What a row's left slot can hold: a thread marker or a `+` for a row
/// that is a display line of the file, or nothing at all.
#[derive(Clone, Copy)]
enum Slot {
    Anchor(usize),
    Blank,
}

/// A row of the stacked layout's gutter.
fn gutter_row(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    row: usize,
) -> AnyView {
    let entry = doc.with_untracked(|d| d.rows.get(row).cloned());
    let Some(DocRow::Line { display, tint }) = entry else {
        return other_row(state, app, file, row, entry);
    };
    let buffer_row = file.buffer.row(display);

    // A ghost line shows the base file's number; a real line shows its own.
    // Both are one-based, as every other tool displays them.
    let number = match buffer_row {
        Some(Row::Head { head_line, .. }) => Some(head_line + 1),
        Some(Row::Deleted { base_line }) => Some(base_line + 1),
        None => None,
    };
    let dimmed = matches!(buffer_row, Some(Row::Deleted { .. }));

    row_view(state, app, file, row, number, dimmed, change_bar_color(tint), Slot::Anchor(display))
}

/// A row of one side's gutter in a split.
fn side_row(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    side: Side,
    doc: RwSignal<Arc<Doc>>,
    row: usize,
) -> AnyView {
    let entry = doc.with_untracked(|d| d.rows.get(row).cloned());
    let Some(DocRow::Line { display, tint }) = entry else {
        return other_row(state, app, file, row, entry);
    };

    // The base side's document is over the base file, so its line is its
    // number; the head side's is over the unified buffer, and the number
    // is the head line displayed there. Anchors are on the head, so the
    // marker and the `+` live on the right, keyed by the display line.
    let (number, slot) = match side {
        Side::Base => (Some(display + 1), Slot::Blank),
        Side::Head => (file.buffer.head_line(display).map(|h| h + 1), Slot::Anchor(display)),
    };

    row_view(state, app, file, row, number, false, change_bar_color(tint), slot)
}

/// A row that is no line: a divider is a bar in its colour, and a filler,
/// a spacer or a row past the end is blank. None of them records the
/// pointer, so nothing lights a `+`.
fn other_row(state: ReviewState, app: AppState, file: Arc<LoadedFile>, row: usize, entry: Option<DocRow>) -> AnyView {
    match entry {
        Some(DocRow::Divider { .. }) => divider_row(),
        Some(DocRow::Line { .. } | DocRow::Filler | DocRow::Spacer { .. }) | None => {
            row_view(state, app, file, row, None, false, None, Slot::Blank)
        }
    }
}

/// The gutter's row beside a divider: the whole width in the divider's
/// colour, with no number and no bar.
fn divider_row() -> AnyView {
    Empty::new()
        .style(|s| {
            s.width_full()
                .height(theme::CODE_LINE_HEIGHT as f64)
                .flex_shrink(0.0_f32)
                .background(theme::DIFF_DIVIDER_BG)
        })
        .into_any()
}

fn change_bar_color(tint: Tint) -> Option<Color> {
    match tint {
        Tint::Added => Some(theme::DIFF_ADDED_BAR),
        Tint::Modified => Some(theme::DIFF_MODIFIED_BAR),
        Tint::Deleted => Some(theme::DIFF_DELETED_BAR),
        Tint::Plain => None,
    }
}

/// One row: the slot, the number, the change bar. `key` is what the
/// pointer's row is recorded as while it is here, in this gutter's own
/// numbering, and the slot shows its `+` when that matches. A row with a
/// blank slot records nothing: the two gutters of a split share the
/// signal, and a row on the left must not light a `+` on the right.
#[allow(clippy::too_many_arguments)]
fn row_view(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    key: usize,
    number: Option<usize>,
    dimmed: bool,
    bar: Option<Color>,
    slot: Slot,
) -> AnyView {
    let slot_view = match slot {
        Slot::Anchor(display_line) => anchor_slot(state, app, file, key, display_line).into_any(),
        Slot::Blank => Empty::new().into_any(),
    };

    let row = Stack::horizontal((
        // The left slot: a marker where a thread starts, a `+` under the
        // pointer, nothing otherwise. Fixed width, so numbers never shift.
        slot_view.style(|s| s.size(16.0, 16.0).margin_left(3.0).flex_shrink(0.0_f32)),
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
    });

    match slot {
        Slot::Anchor(_) => row
            .on_event_cont(el::PointerEnter, move |_, _| state.hovered_line.set(Some(key)))
            .on_event_cont(el::PointerLeave, move |_, _| {
                state.hovered_line.update(|h| {
                    if *h == Some(key) {
                        *h = None;
                    }
                })
            })
            .into_any(),
        Slot::Blank => row.into_any(),
    }
}

/// The marker for a thread starting on a display line, or the `+` that
/// starts one while the pointer is on the row.
fn anchor_slot(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    key: usize,
    display_line: usize,
) -> impl IntoView {
    let marker_file = file.clone();
    let github_file = file.clone();
    let plus_file = file;

    dyn_container(
        move || {
            let thread = state.thread_starting_at(&marker_file, display_line).map(|t| (t.id, t.outdated));
            let github = state.github_comment_starting_at(&github_file, display_line);
            let hovered = state.hovered_line.get() == Some(key);
            (thread, github, hovered)
        },
        move |(thread, github, hovered)| match (thread, github) {
            // A review comment from GitHub on this line, hollow where a
            // thread of the reader's own is solid. Click opens the column,
            // where it is listed under On GitHub.
            (None, Some(_)) => Container::new(
                sidebar_glyph(SidebarIcon::ThreadHollow, theme::FG_MUTED).style(|s| s.size(12.0, 12.0)),
            )
            .on_event_stop(el::Click, move |_, _| state.show_threads())
            .style(|s| {
                crate::ui::widgets::clickable(s).size(16.0, 16.0).items_center().justify_center()
            })
            .into_any(),
            (Some((id, outdated)), _) => Container::new(
                sidebar_glyph(
                    SidebarIcon::Thread,
                    if outdated { theme::FG_SUBTLE } else { theme::ACCENT },
                )
                .style(|s| s.size(12.0, 12.0)),
            )
            .on_event_stop(el::Click, move |_, _| state.focus_thread.set(Some(id.clone())))
            .style(|s| {
                crate::ui::widgets::clickable(s).size(16.0, 16.0).items_center().justify_center()
            })
            .into_any(),
            (None, None) if hovered && plus_file.buffer.head_line(display_line).is_some() => {
                let plus_file = plus_file.clone();
                Container::new(
                    sidebar_glyph(SidebarIcon::Plus, theme::CANVAS).style(|s| s.size(11.0, 11.0)),
                )
                .on_event_stop(el::Click, move |_, _| {
                    // The selection if this line is inside it, else this line.
                    let (first, last) = match state.selection.get() {
                        Some((a, b)) if (a.min(b)..=a.max(b)).contains(&display_line) => (a, b),
                        _ => (display_line, display_line),
                    };
                    let anchor = state.anchor_for_display_range(&plus_file, first, last);
                    state.open_composer(None, anchor, String::new(), crate::review::threads::defaults(app));
                })
                .style(|s| {
                    crate::ui::widgets::clickable(s)
                        .size(16.0, 16.0)
                        .items_center()
                        .justify_center()
                        .border_radius(3.0)
                        .background(theme::ACCENT)
                })
                .into_any()
            }
            (None, None) => Empty::new().into_any(),
        },
    )
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
