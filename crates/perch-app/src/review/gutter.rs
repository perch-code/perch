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
//! on the left, head numbers on the right, and nothing on a filler. Thread
//! markers follow their anchor's side; only the head carries the `+` for a
//! new question and GitHub's head-side markers.
//!
//! The rows are the document's, not the buffer's: a row that is a line is
//! numbered and barred from the buffer's row table, a divider's row holds
//! the expander that opens its run that way, as github.com's hunk rows do,
//! and a spacer under a card is blank. In a split the expanders are on the
//! base side, at the row's left end as github.com has them, and the head
//! side's row is the divider's tint.
//!
//! A gutter is as wide as the file's widest number needs, and no wider:
//! the controls that open a fold are a row each rather than a row of
//! controls, so they need no room of their own.
//!
//! The document is edited in place under the editor, so a row is keyed by
//! what it is as well as where it is: a row that is the same thing after an
//! edit keeps its view, and one that is not is built again.

use std::sync::Arc;

use floem::event::PointerScrollEventExt;
use floem::kurbo::{Rect, Size, Vec2};
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, dyn_stack, ClipExt, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_git::Row;
use perch_github::{Side as AnchorSide, ThreadOrigin};

use crate::icons::{self, SidebarIcon};
use crate::review::cards::tip_with;
use crate::review::doc::{self, CardId, Doc, DocRow, Expand, Tint};
use crate::review::editor::Side;
use crate::review::model::{LoadedFile, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_in, sidebar_glyph};

/// Extra rows rendered past the viewport, so a partially visible line at
/// either edge is still numbered.
const OVERSCAN: usize = 2;

