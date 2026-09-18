//! Top bar: the All/Unread segmented control, search, sort, and group.

use floem::prelude::*;
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Label, Stack, TextInput};

use crate::filter::{Group, Sort};
use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::sidebar_glyph;
use floem::menu::Menu;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};

pub fn toolbar(state: AppState) -> impl IntoView {
    Stack::horizontal((
        segmented(state),
        search_box(state),
        menu_picker("Sort by", state.sort, Sort::ALL),
        menu_picker("Group by", state.group, Group::ALL),
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
            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
    }))
    .on_event_stop(el::Click, move |_, _| state.unread_only.set(unread_value))
    .style(move |s| {
        let base = s
            .height(28.0)
            .padding_horiz(14.0)
            .items_center()
            .justify_center()
            .border_radius(4.0)
            .selectable(false)
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
            // The window's bare-letter shortcuts have to stand down while
            // somebody is typing a search, and only this view knows.
            .on_event_cont(el::FocusGained, move |_, _| state.search_focused.set(true))
            .on_event_cont(el::FocusLost, move |_, _| state.search_focused.set(false))
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
            .height(34.0)
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

/// A caption, the current value, and a native popup menu of the options.
///
/// floem's own Dropdown renders its label and its popup list outside this
/// app's styling, so its text never quite matched the rest of the window.
/// The button here is an ordinary label, and the menu is the platform's, so
/// both look like they belong.
fn menu_picker<T, const N: usize>(
    caption: &'static str,
    selected: RwSignal<T>,
    options: [T; N],
) -> impl IntoView
where
    T: Copy + PartialEq + std::fmt::Display + 'static,
{
    Stack::horizontal((
        Label::derived(move || caption).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .selectable(false)
                .margin_right(6.0)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
        Stack::horizontal((
            Label::derived(move || selected.get().to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG)
                    .selectable(false)
                    .margin_right(6.0)
                    .margin_bottom(theme::OPTICAL_TEXT_LIFT)
            }),
            sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
                .style(|s| s.size(11.0, 11.0)),
        ))
        .popout_menu(move || {
            let mut menu = Menu::new();
            for option in options {
                let label = option.to_string();
                menu = menu.item(label, |item| {
                    item.checked(selected.get_untracked() == option)
                        .action(move || selected.set(option))
                });
            }
            menu
        })
        .style(|s| {
            s.items_center()
                .height(28.0)
                .padding_horiz(9.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .selectable(false)
                .hover(|s| s.background(theme::CANVAS_SUBTLE).cursor(CursorStyle::Pointer))
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}
