//! Open file tabs, shaped like an editor's.
//!
//! The active tab takes the editor's own background and an accent stripe
//! along its top, so it reads as continuous with the code below it. Only the
//! inactive tabs carry the rule that separates the strip from the editor;
//! drawing that rule under the active tab too would cut it off from the file
//! it is showing.

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, CustomStylable};
use floem::views::scroll::ScrollCustomStyle;
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack,
};

use crate::review::model::ReviewState;
use crate::theme;

pub fn tab_bar(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        dyn_container(
            move || state.tabs.with(|t| t.is_empty()),
            move |empty| {
                if empty {
                    Empty::new().style(|s| s.size_full()).into_any()
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
    // No visible bar on a strip this short: it would sit over the tab names,
    // and the wheel still scrolls. A trackpad's vertical scroll moves the
    // strip sideways, which is the only direction it can go.
    .custom_style(|s: ScrollCustomStyle| s.hide_bars(true).vertical_scroll_as_horizontal(true))
    .style(|s| s.height_full().width_full())
}

fn tab_view(state: ReviewState, path: String) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();

    let (active_box, active_text) = (path.clone(), path.clone());
    let is_active_box = move || state.active.with(|a| a.as_deref() == Some(active_box.as_str()));
    let is_active_text = move || state.active.with(|a| a.as_deref() == Some(active_text.as_str()));

    let dot_path = path.clone();
    let count_path = path.clone();
    let select_path = path.clone();
    let close_path = path.clone();

    Stack::horizontal((
        language_dot(state, dot_path),
        Label::derived(move || name.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if is_active_text() { theme::FG } else { theme::FG_MUTED })
                .text_ellipsis()
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .margin_right(6.0)
        }),
        change_count(state, count_path),
        close_button(state, close_path),
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.open_tab(&select_path);
    })
    .style(move |s| {
        let active = is_active_box();
        let base = s
            .height_full()
            .items_center()
            .padding_left(10.0)
            .padding_right(6.0)
            .max_width(theme::TAB_MAX_WIDTH)
            .flex_shrink(0.0_f32)
            .selectable(false)
            .border_right(1.0)
            .border_color(theme::BORDER_MUTED)
            .hover(|s| s.cursor(CursorStyle::Pointer));

        if active {
            // Same surface as the editor, with the stripe on top. Nothing
            // along the bottom, so the tab and the file join up.
            base.background(theme::CODE_BG)
                .border_top(2.0)
                .border_color(theme::ACCENT)
        } else {
            base.background(Color::TRANSPARENT)
                .border_bottom(1.0)
                .border_color(theme::BORDER)
                .hover(|s| s.background(theme::CANVAS_HOVER))
        }
    })
}

/// A dot coloured by language, standing in for a file-type icon.
fn language_dot(state: ReviewState, path: String) -> impl IntoView {
    let color = move || theme::language_color(state.file(&path).and_then(|f| f.language));

    Empty::new().style(move |s| {
        s.size(7.0, 7.0)
            .border_radius(4.0)
            .flex_shrink(0.0_f32)
            .margin_right(7.0)
            .background(color())
    })
}

/// How much of this file the pull request touched.
fn change_count(state: ReviewState, path: String) -> impl IntoView {
    let count = move || state.file(&path).map(|f| f.change_count()).unwrap_or(0);

    dyn_container(count, move |n| {
        if n == 0 {
            Empty::new().into_any()
        } else {
            Label::derived(move || n.to_string())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::STATUS_MODIFIED)
                        .selectable(false)
                        .margin_right(5.0)
                })
                .into_any()
        }
    })
    .style(|s| s.flex_shrink(0.0_f32))
}

fn close_button(state: ReviewState, path: String) -> impl IntoView {
    Container::new(
        crate::ui::widgets::sidebar_glyph(crate::icons::SidebarIcon::Close, theme::FG_SUBTLE)
            .style(|s| s.size(11.0, 11.0)),
    )
    .on_event_stop(el::Click, move |_, _| state.close_tab(&path))
    .style(|s| {
        s.size(17.0, 17.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
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
                nav_button(state, false),
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
                        .selectable(false)
                        .margin_horiz(6.0)
                }),
                nav_button(state, true),
            ))
            .style(|s| s.items_center().padding_horiz(10.0).flex_shrink(0.0_f32))
            .into_any()
        }
    })
    .style(|s| {
        s.flex_shrink(0.0_f32)
            .height_full()
            .items_center()
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn nav_button(state: ReviewState, forward: bool) -> impl IntoView {
    use crate::icons::SidebarIcon;
    use crate::ui::widgets::sidebar_glyph;

    let glyph = if forward { SidebarIcon::ChevronDown } else { SidebarIcon::ChevronUp };

    Container::new(sidebar_glyph(glyph, theme::FG_MUTED))
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
                // The editor scrolls by head line, so translate back through
                // the row table; a ghost line has none, so use the line after.
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