/// Which display lines fall inside the viewport.
///
/// Pure, because this is the arithmetic most likely to be wrong and the
/// hardest to see wrong: an off-by-one here shifts every number in the
/// gutter against the code beside it.
pub(crate) fn visible_lines(
    scroll_top: f64,
    height: f64,
    line_height: f64,
    total: usize,
) -> Vec<usize> {
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

/// The rule down the gutter's right edge, between it and the code.
const RULE: f64 = 1.0;

/// How wide one digit of a line number is, in the code font at the code's
/// size, measured once. The font's digits are all one width.
fn digit_width() -> f64 {
    thread_local! {
        static WIDTH: std::cell::Cell<Option<f64>> = const { std::cell::Cell::new(None) };
    }
    WIDTH.with(|cell| {
        if let Some(width) = cell.get() {
            return width;
        }
        use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
        let mut layout = TextLayout::new();
        layout.set_text(
            "0123456789",
            AttrsList::new(Attrs::new().family(&family).font_size(theme::CODE_FONT_SIZE)),
            None,
        );
        let width = layout.size().width / 10.0;
        cell.set(Some(width));
        width
    })
}

/// How wide a gutter is whose widest number has `digits` digits: the
/// slot, the numbers at no fewer than `GUTTER_MIN_DIGITS`, the room about
/// them, the change bar and the rule, rounded up to a whole pixel so the
/// edge between the gutter and the code is never drawn across two.
pub fn width(digits: usize) -> f64 {
    let numbers = digits.max(theme::GUTTER_MIN_DIGITS) as f64 * digit_width();
    (theme::GUTTER_SLOT_INSET
        + theme::GUTTER_SLOT
        + theme::GUTTER_NUMBER_GAP
        + numbers
        + theme::GUTTER_NUMBER_PADDING
        + theme::GUTTER_BAR
        + RULE)
        .ceil()
}

/// How wide a file's gutters are: as `width` has it for the widest number
/// either side shows, a head line, a ghost's base line, or the last line
/// of the base beside it in a split. Both sides of a split are this wide,
/// so the two halves stay the same shape.
pub fn width_of(file: &LoadedFile) -> f64 {
    let shown = file
        .buffer
        .rows
        .iter()
        .map(|row| match row {
            Row::Head { head_line, .. } => head_line + 1,
            Row::Deleted { base_line } => base_line + 1,
        })
        .max()
        .unwrap_or(0);
    let base = file.base_buffer.as_ref().map_or(0, |b| b.len());
    width(shown.max(base).max(1).to_string().len())
}

/// How far the column slides to line up with a part-scrolled first row.
pub(crate) fn column_offset(scroll_top: f64, line_height: f64) -> f64 {
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
    column(viewport, scroll_delta, doc, width_of(&file), move |row, entry| {
        gutter_row(state, app, file.clone(), doc, row, entry)
    })
}

/// The gutter of a file outside the worktree: line numbers and nothing
/// else. No change bar, since there is no diff; no `+` and no marker,
/// since nothing is anchored in a file that is no part of the review.
pub fn plain_gutter(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
) -> impl IntoView {
    column(viewport, scroll_delta, doc, width_of(&file), move |row, entry| {
        let number = match entry {
            DocRow::Line { display, .. } => Some(display + 1),
            _ => None,
        };
        row_view(state, app, file.clone(), row, number, false, None, Slot::Blank)
    })
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
    column(viewport, scroll_delta, doc, width_of(&file), move |row, entry| {
        side_row(state, app, file.clone(), side, doc, row, entry)
    })
}

/// The rows in view, slid to the scroll, in a column `width` wide beside
/// the editor. Each row's view is built from its index and what the
/// document says the row is, and keyed by both, so an edit to the document
/// rebuilds the rows it changed and no other.
fn column<V: IntoView + 'static>(
    viewport: RwSignal<Rect>,
    scroll_delta: RwSignal<Vec2>,
    doc: RwSignal<Arc<Doc>>,
    width: f64,
    row: impl Fn(usize, DocRow) -> V + 'static,
) -> impl IntoView {
    let line_height = theme::CODE_LINE_HEIGHT as f64;

    let visible = move || {
        let vp = viewport.get();
        // The document is read here, tracked, so one that changes under
        // the editor changes the column with it.
        doc.with(|d| {
            visible_lines(vp.y0, vp.height(), line_height, d.len())
                .into_iter()
                .map(|row| (row, d.rows[row].clone()))
                .collect::<Vec<_>>()
        })
    };

    let column = dyn_stack(
        visible,
        |(row, entry)| (*row, entry.clone()),
        move |(index, entry)| row(index, entry),
    )
    .style(move |s| {
        // The whole column slides by the sub-line remainder of the
        // scroll. Reading the viewport here, in a style closure, is what
        // keeps it live: style closures re-run when their signals change,
        // whereas a value captured when a row was built does not.
        let offset = column_offset(viewport.get().y0, line_height);
        s.absolute().inset_top(offset).width_full().flex_col()
    });

    Container::new(column)
        .debug_name("gutter")
        // This inner box is what the column positions against, so it needs
        // the size and the relative position. Styling the clip wrapper
        // instead would leave it shrinking to its content.
        .style(|s| s.size_full().position(Position::Relative))
        .clip()
        // The wheel over the numbers scrolls the code, as it does over the
        // code itself. Down the file only: nothing here goes sideways.
        .on_event_stop(el::PointerWheel, move |_, wheel| {
            let size = viewport.get_untracked().size();
            let delta =
                wheel.resolve_to_points(Some(Size::new(line_height, line_height)), Some(size));
            scroll_delta.set(Vec2::new(0.0, -delta.y));
        })
        // A bare key goes to the view that has the keyboard, and the
        // window's handler catches it on the way down. A click in the code
        // gives the editor the keyboard; a click here gives it to the
        // gutter, so `]` works after either. Not to a number: floem's theme
        // makes every label focusable, and a number that took the keyboard
        // would take it away with it when a jump scrolls its row off.
        .style(move |s| {
            s.width(width)
                .height_full()
                .flex_shrink(0.0_f32)
                .background(theme::GUTTER_BG)
                .border_right(RULE)
                .border_color(theme::BORDER_MUTED)
                .focusable()
                .class(floem::views::LabelClass, |s| s.focus_none())
        })
}

/// Which line an anchor slot represents. A stacked row gets its side from
/// the unified row; a split row names its side explicitly.
#[derive(Clone, Copy)]
enum AnchorRow {
    Stacked(usize),
    Base(usize),
    Head { display: usize, head: usize },
}

/// What a row's left slot can hold: a thread marker, a `+` on an eligible
/// head row, or nothing at all.
#[derive(Clone, Copy)]
enum Slot {
    Anchor(AnchorRow),
    Blank,
}

