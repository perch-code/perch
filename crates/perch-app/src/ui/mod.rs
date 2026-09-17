//! Window layout.

pub mod list;
pub mod row;
pub mod sidebar;
pub mod status;
pub mod toolbar;
pub mod widgets;

use std::rc::Rc;

use floem::prelude::*;
use floem::views::{Decorators, Stack};

use crate::backend::Backend;
use crate::state::AppState;
use crate::theme;

pub fn app_view(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        Stack::horizontal((
            sidebar::sidebar(state),
            Stack::vertical((toolbar::toolbar(state), list::list(state)))
                .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full().flex_col()),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
        status::status_bar(state, backend),
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
