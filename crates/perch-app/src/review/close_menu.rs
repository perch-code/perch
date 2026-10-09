//! Close, beside Comment, as github.com has it: a split button. Its face
//! closes the way last picked and its arrow opens a menu of the ways,
//! each with a line on what it means. An issue closes as completed, as
//! not planned, or as a duplicate of another issue, found by a search of
//! the repository's issues; a pull request closes, or closes as an
//! issue, which unfolds that form.
//!
//! The face asks first, on the footer's line, as Comment does. The menu
//! is drawn over the window, as the `@` list is, so a composer low in
//! Conversation does not cut it off; a press anywhere else, or a scroll,
//! puts it away.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use floem::event::EventPropagation;
use floem::kurbo::Rect;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{
    BorderBottomLeftRadius, BorderBottomRightRadius, BorderTopLeftRadius, BorderTopRightRadius,
    Position,
};
use floem::views::{
    dyn_container, Container, Decorators, Empty, Label, PlaceholderTextClass, ScrollExt, Stack,
    TextInput,
};
use perch_github::{IssueCloseReason, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::mention::{self, ReferenceHit};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph, state_glyph};

/// The menu while it is up: the button's box in the window, which it
/// hangs from, whether it shows the search for the issue a duplicate
/// repeats, and whether that search was opened from the state pill,
/// which closes at a pick rather than asking on the composer's line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloseMenu {
    pub anchor: Rect,
    pub duplicate: bool,
    pub direct: bool,
}

/// The search for the issue a duplicate repeats, hung from `anchor`,
/// as the state pill's Close as duplicate opens it. A pick closes the
/// issue at once.
pub fn search_duplicate(state: ReviewState, anchor: Rect) {
    state.close_duplicate_query.set(String::new());
    state.close_menu.set(Some(CloseMenu {
        anchor,
        duplicate: true,
        direct: true,
    }));
}

const WIDTH: f64 = 300.0;
const ROW_HEIGHT: f64 = 50.0;
const PADDING: f64 = crate::ui::menu::GUTTER;
/// The room kept between the menu and the window's edges.
const GUTTER: f64 = 8.0;
/// The issues the duplicate search shows before it scrolls.
const DUPLICATE_ROWS: usize = 8;
const SEARCH_HEIGHT: f64 = 30.0;
const HEADER_HEIGHT: f64 = 20.0;

/// The face: the glyph of the close it makes and what it says. Tracked.
fn face(state: ReviewState) -> (SubjectState, String) {
    let commented = !state.composer_draft.with(|d| d.trim().is_empty());
    if state.issue.with(Option::is_none) {
        let label = if commented {
            "Close with comment"
        } else {
            "Close pull request"
        };
        return (SubjectState::PrClosed, label.to_string());
    }
    match state.composer_close_reason.get() {
        IssueCloseReason::Completed => {
            let label = if commented {
                "Close with comment"
            } else {
                "Close issue"
            };
            (SubjectState::IssueClosed, label.to_string())
        }
        IssueCloseReason::NotPlanned => (
            SubjectState::IssueNotPlanned,
            "Close as not planned".to_string(),
        ),
        IssueCloseReason::Duplicate(n) => (
            SubjectState::IssueNotPlanned,
            format!("Close as duplicate of #{n}"),
        ),
    }
}

/// Ask whether to close, on the footer's line.
fn ask(state: ReviewState) {
    if state.posting.with_untracked(Option::is_some) {
        return;
    }
    state.composer_closing.set(true);
    state.composer_confirming.set(true);
}

/// Put the menu away, and give the composer's box the keyboard back.
pub fn close(state: ReviewState) {
    if state.close_menu.with_untracked(Option::is_some) {
        state.close_menu.set(None);
    }
    if let Some(id) = state.composer_box.get_untracked() {
        id.request_focus();
    }
}