/// A row of the stacked layout's gutter.
fn gutter_row(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    row: usize,
    entry: DocRow,
) -> AnyView {
    let DocRow::Line { display, tint } = entry else {
        return other_row(state, app, file, doc, row, entry, true);
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

    row_view(
        state,
        app,
        file,
        row,
        number,
        dimmed,
        change_bar_color(tint),
        Slot::Anchor(AnchorRow::Stacked(display)),
    )
}

/// A row of one side's gutter in a split.
fn side_row(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    side: Side,
    doc: RwSignal<Arc<Doc>>,
    row: usize,
    entry: DocRow,
) -> AnyView {
    let DocRow::Line { display, tint } = entry else {
        // The divider's expanders are on the base side, at the row's left
        // end as on github.com; the head side's row is the tint alone.
        return other_row(state, app, file, doc, row, entry, side == Side::Base);
    };

    // The base side's document is over the base file, so its line is its
    // number; the head side's is over the unified buffer, and the number
    // is the head line displayed there. A marker follows its anchor's side;
    // only the head slot can offer the `+` for a new question.
    let (number, slot) = match side {
        Side::Base => (Some(display + 1), Slot::Anchor(AnchorRow::Base(display))),
        Side::Head => match file.buffer.head_line(display) {
            Some(head) => (
                Some(head + 1),
                Slot::Anchor(AnchorRow::Head { display, head }),
            ),
            None => (None, Slot::Blank),
        },
    };

    row_view(
        state,
        app,
        file,
        row,
        number,
        false,
        change_bar_color(tint),
        slot,
    )
}

/// A row that is no line: a divider's row is its expander when this
/// gutter holds them and the divider's tint when it does not, and a
/// filler or a spacer is blank. A blank row records no pointer, so
/// nothing lights a `+`.
fn other_row(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    row: usize,
    entry: DocRow,
    controls: bool,
) -> AnyView {
    match entry {
        DocRow::Divider { expand, .. } if controls => expander(state, file, doc, row, expand),
        DocRow::Divider { .. } => Empty::new()
            .style(|s| {
                s.width_full()
                    .height(theme::CODE_LINE_HEIGHT as f64)
                    .flex_shrink(0.0_f32)
                    .background(theme::DIFF_DIVIDER_BG)
            })
            .into_any(),
        DocRow::Line { .. } | DocRow::Filler | DocRow::Spacer { .. } => {
            row_view(state, app, file, row, None, false, None, Slot::Blank)
        }
    }
}

/// One of a divider's expanders, as the divider's row of the gutter: the
/// gutter's width and a row's height in the hunk number cell's blue with
/// the expander's glyph in the middle, and GitHub's blue under a white
/// glyph while the pointer is on it. A click reveals what the expander
/// opens of the run as the document has it then, not as it was when the
/// row was built: a reveal that leaves the divider where it is changes
/// what the divider hides. The pointer on it is recorded as on the
/// divider's first row, which is what the row's Fold again reads.
fn expander(
    state: ReviewState,
    file: Arc<LoadedFile>,
    doc: RwSignal<Arc<Doc>>,
    row: usize,
    expand: Expand,
) -> AnyView {
    let glyph = match expand {
        Expand::Down => SidebarIcon::ExpandDown,
        Expand::Up => SidebarIcon::ExpandUp,
        Expand::All => SidebarIcon::ExpandAll,
    };
    let hovered = RwSignal::new(false);
    let color = move || {
        if hovered.get() {
            theme::DIFF_EXPANDER_HOVER_FG
        } else {
            theme::DIFF_EXPANDER_FG
        }
    };
    let explanation = move || {
        let Some(run) = doc.with_untracked(|d| d.run_of(row)) else {
            return String::new();
        };
        let step = run.len().min(doc::REVEAL_STEP);
        let lines = |n: usize| if n == 1 { "line" } else { "lines" };
        match expand {
            Expand::Down => format!("Show the first {step} hidden {}", lines(step)),
            Expand::Up => format!("Show the last {step} hidden {}", lines(step)),
            Expand::All => format!("Show all {} hidden {}", run.len(), lines(run.len())),
        }
    };
    let first_row = move || {
        doc.with_untracked(|d| d.divider_span(row))
            .map_or(row, |span| span.start)
    };
    tip_with(
        Container::new(
            icon_in(move || icons::sidebar_icon(glyph, color()), color)
                .style(|s| s.size(16.0, 16.0)),
        )
        .debug_name(format!("expander:{expand:?}"))
        .on_event_stop(el::Click, move |_, _| {
            if let Some(run) = doc.with_untracked(|d| d.run_of(row)) {
                let (first, last) = run.reveals(expand);
                state.reveal(&file.path, &file.blob, first, last);
            }
        })
        .on_event_cont(el::PointerEnter, move |cx, _| {
            hovered.set(true);
            state.enter_row(cx.target, first_row());
        })
        .on_event_cont(el::PointerLeave, move |cx, _| {
            hovered.set(false);
            state.leave_row(cx.target);
        })
        .style(move |s| {
            clickable(s)
                .width_full()
                .height(theme::CODE_LINE_HEIGHT as f64)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .background(if hovered.get() {
                    theme::DIFF_EXPANDER_HOVER_BG
                } else {
                    theme::DIFF_EXPANDER_BG
                })
        }),
        explanation,
    )
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
        Slot::Anchor(anchor) => anchor_slot(state, app, file, key, anchor).into_any(),
        Slot::Blank => Empty::new().into_any(),
    };

    let row = Stack::horizontal((
        // The left slot: a marker where a thread starts, a `+` under the
        // pointer, nothing otherwise. Fixed width, so numbers never shift.
        slot_view.style(|s| {
            s.size(theme::GUTTER_SLOT, theme::GUTTER_SLOT)
                .margin_left(theme::GUTTER_SLOT_INSET)
                .flex_shrink(0.0_f32)
        }),
        // The number, set against the bar. The gutter is as wide as the
        // file's widest number needs, so that one sits the gap `width`
        // leaves from the slot, and every other is further off.
        Label::derived(move || number.map(|n| n.to_string()).unwrap_or_default()).style(move |s| {
            s.flex_grow(1.0_f32)
                .min_width(0.0)
                .selectable(false)
                // The code's own size, as Codex numbers its diffs.
                .font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::CODE_FONT_SIZE)
                .color(if dimmed {
                    theme::DIFF_DELETED_BAR
                } else {
                    theme::GUTTER_FG
                })
                .text_align(floem::text::Alignment::End)
                .padding_right(theme::GUTTER_NUMBER_PADDING)
        }),
        change_bar(bar),
    ))
    .style(move |s| {
        s.width_full()
            .height(theme::CODE_LINE_HEIGHT as f64)
            .flex_shrink(0.0_f32)
            .items_center()
    });

    match slot {
        Slot::Anchor(AnchorRow::Stacked(_) | AnchorRow::Head { .. }) => row
            .on_event_cont(el::PointerEnter, move |cx, _| {
                state.enter_row(cx.target, key)
            })
            .on_event_cont(el::PointerLeave, move |cx, _| state.leave_row(cx.target))
            .into_any(),
        Slot::Anchor(AnchorRow::Base(_)) | Slot::Blank => row.into_any(),
    }
}

