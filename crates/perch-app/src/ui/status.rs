//! One-line footer: refresh state, token source, and any error.

use floem::peniko::Color;
use floem::prelude::*;
use floem::style::CursorStyle;
use floem::views::{Container, Decorators, Label, Stack};

use crate::backend::{Backend, Status};
use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{sidebar_glyph, spacer};

pub fn status_bar(state: AppState, backend: std::rc::Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        dot(state),
        Label::derived(move || message(state)).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(color_for(&state.status.get()))
                .text_ellipsis()
                .min_width(0.0)
        }),
        spacer(),
        Label::derived(move || {
            let n = state.visible().len();
            let total = state.threads.with(|t| t.len());
            if n == total {
                format!("{total} notifications")
            } else {
                format!("{n} of {total} notifications")
            }
        })
        .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE).margin_right(12.0)),
        refresh_button(backend),
    ))
    .style(|s| {
        s.height(theme::STATUS_HEIGHT)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .gap(7.0)
            .background(theme::CANVAS_SIDEBAR)
            .border_top(1.0)
            .border_color(theme::BORDER)
    })
}

fn dot(state: AppState) -> impl IntoView {
    floem::views::Empty::new().style(move |s| {
        s.size(7.0, 7.0)
            .border_radius(4.0)
            .flex_shrink(0.0_f32)
            .background(color_for(&state.status.get()))
    })
}

fn color_for(status: &Status) -> Color {
    match status {
        Status::Idle { .. } => theme::OPEN,
        Status::Refreshing => theme::ACCENT,
        Status::RateLimited { .. } => theme::DRAFT,
        Status::Error { .. } | Status::NoToken { .. } => theme::CLOSED,
    }
}

fn message(state: AppState) -> String {
    match state.status.get() {
        Status::Refreshing => "Refreshing…".into(),
        Status::Idle { at, source } => {
            format!("Updated {} · {}", timefmt::relative(at, jiff::Timestamp::now()), source)
        }
        Status::RateLimited { message } => message,
        Status::Error { message } => message,
        Status::NoToken { .. } => "Not signed in".into(),
    }
}

fn refresh_button(backend: std::rc::Rc<Backend>) -> impl IntoView {
    Container::new(sidebar_glyph(SidebarIcon::Refresh, theme::FG_MUTED))
        .on_event_stop(el::Click, move |_, _| backend.refresh())
        .style(|s| {
            s.size(22.0, 22.0)
                .items_center()
                .justify_center()
                .border_radius(4.0)
                .hover(|s| s.background(theme::CANVAS_HOVER).cursor(CursorStyle::Pointer))
        })
}