/// The split button, where the composer offers to close its pull request
/// or issue.
pub fn split_button(app: AppState, state: ReviewState) -> impl IntoView {
    let offered = Memo::new(move |_| crate::review::threads::close_offered(app, state));
    let face = Memo::new(move |_| face(state));
    let inner = theme::RADIUS - 1.0;

    let main = Stack::horizontal((
        dyn_container(
            move || face.get().0,
            |subject| state_glyph(subject, move || icons::state_color(subject)).into_any(),
        )
        .style(|s| s.margin_right(6.0).pointer_events_none()),
        Label::derived(move || face.get().1).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.close_menu.set(None);
        ask(state);
    })
    .style(move |s| {
        clickable(s)
            .height_full()
            .items_center()
            .padding_horiz(10.0)
            .set(BorderTopLeftRadius, inner)
            .set(BorderBottomLeftRadius, inner)
            .hover(|s| s.background(theme::canvas_hover()))
    })
    .debug_name("subject-close");

    // The menu hangs from the whole button, which is built after the arrow.
    let whole: RwSignal<Option<floem::ViewId>> = RwSignal::new(None);
    let arrow = Container::new(
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_muted)
            .style(|s| s.size(10.0, 10.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| {
        if state.close_menu.with_untracked(Option::is_some) {
            close(state);
        } else if state.posting.with_untracked(Option::is_none) {
            let Some(id) = whole.get_untracked() else {
                return;
            };
            state.close_duplicate_query.set(String::new());
            state.close_menu.set(Some(CloseMenu {
                anchor: id.get_visual_rect_no_clip(),
                duplicate: false,
                direct: false,
            }));
        }
    })
    .style(move |s| {
        clickable(s)
            .height_full()
            .items_center()
            .padding_horiz(8.0)
            .set(BorderTopRightRadius, inner)
            .set(BorderBottomRightRadius, inner)
            .hover(|s| s.background(theme::canvas_hover()))
            .apply_if(state.close_menu.with(Option::is_some), |s| {
                s.background(theme::canvas_hover())
            })
    })
    .debug_name("subject-close-menu");
    let button = Stack::horizontal((
        main,
        Empty::new().style(|s| s.width(1.0).height_full().background(theme::border())),
        arrow,
    ))
    .style(|s| {
        s.height(28.0)
            .items_center()
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
    });
    whole.set(Some(button.view_id()));

    Container::new(button).style(move |s| s.apply_if(!offered.get(), |s| s.hide()))
}

/// The menu's height, for placing it before it is laid out.
fn height(app: AppState, state: ReviewState, duplicate: bool) -> f64 {
    let body = if duplicate {
        HEADER_HEIGHT + 8.0 + SEARCH_HEIGHT + 6.0 + list_height(state)
    } else {
        let rows = if state.issue.with(Option::is_some) {
            3.0
        } else if crate::review::threads::pull_request_close_offered(app, state) {
            2.0
        } else {
            1.0
        };
        rows * ROW_HEIGHT + (rows - 1.0)
    };
    body + 2.0 * PADDING + 2.0
}

/// The query typed into the duplicate search, without a leading `#`.
/// Tracked.
fn duplicate_query(state: ReviewState) -> String {
    state
        .close_duplicate_query
        .with(|q| q.trim().trim_start_matches('#').to_string())
}

/// The issues a duplicate can repeat, for what is typed. Tracked.
fn duplicate_hits(state: ReviewState) -> Vec<ReferenceHit> {
    let number = state
        .issue
        .with(|i| i.as_ref().map(|i| i.number))
        .unwrap_or_default();
    mention::duplicate_candidates(state, &duplicate_query(state), number)
}

/// The duplicate search's list: as tall as its issues, up to
/// `DUPLICATE_ROWS`, and a row for saying there are none. Tracked.
fn list_height(state: ReviewState) -> f64 {
    duplicate_hits(state).len().clamp(1, DUPLICATE_ROWS) as f64 * theme::GO_TO_ROW_HEIGHT
}

/// Where the menu goes: under the button, its right edge on the
/// button's, or over it when there is not room below, and kept inside
/// the window. The left edge and the top.
fn placement(anchor: Rect, size: (f64, f64), window: (f64, f64)) -> (f64, f64) {
    let (width, height) = size;
    let left = (anchor.x1 - width)
        .min(window.0 - width - GUTTER)
        .max(GUTTER.min(window.0 - width).max(0.0));
    let below = anchor.y1 + 4.0;
    let above = anchor.y0 - 4.0 - height;
    let top = if below + height <= window.1 - GUTTER || above < GUTTER {
        below
    } else {
        above
    };
    (left, top)
}

/// The menu over the window while it is up, over a sheet that puts it
/// away at a press or a scroll anywhere else.
pub fn popup(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let pane = Memo::new(move |_| state.close_menu.with(|m| m.map(|m| m.duplicate)));
    let sheet = Empty::new()
        .on_event_stop(el::PointerDown, move |_, _| close(state))
        .on_event_stop(el::PointerWheel, move |_, _| close(state))
        .style(|s| s.position(Position::Absolute).inset(0.0).z_index(94))
        .debug_name("close-sheet");
    let menu = dyn_container(
        move || pane.get(),
        move |pane| match pane {
            None => Empty::new().into_any(),
            Some(false) => reasons(app, state).into_any(),
            Some(true) => duplicate(state, backend.clone()).into_any(),
        },
    )
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape
            && key.modifiers.is_empty()
            && state.close_menu.with_untracked(Option::is_some)
        {
            back(state);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let Some(menu) = state.close_menu.get() else {
            return s.hide();
        };
        let window = (app.window_width.get(), app.window_height.get());
        let width = WIDTH.min(window.0 - 2.0 * GUTTER).max(0.0);
        let height = height(app, state, menu.duplicate);
        let (left, top) = placement(menu.anchor, (width, height), window);
        s.position(Position::Absolute)
            .inset_left(left)
            .inset_top(top)
            .width(width)
            .height(height)
            .z_index(95)
            .padding(PADDING)
            .apply(crate::ui::menu::surface(floem::style::Style::new()))
    })
    .debug_name("close-ways");
    Stack::new((sheet, menu))
        .style(move |s| {
            s.position(Position::Absolute)
                .inset(0.0)
                .apply_if(state.close_menu.with(Option::is_none), |s| s.hide())
        })
        .debug_name("close-overlay")
}