/// The mark on a line with a thread: solid in GitHub's blue when the
/// thread holds a draft on its way there, whatever else it is; hollow
/// and muted for one of GitHub's; solid for the reader's own, faded
/// when it is outdated.
pub(crate) fn thread_mark(
    origin: ThreadOrigin,
    outdated: bool,
    draft: bool,
) -> (SidebarIcon, Color) {
    match origin {
        _ if draft => (SidebarIcon::Thread, theme::GITHUB),
        ThreadOrigin::GitHub => (SidebarIcon::ThreadHollow, theme::FG_MUTED),
        ThreadOrigin::Local if outdated => (SidebarIcon::Thread, theme::FG_SUBTLE),
        ThreadOrigin::Local => (SidebarIcon::Thread, theme::ACCENT),
    }
}

/// The marker for a thread starting on this row, or the `+` that starts one
/// on a stacked/head row while the pointer is there. Base rows are marker-only.
fn anchor_slot(
    state: ReviewState,
    app: AppState,
    file: Arc<LoadedFile>,
    key: usize,
    anchor: AnchorRow,
) -> impl IntoView {
    let plus_display = match anchor {
        AnchorRow::Stacked(display) if file.buffer.head_line(display).is_some() => Some(display),
        AnchorRow::Head { display, .. } => Some(display),
        AnchorRow::Stacked(_) | AnchorRow::Base(_) => None,
    };
    let marker_file = file.clone();
    let plus_file = file;

    dyn_container(
        move || {
            let thread = match anchor {
                AnchorRow::Stacked(display) => state.thread_starting_at(&marker_file, display),
                AnchorRow::Base(base) => {
                    state.thread_starting_on(&marker_file, AnchorSide::Left, base)
                }
                AnchorRow::Head { head, .. } => {
                    state.thread_starting_on(&marker_file, AnchorSide::Right, head)
                }
            }
            .map(|t| {
                let draft = t.messages.iter().any(|m| m.is_draft());
                (t.id, t.origin, t.outdated, draft)
            });
            let hovered = state.hovered_line.get() == Some(key);
            (thread, hovered)
        },
        move |(thread, hovered)| match (thread, hovered.then_some(plus_display).flatten()) {
            // A thread on this line: solid in GitHub's blue when it holds
            // a draft on its way there, solid for the reader's own, hollow
            // and muted for one of GitHub's. Click scrolls the editor to
            // its card and flashes it.
            (Some((id, origin, outdated, draft)), _) => {
                let (icon, color) = thread_mark(origin, outdated, draft);
                Container::new(sidebar_glyph(icon, color).style(|s| s.size(12.0, 12.0)))
                    .on_event_stop(el::Click, move |_, _| {
                        state.focus_card.set(Some(CardId::Thread(id.clone())))
                    })
                    .style(|s| {
                        crate::ui::widgets::clickable(s)
                            .size(16.0, 16.0)
                            .items_center()
                            .justify_center()
                    })
                    .into_any()
            }
            (None, Some(display_line)) => {
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
                    state.open_composer(
                        None,
                        anchor,
                        String::new(),
                        crate::review::threads::defaults(app),
                    );
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
        let base = s.width(theme::GUTTER_BAR).height_full();
        match color {
            Some(c) => base.background(c),
            None => base,
        }
    }))
    .style(|s| s.width(theme::GUTTER_BAR).height_full().flex_shrink(0.0_f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 19.0;

    #[test]
    fn a_draft_marks_its_line_in_githubs_blue_ahead_of_whose_thread_it_is() {
        assert_eq!(
            thread_mark(ThreadOrigin::GitHub, false, true),
            (SidebarIcon::Thread, theme::GITHUB)
        );
        assert_eq!(
            thread_mark(ThreadOrigin::GitHub, true, true),
            (SidebarIcon::Thread, theme::GITHUB),
            "a new draft thread has no id on GitHub yet, and is still on its way"
        );
        assert_eq!(
            thread_mark(ThreadOrigin::GitHub, false, false),
            (SidebarIcon::ThreadHollow, theme::FG_MUTED)
        );
        assert_eq!(
            thread_mark(ThreadOrigin::Local, false, false),
            (SidebarIcon::Thread, theme::ACCENT)
        );
        assert_eq!(
            thread_mark(ThreadOrigin::Local, true, false),
            (SidebarIcon::Thread, theme::FG_SUBTLE)
        );
    }

    #[test]
    fn a_gutter_is_as_wide_as_its_widest_number_needs_and_never_under_three_digits() {
        let digit = digit_width();
        assert!(digit > 0.0, "the code font measures");
        assert_eq!(width(1), width(3), "three digits at the least");
        let fixed = theme::GUTTER_SLOT_INSET
            + theme::GUTTER_SLOT
            + theme::GUTTER_NUMBER_GAP
            + theme::GUTTER_NUMBER_PADDING
            + theme::GUTTER_BAR
            + RULE;
        for digits in 3..=6 {
            let w = width(digits);
            assert_eq!(w, w.round(), "{digits} digits: a whole pixel");
            assert!(w - fixed >= digits as f64 * digit, "{digits} digits fit");
            assert!(w - fixed < digits as f64 * digit + 1.0, "{digits} digits: no room to spare");
        }
    }

    #[test]
    fn a_files_gutter_is_sized_for_the_longer_side() {
        let file = |head: String, base: String| {
            crate::backend::build_loaded_file(
                "src/x.rs".into(),
                head,
                base,
                Some(perch_git::ChangeStatus::Modified),
                false,
                perch_git::Scope::Branch,
                String::new(),
            )
        };
        let lines = |n: usize| (0..n).map(|i| format!("line {i}\n")).collect::<String>();
        assert_eq!(width_of(&file(lines(90), lines(80))), width(3));
        assert_eq!(width_of(&file(lines(1200), lines(1200))), width(4));
        // A split puts every base line beside the head, so a long base
        // widens both gutters, though the head is short.
        assert_eq!(width_of(&file(lines(10), lines(1000))), width(4));
    }

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
            assert!(
                offset > -H && offset <= 0.0,
                "scroll {scroll} gave {offset}"
            );
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
