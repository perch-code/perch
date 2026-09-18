//! Conversation: the item's description, everything said about it on
//! GitHub, and every thread the reader has had with an agent about it, in
//! one column of cards in time order, with the composer at the end.
//!
//! This is the skeleton the cards fill in. Until then an issue shows its
//! view as it was, and a pull request its description alone.

use std::rc::Rc;

use floem::prelude::*;
use floem::views::{Decorators, Stack};

use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::review::{issue, threads, OpenFile};
use crate::state::AppState;

/// The column beside the thread column, which stays where it was for an
/// item with a checkout.
pub fn view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: crate::ui::items::OpenItem,
    open_file: OpenFile,
) -> impl IntoView {
    let column_backend = backend.clone();
    Stack::horizontal((
        issue::issue_body(app, state, backend, open_item),
        floem::views::dyn_container(
            move || state.threads_open.get(),
            move |open| {
                if open {
                    floem::views::Container::new(Stack::horizontal((
                        threads::resize_handle(state),
                        threads::thread_column(app, state, column_backend.clone(), open_file.clone()),
                    )))
                    .style(|s| s.height_full().flex_shrink(0.0_f32).position(floem::style::Position::Relative))
                    .into_any()
                } else {
                    floem::views::Empty::new().into_any()
                }
            },
        ),
    ))
    .style(|s| s.size_full().min_height(0.0))
}
