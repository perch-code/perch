//! Window layout.

pub mod list;
pub mod row;
pub mod sidebar;
pub mod status;
pub mod toolbar;
pub mod widgets;

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::SignalUpdate;
use floem::views::{Decorators, Stack, dyn_container};

use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;

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
        Stack::horizontal((
            sidebar::sidebar(state, review),
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
    .style(|s| {
        s.size_full()
            .flex_col()
            .font_family(theme::FONT_STACK.to_string())
            .font_size(theme::TEXT_BODY)
            .color(theme::FG)
            .background(theme::CANVAS)
    })
}
