//! Window layout.

pub mod list;
pub mod row;
pub mod sidebar;
pub mod status;
pub mod toolbar;
pub mod widgets;

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::SignalUpdate;
use floem::views::{Decorators, Empty, Stack, dyn_container};

use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;

/// The platform's command modifier.
const OS_MOD: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

/// The strip that stands in for the title bar.
///
/// The window is drawn without one, so this reserves room for the traffic
/// lights, carries the drawer toggle, and is draggable everywhere else so
/// the window can still be moved.
fn titlebar(state: AppState) -> impl IntoView {
    use floem::views::drag_window_area;

    drag_window_area(
        Stack::horizontal((
            Empty::new().style(|s| {
                s.width(theme::TRAFFIC_LIGHTS_WIDTH).height_full().flex_shrink(0.0_f32)
            }),
            drawer_toggle(state),
            Empty::new().style(|s| s.flex_grow(1.0_f32).height_full()),
        ))
        .style(|s| s.size_full().items_center()),
    )
    .style(|s| {
        s.height(theme::TITLEBAR_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_SIDEBAR)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn drawer_toggle(state: AppState) -> impl IntoView {
    use floem::views::Container;

    Container::new(widgets::sidebar_glyph(
        crate::icons::SidebarIcon::Sidebar,
        theme::FG_MUTED,
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.sidebar_open.update(|open| *open = !*open);
    })
    .style(|s| {
        widgets::clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

pub fn app_view(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let review_backend = backend.clone();
    let open_thread: list::OpenThread = Rc::new({
        let backend = backend.clone();
        move |repo: &str, number: u64| {
            review.preparing.set(crate::review::model::Preparing::Working {
                message: "Reading the pull request".into(),
                percent: None,
            });
            // Showing the review view immediately, with its progress card,
            // beats leaving the inbox looking like the click did nothing.
            review.pull_request.set(None);
            review.opening.set(true);
            backend.open_pull_request(repo, number);
        }
    });

    Stack::vertical((
        titlebar(state),
        Stack::horizontal((
            // The drawer slides away entirely rather than narrowing, so the
            // list gets the whole window when it is closed.
            dyn_container(
                move || state.sidebar_open.get(),
                move |open| {
                    if open {
                        sidebar::sidebar(state, review).into_any()
                    } else {
                        Empty::new().into_any()
                    }
                },
            ),
            // The review view replaces the inbox in place, so the sidebar and
            // the status line stay where they were.
            dyn_container(
                move || review.is_showing(),
                move |open| {
                    if open {
                        crate::review::review_view(state, review, review_backend.clone())
                            .into_any()
                    } else {
                        Stack::vertical((
                            toolbar::toolbar(state),
                            list::list(state, open_thread.clone()),
                        ))
                            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full().flex_col())
                            .into_any()
                    }
                },
            )
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
        status::status_bar(state, review, backend),
    ))
    // Window-level shortcuts. Key events bubble from whatever has focus, so
    // a handler on the root view catches them wherever the reader is.
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if !key.modifiers.contains(OS_MOD) {
            return EventPropagation::Continue;
        }
        // Physical key codes, so the shortcuts hold on any keyboard layout.
        match key.code {
            // Close the open file, as in any editor.
            Code::KeyW => {
                if let Some(path) = review.active.get() {
                    review.close_tab(&path);
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            }
            // Show or hide the drawer.
            Code::Backslash => {
                state.sidebar_open.update(|open| *open = !*open);
                EventPropagation::Stop
            }
            // Back to the inbox.
            Code::BracketLeft => {
                if review.is_showing() {
                    review.close();
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            }
            _ => EventPropagation::Continue,
        }
    })
    .style(|s| {
        s.size_full()
            .flex_col()
            .font_family(theme::FONT_STACK.to_string())
            .font_size(theme::TEXT_BODY)
            .color(theme::FG)
            .background(theme::CANVAS)
    })
}
