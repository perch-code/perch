//! Open file tabs across the top of the editor.

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};

use crate::review::model::ReviewState;
use crate::theme;

pub fn tab_bar(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
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
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
        change_nav(state),
    ))
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// Jump between the changed regions of the open file.
///
/// The count is the point as much as the buttons: it says at a glance how
/// much of this file the pull request actually touched.
fn change_nav(state: ReviewState) -> impl IntoView {
    let count = move || state.active_file().map(|f| f.change_count()).unwrap_or(0);

    dyn_container(count, move |n| {
        if n == 0 {
            Empty::new().into_any()
        } else {
            Stack::horizontal((
                nav_button(state, "\u{2191}", false),
                Label::derived(move || {
                    if n == 1 {
                        "1 change".to_string()
                    } else {
                        format!("{n} changes")
                    }
                })
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .margin_horiz(6.0)
                }),
                nav_button(state, "\u{2193}", true),
            ))
            .style(|s| s.items_center().padding_horiz(10.0).flex_shrink(0.0_f32))
            .into_any()
        }
    })
    .style(|s| s.flex_shrink(0.0_f32).height_full().items_center())
}

fn nav_button(state: ReviewState, glyph: &'static str, forward: bool) -> impl IntoView {
    Container::new(Label::derived(move || glyph.to_string()).style(|s| {
        s.font_size(12.0).color(theme::FG_MUTED)
    }))
    .on_event_stop(el::Click, move |_, _| {
        let Some(file) = state.active_file() else { return };
        let from = state.cursor_line.get();
        let target = if forward {
            file.buffer.next_change(from)
        } else {
            file.buffer.previous_change(from)
        };
        if let Some(display_line) = target {
            state.cursor_line.set(display_line);
            // The editor scrolls by head line, so translate back through the
            // row table; a ghost line has none, so use the line after it.
            let head = file
                .buffer
                .rows
                .iter()
                .skip(display_line)
                .find_map(|r| r.head_line())
                .unwrap_or(display_line);
            state.jump_to_line.set(Some(head));
        }
    })
    .style(|s| {
        s.size(20.0, 20.0)
            .items_center()
            .justify_center()
            .border_radius(4.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
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
    Empty::new().style(|s| s.height_full().width_full())
}
