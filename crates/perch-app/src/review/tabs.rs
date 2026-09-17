//! Open file tabs across the top of the editor.

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};

use crate::review::model::ReviewState;
use crate::theme;

pub fn tab_bar(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || state.tabs.with(|t| t.is_empty()),
        move |empty| {
            if empty {
                empty_tab_bar().into_any()
            } else {
                open_tabs(state).into_any()
            }
        },
    )
}

fn open_tabs(state: ReviewState) -> impl IntoView {
    dyn_stack(
        move || state.tabs.get(),
        |tab| tab.path.clone(),
        move |tab| tab_view(state, tab.path),
    )
    .style(|s| s.items_center().height_full())
    .scroll()
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn tab_view(state: ReviewState, path: String) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();

    let is_active = {
        let path = path.clone();
        move || state.active.with(|a| a.as_deref() == Some(path.as_str()))
    };
    let is_active_style = is_active.clone();
    let is_active_weight = is_active.clone();

    let select_path = path.clone();
    let close_path = path.clone();

    Stack::horizontal((
        Label::derived(move || name.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if is_active_weight() { theme::FG } else { theme::FG_MUTED })
                .font_weight(if is_active_weight() {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .margin_right(8.0)
        }),
        close_button(state, close_path),
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.open_tab(&select_path);
    })
    .style(move |s| {
        let base = s
            .height_full()
            .items_center()
            .padding_horiz(12.0)
            .border_right(1.0)
            .border_color(theme::BORDER_MUTED)
            .hover(|s| s.cursor(CursorStyle::Pointer));
        if is_active_style() {
            // The active tab reads as continuous with the code beneath it.
            base.background(theme::CODE_BG)
                .border_bottom(2.0)
                .border_color(theme::ACCENT)
        } else {
            base.background(Color::TRANSPARENT)
        }
    })
}

fn close_button(state: ReviewState, path: String) -> impl IntoView {
    Container::new(Label::derived(|| "\u{00d7}".to_string()).style(|s| {
        s.font_size(13.0).color(theme::FG_SUBTLE)
    }))
    .on_event_stop(el::Click, move |_, _| state.close_tab(&path))
    .style(|s| {
        s.size(16.0, 16.0)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

/// Keeps the bar's height when nothing is open yet.
fn empty_tab_bar() -> impl IntoView {
    Empty::new().style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}
