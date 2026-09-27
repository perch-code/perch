//! What is an issue's alone: Start work and the state its glyph is drawn
//! in. The body is Conversation, drawn from `review::cards`, and the header
//! over it is the same bar every item has.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::SignalWith;
use floem::views::{dyn_container, Decorators, Empty};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::ui::widgets::text_button;

/// Cut a branch from the default branch and open it. When one exists
/// already, opens that. The issue's primary action, where a pull
/// request's is Submit review.
pub fn start_work_button(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.item.with(|i| i.as_ref().map(|i| i.id.clone())),
        move |id| {
            let Some(id) = id else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            text_button(
                || "Start work".to_string(),
                SidebarIcon::Branch,
                move || {
                    let navigation_id = backend.start_work(&id);
                    app.begin_open("Cutting a branch", navigation_id);
                },
            )
            .debug_name("start work button")
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