/// Escape: from the duplicate search back to the ways, and from them
/// out of the menu. The state pill's search has no ways to go back to.
pub fn back(state: ReviewState) {
    match state.close_menu.get_untracked() {
        Some(menu) if menu.duplicate && !menu.direct => state.close_menu.set(Some(CloseMenu {
            duplicate: false,
            ..menu
        })),
        Some(_) => close(state),
        None => {}
    }
}

/// The ways to close, one to a row, with lines between, as GitHub's menu
/// has them.
fn reasons(app: AppState, state: ReviewState) -> impl IntoView {
    let separator = || {
        Empty::new().style(|s| {
            s.height(1.0)
                .width_full()
                .flex_shrink(0.0_f32)
                .margin_horiz(8.0)
                .background(theme::menu_separator())
        })
    };
    let reason = move || state.composer_close_reason.get();
    let rows = if state.issue.with_untracked(Option::is_some) {
        let pick = move |picked: IssueCloseReason| {
            move || {
                state.composer_close_reason.set(picked);
                close(state);
            }
        };
        Stack::vertical((
            row(
                move || reason() == IssueCloseReason::Completed,
                SubjectState::IssueClosed,
                "Close as completed",
                "Done, closed, fixed, resolved",
                false,
                pick(IssueCloseReason::Completed),
            )
            .debug_name("close-as-completed"),
            separator(),
            row(
                move || reason() == IssueCloseReason::NotPlanned,
                SubjectState::IssueNotPlanned,
                "Close as not planned",
                "Won't fix, can't repro, stale",
                false,
                pick(IssueCloseReason::NotPlanned),
            )
            .debug_name("close-as-not-planned"),
            separator(),
            row(
                move || matches!(reason(), IssueCloseReason::Duplicate(_)),
                SubjectState::IssueNotPlanned,
                "Close as duplicate",
                "Duplicate of another issue",
                true,
                move || {
                    state.close_duplicate_query.set(String::new());
                    state.close_menu.update(|m| {
                        if let Some(m) = m {
                            m.duplicate = true;
                        }
                    })
                },
            )
            .debug_name("close-as-duplicate"),
        ))
        .into_any()
    } else {
        let as_issue =
            Memo::new(move |_| crate::review::threads::pull_request_close_offered(app, state));
        Stack::vertical((
            row(
                || true,
                SubjectState::PrClosed,
                "Close pull request",
                "Close without merging",
                false,
                move || close(state),
            )
            .debug_name("close-pull-request"),
            separator().style(move |s| s.apply_if(!as_issue.get(), |s| s.hide())),
            row(
                || false,
                SubjectState::IssueOpen,
                "Close as issue\u{2026}",
                "Open an issue for it, then close",
                false,
                move || {
                    close(state);
                    let item_id = state
                        .item
                        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()));
                    if let Some(item_id) = item_id {
                        crate::review::close_as_issue::unfold(app, state, &item_id);
                    }
                },
            )
            .debug_name("close-as-issue-unfold")
            .style(move |s| s.apply_if(!as_issue.get(), |s| s.hide())),
        ))
        .into_any()
    };
    rows.style(|s| s.flex_col().width_full())
}

