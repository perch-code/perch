//! The notification list: grouped cards of rows.

use floem::prelude::*;
use floem::text::FontWeight;
use floem::views::{
    ClipExt, Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack,
};

use crate::backend::Status;
use crate::filter::{Group, Section};
use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::ui::row::row;
use crate::ui::widgets::{avatar, centered_message, count_badge, hairline};

pub fn list(state: AppState) -> impl IntoView {
    dyn_container(
        move || ListMode::of(state),
        move |mode| match mode {
            ListMode::NoToken => token_help(state).into_any(),
            ListMode::Loading => centered_message(
                SidebarIcon::Refresh,
                || "Loading notifications".to_string(),
                || "Fetching your inbox from GitHub.".to_string(),
            )
            .into_any(),
            ListMode::Empty => centered_message(
                SidebarIcon::Inbox,
                || "You are all caught up".to_string(),
                || "No notifications in your inbox.".to_string(),
            )
            .into_any(),
            ListMode::NoMatches => centered_message(
                SidebarIcon::Search,
                || "No notifications match".to_string(),
                move || {
                    if state.filters().is_narrowed() {
                        "Try clearing a filter or the search box.".to_string()
                    } else {
                        "Nothing to show right now.".to_string()
                    }
                },
            )
            .into_any(),
            ListMode::Rows => rows(state).into_any(),
        },
    )
    .style(|s| s.size_full().min_height(0.0).background(theme::CANVAS_SUBTLE))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListMode {
    NoToken,
    Loading,
    Empty,
    NoMatches,
    Rows,
}

impl ListMode {
    fn of(state: AppState) -> Self {
        if matches!(state.status.get(), Status::NoToken { .. }) {
            return ListMode::NoToken;
        }
        if !state.loaded.get() {
            return ListMode::Loading;
        }
        if state.threads.with(|t| t.is_empty()) {
            return ListMode::Empty;
        }
        if state.visible().is_empty() {
            return ListMode::NoMatches;
        }
        ListMode::Rows
    }
}

fn rows(state: AppState) -> impl IntoView {
    dyn_stack(
        move || state.sections(),
        |section| section.key.clone(),
        move |section| section_card(state, section),
    )
    .style(|s| s.flex_col().width_full().gap(16.0).padding(16.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0))
}

/// One repository or date group, drawn as a bordered card.
fn section_card(state: AppState, section: Section) -> impl IntoView {
    let grouped = state.group.get() != Group::None;
    let threads = section.threads.clone();
    let count = threads.len();

    let header = if grouped {
        section_header(state, &section, count).into_any()
    } else {
        Empty::new().into_any()
    };

    Stack::vertical((
        header,
        dyn_stack(
            move || threads.clone().into_iter().enumerate(),
            |(i, t)| (*i, t.id.clone()),
            move |(i, thread)| {
                Stack::vertical((
                    if i == 0 {
                        Empty::new().into_any()
                    } else {
                        hairline().into_any()
                    },
                    row(state, thread),
                ))
                .style(|s| s.width_full())
            },
        )
        .style(|s| s.flex_col().width_full()),
    ))
    .clip()
    .style(|s| {
        s.width_full()
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS_LG)
            .box_shadow_blur(4.0)
            .box_shadow_v_offset(1.0)
            .box_shadow_color(theme::SHADOW)
    })
}

fn section_header(state: AppState, section: &Section, count: usize) -> impl IntoView {
    let title = section.title.clone();
    let seed = section.title.clone();
    let avatar_url = section.avatar_url.clone().unwrap_or_default();
    let has_avatar = section.avatar_url.is_some();

    Stack::horizontal((
        Container::new(if has_avatar {
            avatar(state, move || avatar_url.clone(), 18.0, seed).into_any()
        } else {
            Empty::new().into_any()
        })
        .style(move |s| {
            if has_avatar {
                s.margin_right(9.0).flex_shrink(0.0_f32)
            } else {
                s.flex_shrink(0.0_f32)
            }
        }),
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .text_ellipsis()
                .min_width(0.0)
                .margin_right(8.0)
        }),
        count_badge(move || count, false),
    ))
    .style(|s| {
        s.height(40.0)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// Shown when no token resolved, in place of the list.
fn token_help(state: AppState) -> impl IntoView {
    let detail = move || match state.status.get() {
        Status::NoToken { detail } => detail,
        _ => String::new(),
    };

    Container::new(
        Stack::vertical((
            Container::new(crate::ui::widgets::sidebar_glyph(
                SidebarIcon::Warning,
                theme::FG_SUBTLE,
            ))
            .style(|s| {
                s.size(48.0, 48.0)
                    .border_radius(24.0)
                    .background(theme::CANVAS_SUBTLE)
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "No GitHub token found").style(|s| {
                s.font_size(15.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .margin_bottom(6.0)
            }),
            Label::derived(detail).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_MUTED)
                    .max_width(460.0)
                    .margin_bottom(16.0)
            }),
            Stack::vertical((
                code_line("export GITHUB_TOKEN=ghp_..."),
                code_line("gh auth login"),
            ))
            .style(|s| s.gap(6.0)),
            Label::derived(|| "The token needs the notifications scope.").style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .margin_top(14.0)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(|s| s.size_full().items_center().justify_center().padding(40.0))
}

fn code_line(text: &'static str) -> impl IntoView {
    Label::derived(move || text).style(|s| {
        s.font_family("SF Mono, Menlo, monospace".to_string())
            .font_size(theme::TEXT_META)
            .color(theme::FG)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .padding_horiz(10.0)
            .padding_vert(5.0)
    })
}
