//! Left rail: views, reason filters, and the repository list.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};

use crate::filter::ReasonFilter;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{avatar, count_badge, section_heading, sidebar_row};

pub fn sidebar(state: AppState, review: ReviewState) -> impl IntoView {
    Stack::vertical((
        // The window has no title bar, so the drawer starts with the room
        // the traffic lights need.
        crate::ui::widgets::drawer_controls(state, true)
            .style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().padding_left(6.0)),
        views_section(state, review),
        section_heading("Filters"),
        filters_section(state),
        section_heading("Repositories"),
        repos_section(state),
    ))
    .style(|s| {
        s.width(theme::SIDEBAR_WIDTH)
            .height_full()
            .flex_col()
            .background(theme::CANVAS_SIDEBAR)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

fn views_section(state: AppState, review: ReviewState) -> impl IntoView {
    let unread = move || state.counts().unread;

    Stack::vertical((
        // Inbox is the only implemented view; it is always the active one.
        Container::new(inbox_row(unread)).on_event_stop(el::Click, move |_, _| {
            // Doubles as the way back from a pull request.
            review.close();
            state.clear_filters();
        }),
        // Saved and Done need endpoints the REST API does not expose. They
        // are shown so the rail matches the web UI, and are inert.
        sidebar_row(SidebarIcon::Bookmark, "Saved".into(), || false, false, None),
        sidebar_row(SidebarIcon::Check, "Done".into(), || false, false, None),
    ))
    .style(|s| s.width_full().gap(1.0))
}

fn inbox_row(unread: impl Fn() -> usize + 'static + Copy) -> impl IntoView {
    use floem::text::FontWeight;
    use floem::views::{Label, Stack};

    let content = Stack::horizontal((
        crate::ui::widgets::sidebar_glyph(SidebarIcon::Inbox, theme::FG),
        Label::derived(|| "Inbox").style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .margin_left(9.0)
                .flex_grow(1.0_f32)
        }),
        count_badge(unread, true),
    ))
    .style(|s| s.items_center().width_full());

    Container::new(content).style(|s| {
        crate::ui::widgets::clickable(s)
            .width_full()
            .height(34.0)
            .items_center()
            .padding_horiz(10.0)
            .margin_horiz(6.0)
            .border_radius(theme::RADIUS)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
    })
}

fn filters_section(state: AppState) -> impl IntoView {
    dyn_stack(
        || ReasonFilter::ALL,
        |f| *f,
        move |f| {
            let selected = move || state.reason_filter.get() == Some(f);
            let count = move || state.counts().by_reason.get(&f).copied().unwrap_or(0);
            Container::new(sidebar_row(
                icon_for(f),
                f.label().to_string(),
                selected,
                true,
                Some(Box::new(count)),
            ))
            .on_event_stop(el::Click, move |_, _| state.toggle_reason(f))
        },
    )
    .style(|s| s.flex_col().width_full().gap(1.0))
}

fn icon_for(f: ReasonFilter) -> SidebarIcon {
    match f {
        ReasonFilter::Assigned => SidebarIcon::Target,
        ReasonFilter::Participating => SidebarIcon::Comment,
        ReasonFilter::Mentioned => SidebarIcon::Mention,
        ReasonFilter::TeamMentioned => SidebarIcon::People,
        ReasonFilter::ReviewRequested => SidebarIcon::Eye,
    }
}

fn repos_section(state: AppState) -> impl IntoView {
    dyn_stack(
        move || state.counts().repos,
        |(name, _, _)| name.clone(),
        move |(name, count, avatar_url)| {
            let click_name = name.clone();
            Container::new(repo_row(state, name, count, avatar_url))
                .on_event_stop(el::Click, move |_, _| state.toggle_repo(&click_name))
        },
    )
    .style(|s| s.flex_col().width_full().gap(1.0).padding_bottom(10.0))
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

fn repo_row(state: AppState, name: String, count: usize, avatar_url: String) -> impl IntoView {
    use floem::peniko::Color;
    use floem::text::FontWeight;
    use floem::views::{Label, Stack};

    // Owner avatars make a long repository list scannable at a glance.
    let seed = name.clone();
    let label_text = name.clone();

    // One clone per closure: the capture is a String, so these cannot share a
    // single `Copy` predicate the way the reason filters do.
    let (sel_a, sel_b, sel_c) = (name.clone(), name.clone(), name.clone());
    let selected_text = move || state.repo_filter.with(|r| r.as_deref() == Some(sel_a.as_str()));
    let selected_weight = move || state.repo_filter.with(|r| r.as_deref() == Some(sel_b.as_str()));
    let selected_box = move || state.repo_filter.with(|r| r.as_deref() == Some(sel_c.as_str()));

    let content = Stack::horizontal((
        avatar(state, move || avatar_url.clone(), 16.0, seed).style(|s| s.margin_right(9.0)),
        Label::derived(move || label_text.clone()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .color(if selected_text() { theme::FG } else { theme::FG_MUTED })
                .font_weight(if selected_weight() {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_ellipsis()
                .flex_grow(1.0_f32)
                .min_width(0.0)
        }),
        count_badge(move || count, false),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0));

    Container::new(content).style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .width_full()
            .height(34.0)
            .items_center()
            .padding_horiz(10.0)
            .margin_horiz(6.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER).cursor(floem::style::CursorStyle::Pointer));
        if selected_box() {
            base.background(theme::ACCENT_SUBTLE)
                .border(1.0)
                .border_color(theme::ACCENT_RING)
        } else {
            base.border(1.0).border_color(Color::TRANSPARENT)
        }
    })
}


