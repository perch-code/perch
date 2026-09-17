//! Left rail: views, reason filters, and the repository list.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};

use crate::filter::ReasonFilter;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{avatar, count_badge, section_heading, sidebar_row};

/// The navigation panel: views, reason filters, and the repository list.
///
/// Sits beside the activity rail and hides when the rail's current entry is
/// clicked, the way an editor's explorer does.
pub fn nav_panel(state: AppState, review: ReviewState) -> impl IntoView {
    Stack::vertical((
        Empty::new().style(|s| s.height(8.0).width_full().flex_shrink(0.0_f32)),
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


/// The activity rail.
///
/// Modelled on an editor's: a narrow column of destinations that is always
/// on screen, with the current one marked by a stripe down its left edge.
/// Clicking the entry that is already showing hides the panel beside it, and
/// clicking it again brings the panel back. There is no separate collapse
/// button, because the rail itself is the control.
///
/// It is wider than the 48 pixels an editor uses because the traffic lights
/// sit at the top of it and need the room. Since the rail is never hidden,
/// nothing else in the window has to leave space for them.
pub fn activity_rail(state: AppState, review: ReviewState) -> impl IntoView {
    use floem::views::drag_window_area;

    Stack::vertical((
        // The lights sit over this.
        drag_window_area(Empty::new().style(|s| s.size_full()))
            .style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().flex_shrink(0.0_f32)),
        rail_entry(state, review),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
    ))
    .style(|s| {
        s.width(theme::ACTIVITY_RAIL_WIDTH)
            .height_full()
            .flex_col()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_RAIL)
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
        if review.is_showing() {
            // Coming back from a pull request: show the inbox and its panel.
            review.close();
            state.sidebar_open.set(true);
        } else {
            // Already here, so this is the collapse control.
            state.sidebar_open.update(|open| *open = !*open);
        }
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
