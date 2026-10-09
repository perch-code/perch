//! Back and Forward, as VS Code has them: through the places the window
//! has shown, across items, Conversation and Code, and the files and
//! lines in them. `Ctrl--` and `Ctrl-Shift--`, the mouse's back and
//! forward buttons, and the arrows in the bar. The history itself is the
//! review's (`review::model::History`); what is here is the part that
//! opens another item, which takes the backend.

use std::rc::Rc;

use floem::prelude::*;
use floem::views::{Container, Decorators, Stack};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::model::{ReviewState, HISTORY_CAP};
use crate::state::AppState;
use crate::theme;

/// Go back, or forward, one place. A place on an item that is gone from
/// the sidebar is passed over for the one beyond it.
pub fn travel(state: AppState, review: ReviewState, backend: &Rc<Backend>, back: bool) {
    for _ in 0..=HISTORY_CAP {
        let Some(place) = review.travel(back) else {
            return;
        };
        let Some(item) = place.item.as_deref().and_then(|id| state.item(id)) else {
            continue;
        };
        review.travel_to(place, |_| super::open_item(state, review, backend, &item));
        return;
    }
}

/// The arrows: Back, then Forward, each faded while there is nowhere to
/// go that way.
pub fn buttons(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        arrow(state, review, backend.clone(), true),
        arrow(state, review, backend, false),
    ))
    .debug_name("history arrows")
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}

fn arrow(state: AppState, review: ReviewState, backend: Rc<Backend>, back: bool) -> impl IntoView {
    let can = move || {
        review.history.with(|h| {
            if back {
                h.can_go_back()
            } else {
                h.can_go_forward()
            }
        })
    };
    let glyph = if back {
        SidebarIcon::Back
    } else {
        SidebarIcon::Forward
    };
    let color = move || {
        if can() {
            theme::fg_muted()
        } else {
            theme::fg_subtle()
        }
    };
    let button = Container::new(crate::ui::widgets::icon_in(
        move || icons::sidebar_icon(glyph, color()),
        color,
    ))
    .on_event_stop(floem::event::listener::Click, move |_, _| {
        travel(state, review, &backend, back)
    })
    .debug_name(if back { "back" } else { "forward" })
    .style(move |s| {
        let s = s
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .pointer_events_auto();
        if can() {
            crate::ui::widgets::clickable(s).hover(|s| s.background(theme::nav_hover()))
        } else {
            s.selectable(false)
        }
    });
    crate::review::cards::tip(
        button,
        if back {
            "Go Back (⌃-)".to_string()
        } else {
            "Go Forward (⌃⇧-)".to_string()
        },
    )
}