/// The drawer, collapsed to icons.
///
/// Modelled on an editor's activity bar: a narrow column of destinations,
/// the current one marked with a stripe down its left edge. Only the inbox
/// lives here so far.
///
/// It keeps the traffic lights, which is why it is wider than the 48 pixels
/// an editor would use. Because this column is always present in one form or
/// the other, nothing else in the window has to leave room for them.
pub fn collapsed_rail(state: AppState, review: ReviewState) -> impl IntoView {
    use floem::views::drag_window_area;

    Stack::vertical((
        // The lights sit over this.
        drag_window_area(Empty::new().style(|s| s.size_full()))
            .style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().flex_shrink(0.0_f32)),
        Container::new(crate::ui::widgets::drawer_toggle(state)).style(|s| {
            s.width_full()
                .height(34.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
        }),
        Empty::new().style(|s| {
            s.height(1.0)
                .width(theme::RAIL_ENTRY_SIZE)
                .margin_vert(6.0)
                .flex_shrink(0.0_f32)
                .background(theme::BORDER_MUTED)
        }),
        rail_entry(state, review),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
    ))
    .style(|s| {
        s.width(theme::COLLAPSED_RAIL_WIDTH)
            .height_full()
            .flex_col()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_SIDEBAR)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

/// The inbox entry: an icon, the unread count, and a stripe when it is the
/// view on screen.
fn rail_entry(state: AppState, review: ReviewState) -> impl IntoView {
    use floem::peniko::Color;
    use floem::style::Position;

    let unread = move || state.counts().unread;
    // The inbox is showing whenever a pull request is not.
    let active = move || !review.is_showing();

    Container::new(
        Container::new(Stack::horizontal((
            dyn_container(active, move |on| {
                crate::ui::widgets::sidebar_glyph(
                    SidebarIcon::Inbox,
                    if on { theme::FG } else { theme::FG_SUBTLE },
                )
                .style(|s| s.size(20.0, 20.0))
                .into_any()
            })
            .style(|s| s.size(20.0, 20.0)),
            unread_badge(unread),
        )))
        .style(move |s| {
            let base = crate::ui::widgets::clickable(s)
                .size(theme::RAIL_ENTRY_SIZE, theme::RAIL_ENTRY_SIZE)
                .items_center()
                .justify_center()
                .border_radius(theme::RADIUS)
                .position(Position::Relative);
            if active() {
                base.background(theme::CANVAS)
            } else {
                base.hover(|s| s.background(theme::CANVAS_HOVER))
            }
        }),
    )
    .on_event_stop(el::Click, move |_, _| {
        review.close();
        state.clear_filters();
    })
    .style(move |s| {
        // The stripe is the container's own left border, so it runs the full
        // height of the entry rather than floating beside it.
        let base = s
            .width_full()
            .height(theme::RAIL_ENTRY_SIZE)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center();
        if active() {
            base.border_left(2.0).border_color(theme::ACCENT)
        } else {
            base.border_left(2.0).border_color(Color::TRANSPARENT)
        }
    })
}

/// A small count over the corner of a rail icon.
fn unread_badge(unread: impl Fn() -> usize + 'static + Copy) -> impl IntoView {
    use floem::style::Position;

    dyn_container(
        unread,
        move |n| {
            if n == 0 {
                Empty::new().into_any()
            } else {
                Container::new(
                    Label::derived(move || if n > 99 { "99+".to_string() } else { n.to_string() })
                        .style(|s| {
                            s.font_size(9.0)
                                .color(theme::CANVAS)
                                .selectable(false)
                                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                        }),
                )
                .style(|s| {
                    s.position(Position::Absolute)
                        .inset_right(-4.0)
                        .inset_bottom(-3.0)
                        .height(14.0)
                        .min_width(14.0)
                        .padding_horiz(3.0)
                        .border_radius(7.0)
                        .background(theme::ACCENT)
                        .items_center()
                        .justify_center()
                })
                .into_any()
            }
        },
    )
}