/// One way to close: a check on the one picked, its glyph, its name and
/// what it means under it, and an arrow when it goes on to more.
fn row(
    checked: impl Fn() -> bool + 'static,
    glyph: SubjectState,
    title: &'static str,
    detail: &'static str,
    more: bool,
    on_pick: impl Fn() + 'static,
) -> Container {
    Container::new(
        Stack::horizontal((
            Container::new(
                sidebar_glyph(SidebarIcon::Check, theme::fg)
                    .style(move |s| s.size(12.0, 12.0).apply_if(!checked(), |s| s.hide())),
            )
            .style(|s| s.width(20.0).flex_shrink(0.0_f32).items_center()),
            state_glyph(glyph, move || icons::state_color(glyph))
                .style(|s| s.flex_shrink(0.0_f32).margin_right(8.0)),
            Stack::vertical((
                Label::derived(move || title.to_string()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_strong())
                        .color(theme::fg())
                        .selectable(false)
                }),
                Label::derived(move || detail.to_string()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_muted())
                        .margin_top(2.0)
                        .selectable(false)
                }),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
            sidebar_glyph(SidebarIcon::ChevronRight, theme::fg_subtle).style(move |s| {
                s.size(10.0, 10.0)
                    .flex_shrink(0.0_f32)
                    .margin_left(8.0)
                    .apply_if(!more, |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().width_full().pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| on_pick())
    .style(|s| {
        clickable(s)
            .width_full()
            .height(ROW_HEIGHT)
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_right(10.0)
            .border_radius(crate::ui::menu::ROW_RADIUS)
            .hover(|s| s.background(theme::menu_hover()))
    })
}

/// The search for the issue a duplicate repeats: a box, and the
/// repository's issues it finds, the recently updated ones before
/// anything is typed, as a `#` lists them. Picking one asks whether to
/// close, on the footer's line.
fn duplicate(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let selected = RwSignal::new(0usize);
    let hits = Memo::new(move |_| Arc::new(duplicate_hits(state)));
    let asks = backend.clone();
    Effect::new(move |_| {
        let query = duplicate_query(state);
        selected.set(0);
        mention::ask_references(state, asks.clone(), &query);
    });
    let direct = state
        .close_menu
        .with_untracked(|m| m.is_some_and(|m| m.direct));
    let pick: Rc<dyn Fn(usize)> = {
        Rc::new(move |index: usize| {
            let Some(original) = hits.with_untracked(|h| h.get(index).map(|h| h.reference.number))
            else {
                return;
            };
            let reason = IssueCloseReason::Duplicate(original);
            close(state);
            if direct {
                crate::review::summary::change_state(
                    state,
                    &backend,
                    crate::backend::StateChange::CloseIssue(reason),
                );
            } else {
                state.composer_close_reason.set(reason);
                ask(state);
            }
        })
    };

    let on_enter = pick.clone();
    let input = TextInput::new(state.close_duplicate_query)
        .placeholder("Search for the issue it duplicates")
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            let n = hits.with_untracked(|h| h.len());
            let step = |by: isize| {
                if n > 0 {
                    let at = selected.get_untracked() as isize;
                    selected.set((at + by).rem_euclid(n as isize) as usize);
                }
            };
            match key.code {
                Code::ArrowDown if key.modifiers.is_empty() => step(1),
                Code::ArrowUp if key.modifiers.is_empty() => step(-1),
                Code::Enter | Code::NumpadEnter if key.modifiers.is_empty() => {
                    on_enter(selected.get_untracked())
                }
                _ => return EventPropagation::Continue,
            }
            EventPropagation::Stop
        })
        .debug_name("close-duplicate-search")
        .style(|s| {
            s.width_full()
                .height(SEARCH_HEIGHT)
                .flex_shrink(0.0_f32)
                .padding_horiz(8.0)
                .font_size(theme::TEXT_META)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(crate::ui::menu::ROW_RADIUS)
                .focus(|s| s.border_color(theme::accent_ring()))
                .class(PlaceholderTextClass, |s| s.color(theme::fg_subtle()))
        });
    // The keyboard goes to the box once it is laid out.
    let input_id = input.view_id();
    let asked = Rc::new(Cell::new(false));
    let input = input.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            input_id.request_focus();
        }
    });

    let list = dyn_container(
        move || hits.get(),
        move |hits| {
            if hits.is_empty() {
                return Label::derived(|| "No issues found".to_string())
                    .style(|s| {
                        s.height(theme::GO_TO_ROW_HEIGHT)
                            .items_center()
                            .padding_horiz(8.0)
                            .font_size(theme::TEXT_META)
                            .color(theme::fg_subtle())
                            .selectable(false)
                    })
                    .into_any();
            }
            Stack::vertical_from_iter(
                hits.iter()
                    .enumerate()
                    .map(|(index, hit)| duplicate_row(index, hit, selected, pick.clone())),
            )
            .style(|s| s.flex_col().width_full())
            .into_any()
        },
    )
    .style(|s| s.width_full())
    .scroll()
    .ensure_visible(move || {
        let at = selected.get() as f64 * theme::GO_TO_ROW_HEIGHT;
        Rect::new(0.0, at, 1.0, at + theme::GO_TO_ROW_HEIGHT)
    })
    .style(move |s| {
        s.width_full()
            .height(list_height(state))
            .flex_shrink(0.0_f32)
            .overflow_x(floem::taffy::Overflow::Hidden)
            .margin_top(6.0)
    });

    Stack::vertical((
        Label::derived(|| "Close as duplicate".to_string()).style(|s| {
            s.height(HEADER_HEIGHT)
                .items_center()
                .padding_horiz(2.0)
                .font_size(theme::TEXT_META)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
                .selectable(false)
        }),
        Empty::new().style(|s| s.height(8.0)),
        input,
        list,
    ))
    .style(|s| s.flex_col().width_full())
    .debug_name("close-duplicate")
}

/// An issue the search found, as the `#` list draws it.
fn duplicate_row(
    index: usize,
    hit: &ReferenceHit,
    selected: RwSignal<usize>,
    pick: Rc<dyn Fn(usize)>,
) -> impl IntoView {
    let number = hit.reference.number;
    Container::new(Container::new(mention::reference_row(hit)).style(move |s| {
        let s = s
            .size_full()
            .items_center()
            .padding_horiz(8.0)
            .border_radius(crate::ui::menu::ROW_RADIUS);
        if selected.get() == index {
            s.background(theme::menu_hover())
        } else {
            s.hover(|s| s.background(theme::menu_pointed()))
        }
    }))
    .on_event_stop(el::Click, move |_, _| pick(index))
    .debug_name(format!("close-duplicate-row:{number}"))
    .style(|s| {
        clickable(s)
            .width_full()
            .height(theme::GO_TO_ROW_HEIGHT)
            .flex_shrink(0.0_f32)
    })
}
