//! Left rail: views, reason filters, and the repository list.

use floem::prelude::*;
use floem::reactive::SignalWith;
use floem::views::{Container, Decorators, ScrollExt, Stack, dyn_stack};

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

