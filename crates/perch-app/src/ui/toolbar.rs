//! Top bar: the All/Unread segmented control, search, sort, and group.

use floem::prelude::*;
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::dropdown::Dropdown;
use floem::views::{Container, Decorators, Label, Stack, TextInput};

use crate::filter::{Group, Sort};
use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::sidebar_glyph;

pub fn toolbar(state: AppState) -> impl IntoView {
    Stack::horizontal((
        segmented(state),
        search_box(state),
        labelled_dropdown("Sort by", Dropdown::new_rw(state.sort, Sort::ALL)),
        labelled_dropdown("Group by", Dropdown::new_rw(state.group, Group::ALL)),
    ))
    .style(|s| {
        s.height(theme::TOOLBAR_HEIGHT)
            .width_full()
            .items_center()
            .gap(10.0)
            .padding_horiz(16.0)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// All | Unread. A filter over one list, not a second query.
fn segmented(state: AppState) -> impl IntoView {
    Stack::horizontal((
        segment(state, "All", false),
        segment(state, "Unread", true),
    ))
    .style(|s| {
        s.items_center()
            .padding(2.0)
            .gap(2.0)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
    })
}

fn segment(state: AppState, text: &'static str, unread_value: bool) -> impl IntoView {
    let active = move || state.unread_only.get() == unread_value;

    Container::new(Label::derived(move || text).style(move |s| {
        s.font_size(theme::TEXT_BODY)
            .font_weight(if active() {
                FontWeight::SEMI_BOLD
            } else {
                FontWeight::NORMAL
            })
            .color(if active() { theme::FG } else { theme::FG_MUTED })
    }))
    .on_event_stop(el::Click, move |_, _| state.unread_only.set(unread_value))
    .style(move |s| {
        let base = s
            .height(24.0)
            .padding_horiz(12.0)
            .items_center()
            .justify_center()
            .border_radius(4.0)
            .hover(|s| s.cursor(CursorStyle::Pointer));
        if active() {
            base.background(theme::CANVAS)
                .box_shadow_blur(2.0)
                .box_shadow_color(theme::SHADOW_SOFT)
                .box_shadow_v_offset(1.0)
        } else {
            base
        }
    })
}

fn search_box(state: AppState) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Search, theme::FG_SUBTLE),
        TextInput::new(state.search)
            .placeholder("Search notifications")
            .style(|s| {
                s.flex_grow(1.0_f32)
                    .min_width(0.0)
                    .font_size(theme::TEXT_BODY)
                    .color(theme::FG)
                    .background(floem::peniko::Color::TRANSPARENT)
                    .border(0.0)
                    .padding(0.0)
                    .margin_left(7.0)
            }),
    ))
    .style(|s| {
        s.flex_grow(1.0_f32)
            .min_width(120.0)
            .max_width(460.0)
            .height(30.0)
            .items_center()
            .padding_horiz(10.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .focus_within(|s| {
                s.border_color(theme::ACCENT)
                    .box_shadow_blur(0.0)
                    .box_shadow_spread(3.0)
                    .box_shadow_color(theme::ACCENT_RING)
            })
    })
}

fn labelled_dropdown<T>(caption: &'static str, dropdown: Dropdown<T>) -> impl IntoView
where
    T: Clone + PartialEq + std::fmt::Debug + std::fmt::Display + 'static,
{
    Stack::horizontal((
        Label::derived(move || caption).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .margin_right(6.0)
        }),
        dropdown.style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::FG)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .padding_horiz(9.0)
                .height(30.0)
                .items_center()
                .hover(|s| s.background(theme::CANVAS_SUBTLE).cursor(CursorStyle::Pointer))
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}
