//! Review in the bar: one button, and one popover under it.
//!
//! The popover is the reader's review in the making. On top, the agents
//! who can review the branch, folded to one line with Run beside it; their
//! findings arrive as the reader's drafts. Under them, on an open pull
//! request, those drafts, a summary and a verdict for GitHub. Unfolding
//! the agents' settings folds the reader's review to a line. A branch with
//! no pull request has only the agents, unfolded.
//!
//! `review_launch` draws the agents' part and `submit_review` the rest;
//! this module is the button, the popover around them, and opening and
//! shutting it.

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::review::{review_launch, submit_review};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_action, sidebar_glyph};

/// The button, dark on a pull request the reader can review and quiet on
/// a branch, and the popover under it. It says what is happening: a
/// review being submitted, the agents at work, or the drafts waiting.
pub fn control(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    review_launch::watch(app, state, backend.clone());
    // Opening asks again what the reviewers support and what the item's
    // reviews are, as the launcher did on its own.
    let load_backend = backend.clone();
    Effect::new(move |was: Option<bool>| {
        let open = state.review_open.get();
        if open && was != Some(true) {
            let item_id = state
                .item
                .with_untracked(|item| item.as_ref().map(|item| item.id.clone()));
            if let Some(item_id) = item_id {
                app.begin_review_dashboard_load(&item_id);
                load_backend.load_review_dashboard(&item_id);
            }
        }
        open
    });

    // Memos, so the threads changing under the button do not restyle it.
    let prominent = Memo::new(move |_| submit_review::is_shown(state));
    let count = Memo::new(move |_| state.drafts().len());
    let label = move || {
        if state.submit_review_pending.with(Option::is_some) {
            return "Submitting…".to_string();
        }
        if let Some(group) = review_launch::active_group(app, state) {
            return review_launch::progress_label(&group);
        }
        if review_launch::launching(app, state) {
            return "Starting review…".to_string();
        }
        match count.get() {
            0 => "Review".to_string(),
            n => format!("Review \u{b7} {n}"),
        }
    };
    let ink = move || {
        if prominent.get() {
            theme::canvas()
        } else {
            theme::fg()
        }
    };
    let inner = Stack::horizontal((
        Label::derived(label).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(ink())
                .selectable(false)
                .pointer_events_none()
        }),
        dyn_container(
            move || prominent.get(),
            |prominent| {
                let color = if prominent { theme::canvas } else { theme::fg_subtle };
                sidebar_glyph(SidebarIcon::ChevronDown, color)
                    .style(|s| s.size(9.0, 9.0).pointer_events_none())
            },
        )
        .style(|s| s.margin_left(7.0).pointer_events_none()),
    ))
    .style(|s| s.items_center().pointer_events_none());
    let button = Container::new(inner);
    let button_id = button.view_id();
    let toggle: Rc<dyn Fn()> = Rc::new(move || {
        if state.review_open.get_untracked() {
            cancel(state, true);
        } else {
            open(state, Some(button_id));
        }
    });
    let click = toggle.clone();
    let button = button
        .on_event_stop(el::Click, move |_, _| click())
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle();
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            let s = clickable(s)
                .height(30.0)
                .padding_horiz(12.0)
                .items_center()
                .border_radius(9.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()));
            if prominent.get() {
                s.background(theme::fg())
                    .hover(|s| s.background(theme::action_hover()))
            } else {
                let open = state.review_open.get();
                s.background(if open { theme::border() } else { theme::badge_bg() })
                    .hover(|s| s.background(theme::border()))
            }
        })
        .debug_name("review-control");

    Stack::new((
        button,
        dyn_container(
            move || state.review_open.get(),
            move |open| {
                if open {
                    popover(app, state, backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(34.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape && key.modifiers.is_empty() && state.review_open.get_untracked()
        {
            cancel(state, true);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| s.position(Position::Relative).flex_shrink(0.0_f32))
}

/// Open the popover over whatever else hangs from the bar.
pub fn open(state: ReviewState, opener: Option<floem::ViewId>) {
    if state.item.with_untracked(Option::is_none) {
        return;
    }
    crate::ui::agents::dropdown::close(state, false);
    crate::ui::usage::close(state, false);
    crate::ui::feedback::close(state, false);
    state.review_opener.set(opener);
    state.review_open.set(true);
}

/// A dismissal the reader asked for. It never pretends to cancel a review
/// already sent to GitHub: the popover stays to say how that went.
pub(crate) fn cancel(state: ReviewState, restore_focus: bool) {
    if state.submit_review_pending.with_untracked(Option::is_none) {
        close(state, restore_focus);
    }
}

/// Shut it, whatever is under way; navigation and the other popovers do.
/// Reopening shows a submission's outcome all the same.
pub(crate) fn close(state: ReviewState, restore_focus: bool) {
    state.review_open.set(false);
    state.review_agents_expanded.set(false);
    state.submit_review_focused.set(false);
    state.submit_review_confirm_discard.set(false);
    state.submit_review_outcome.set(None);
    let opener = state.review_opener.get_untracked();
    state.review_opener.set(None);
    if restore_focus {
        if let Some(opener) = opener {
            opener.request_focus();
        }
    }
}

fn popover(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let submitting = Memo::new(move |_| submit_review::is_shown(state));
    let close_button = icon_action(
        SidebarIcon::Close,
        "review-close",
        move || state.submit_review_pending.with(Option::is_none),
        move || cancel(state, true),
    );
    // The keyboard goes to the summary on a pull request, and to the
    // close control on a branch, where there is no text to start on.
    let close_id = close_button.view_id();
    let asked = Rc::new(Cell::new(false));
    let close_button = close_button.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) && !submitting.get_untracked() {
            close_id.request_focus();
        }
    });
    let title = state
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.title().to_string()))
        .unwrap_or_default();

    let submit_backend = backend.clone();
    let content = Stack::vertical((
        Stack::horizontal((
            Stack::vertical((
                Label::derived(|| "Review".to_string()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                }),
                Label::derived(move || title.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_subtle())
                        .text_ellipsis()
                        .min_width(0.0)
                        .margin_top(2.0)
                }),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
            close_button,
        ))
        .style(|s| s.items_start().width_full().gap(8.0)),
        review_launch::agents_section(app, state, backend, submitting),
        // One of the two parts unfolds at a time, so the popover stays
        // the height of either alone. The review is built anew on each
        // submission that comes back with something left to say, so the
        // summary box starts empty: what was in it posted.
        dyn_container(
            move || {
                (
                    submitting.get(),
                    state.review_agents_expanded.get(),
                    state.submit_review_generation.get(),
                )
            },
            move |(shown, agents, _)| match (shown, agents) {
                (false, _) => Empty::new().into_any(),
                (true, true) => submit_review::folded(state).into_any(),
                (true, false) => {
                    submit_review::section(app, state, submit_backend.clone()).into_any()
                }
            },
        )
        .style(|s| {
            s.flex_col()
                .width_full()
                .min_height(0.0)
                .flex_shrink(1.0_f32)
        }),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .min_height(0.0)
            .flex_shrink(1.0_f32)
            .gap(14.0)
    });

    // No taller than the window under the bar, with a margin; the drafts
    // scroll to make up the difference.
    Container::new(content)
        .style(move |s| {
            let room = app.window_height.get() - theme::REVIEW_HEADER_HEIGHT - 12.0;
            s.flex_col()
                .width(430.0)
                .max_width_full()
                .apply_if(room > 0.0, |s| s.max_height(room))
                .padding(14.0)
                .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
        })
        .debug_name("review-popover")
}
