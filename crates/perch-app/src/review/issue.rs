//! The issue view: title, labels and assignees, the body, the comments,
//! and what Perch has started from the issue. The thread column beside it
//! works as it does in a review, with no anchors.
//!
//! Laid out the way the inbox is: cards on the muted canvas, one for the
//! description and one per comment, each led by who wrote it and when.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};
use perch_github::{Issue, IssueComment};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::markdown;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::items::{item_row, OpenItem, Placement};
use crate::ui::widgets::{avatar, sidebar_glyph, text_button};

/// How wide the reading column is. Prose past this is hard to follow.
const COLUMN_WIDTH: f64 = 820.0;

/// The header row: state, number and title, then labels, assignees and a
/// word on who opened it, with Start work at the right.
pub fn issue_header(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let issue = move || state.issue.get();
    dyn_container(issue, move |issue| {
        let Some(issue) = issue else {
            return Empty::new().into_any();
        };
        let state_of = state_of(&issue);
        let title = issue.title.clone();
        let number = issue.number;
        let labels = issue.labels.clone();
        let assignees = issue.assignees.clone();
        let opened = format!(
            "{} opened{}",
            issue.author_login,
            match issue.comments {
                0 => String::new(),
                1 => " \u{00b7} 1 comment".to_string(),
                n => format!(" \u{00b7} {n} comments"),
            }
        );
        let backend = backend.clone();

        Stack::horizontal((
            floem::views::svg(move || icons::state_icon(state_of))
                .style(move |s| s.size(18.0, 18.0).color(icons::state_color(state_of)).flex_shrink(0.0_f32)),
            Stack::vertical((
                Stack::horizontal((
                    Label::derived(move || title.clone()).style(|s| {
                        s.font_size(theme::TEXT_TITLE)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(theme::FG)
                            .text_ellipsis()
                            .min_width(0.0)
                    }),
                    Label::derived(move || format!("#{number}")).style(|s| {
                        s.font_size(theme::TEXT_TITLE)
                            .color(theme::FG_SUBTLE)
                            .margin_left(8.0)
                            .flex_shrink(0.0_f32)
                    }),
                ))
                .style(|s| s.items_center().min_width(0.0)),
                Stack::horizontal((
                    Label::derived(move || opened.clone()).style(|s| {
                        s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE).flex_shrink(0.0_f32)
                    }),
                    dyn_stack(
                        move || labels.clone(),
                        |l| l.name.clone(),
                        |label| label_chip(label.name, label.color),
                    )
                    .style(|s| s.items_center().gap(4.0).margin_left(10.0)),
                    dyn_stack(
                        move || assignees.clone(),
                        |a| a.clone(),
                        move |login| {
                            let url = String::new();
                            avatar(app, move || url.clone(), 18.0, login).style(|s| s.margin_left(4.0))
                        },
                    )
                    .style(|s| s.items_center().margin_left(6.0)),
                ))
                .style(|s| s.items_center().margin_top(2.0).min_width(0.0)),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32).margin_left(10.0)),
            start_work_button(app, state, backend),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0))
        .into_any()
    })
    .style(|s| s.flex_grow(1.0_f32).min_width(0.0))
}

fn state_of(issue: &Issue) -> perch_github::SubjectState {
    if issue.is_open() {
        perch_github::SubjectState::IssueOpen
    } else if issue.state_reason.as_deref() == Some("not_planned") {
        perch_github::SubjectState::IssueNotPlanned
    } else {
        perch_github::SubjectState::IssueClosed
    }
}

/// A label as GitHub draws it: its colour as a tint behind darker text.
fn label_chip(name: String, color: String) -> impl IntoView {
    let (tint, ink) = label_colors(&color);
    Container::new(Label::derived(move || name.clone()).style(move |s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::MEDIUM)
            .color(ink)
            .selectable(false)
            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
    }))
    .style(move |s| {
        s.height(18.0)
            .padding_horiz(7.0)
            .items_center()
            .justify_center()
            .background(tint)
            .border_radius(9.0)
            .flex_shrink(0.0_f32)
    })
}

/// GitHub's label colour: a light wash of it behind, and a darker cut of
/// it for the text, so every label reads on white.
fn label_colors(hex: &str) -> (floem::peniko::Color, floem::peniko::Color) {
    if hex.len() != 6 {
        return (theme::BADGE_BG, theme::FG_MUTED);
    }
    let parse = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32;
    let (r, g, b) = (parse(0), parse(2), parse(4));
    let wash = |c: f32| (255.0 - (255.0 - c) * 0.18) as u8;
    let ink = |c: f32| (c * 0.55) as u8;
    (
        floem::peniko::Color::from_rgb8(wash(r), wash(g), wash(b)),
        floem::peniko::Color::from_rgb8(ink(r), ink(g), ink(b)),
    )
}

/// Cut a branch from the default branch and open it. When one exists
/// already, opens that.
fn start_work_button(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.item.with(|i| i.as_ref().map(|i| i.id.clone())),
        move |id| {
            let Some(id) = id else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            text_button(|| "Start work".to_string(), SidebarIcon::Branch, move || {
                app.begin_open("Cutting a branch");
                backend.start_work(&id)
            })
            .into_any()
        },
    )
    .style(|s| s.flex_shrink(0.0_f32).margin_left(12.0))
}

/// The body, the comments, and what Perch started from the issue.
pub fn issue_body(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_item: OpenItem,
) -> impl IntoView {
    Container::new(
        Stack::vertical((
            dyn_container(
                move || state.issue.get(),
                move |issue| match issue {
                    Some(issue) => description_card(app, state, issue).into_any(),
                    None => Empty::new().into_any(),
                },
            )
            .style(|s| s.width_full()),
            dyn_stack(
                move || state.issue_comments.get().as_ref().clone(),
                |c| c.id,
                move |comment| comment_card(app, state, comment),
            )
            .style(|s| s.flex_col().width_full().gap(12.0)),
            in_perch(app, state, backend, open_item),
        ))
        .style(|s| s.flex_col().width_full().max_width(COLUMN_WIDTH).gap(12.0)),
    )
    .style(|s| s.width_full().items_start().justify_center().padding(20.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0).background(theme::CANVAS_SUBTLE))
}

/// The issue's own text, led by who opened it.
fn description_card(app: AppState, state: ReviewState, issue: std::sync::Arc<Issue>) -> impl IntoView {
    let login = issue.author_login.clone();
    let seed = issue.author_login.clone();
    let url = issue.author_avatar_url.clone();
    let when = issue.created_at;
    let body = issue.body.clone();
    card(
        byline(app, state, login, seed, url, when, "opened"),
        if body.trim().is_empty() {
            Label::derived(|| "No description.".to_string())
                .style(|s| s.font_size(theme::TEXT_BODY).color(theme::FG_SUBTLE))
                .into_any()
        } else {
            markdown::render(&body).into_any()
        },
    )
}

fn comment_card(app: AppState, state: ReviewState, comment: IssueComment) -> impl IntoView {
    let login = comment.user.clone();
    let seed = comment.user.clone();
    let url = comment.user_avatar_url.clone();
    let body = comment.body.clone();
    card(
        byline(app, state, login, seed, url, Some(comment.created_at), "commented"),
        markdown::render(&body).into_any(),
    )
}

/// Avatar, login, what they did and when.
fn byline(
    app: AppState,
    state: ReviewState,
    login: String,
    seed: String,
    url: String,
    when: Option<jiff::Timestamp>,
    verb: &'static str,
) -> impl IntoView {
    let _ = state;
    Stack::horizontal((
        avatar(app, move || url.clone(), 22.0, seed).style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        Label::derived(move || login.clone()).style(|s| {
            s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG)
        }),
        Label::derived(move || {
            match when {
                Some(at) => format!("{verb} {}", timefmt::relative(at, app.now.get())),
                None => verb.to_string(),
            }
        })
        .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE).margin_left(6.0)),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .height(40.0)
            .padding_horiz(16.0)
            .background(theme::CANVAS_SIDEBAR)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

/// One card: a byline strip on top, the text below.
fn card(header: impl IntoView + 'static, body: floem::AnyView) -> impl IntoView {
    Stack::vertical((
        header,
        Container::new(body).style(|s| s.width_full().padding(16.0)),
    ))
    .style(|s| s.flex_col().width_full())
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

/// The branches and pull requests Perch started from this issue.
fn in_perch(app: AppState, state: ReviewState, backend: Rc<Backend>, open_item: OpenItem) -> impl IntoView {
    let children = move || {
        let id = state.item.with(|i| i.as_ref().map(|i| i.id.clone()));
        app.items.with(|all| {
            all.iter()
                .filter(|i| i.from_item_id.is_some() && i.from_item_id == id)
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    dyn_container(
        move || children().is_empty(),
        move |empty| {
            if empty {
                return Empty::new().into_any();
            }
            let (b, o) = (backend.clone(), open_item.clone());
            let heading = Stack::horizontal((
                sidebar_glyph(SidebarIcon::Branch, theme::FG_MUTED).style(|s| s.size(13.0, 13.0).margin_right(7.0)),
                Label::derived(|| "In Perch".to_string()).style(|s| {
                    s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG).selectable(false)
                }),
            ))
            .style(|s| {
                s.items_center()
                    .width_full()
                    .height(40.0)
                    .padding_horiz(16.0)
                    .background(theme::CANVAS_SIDEBAR)
                    .border_bottom(1.0)
                    .border_color(theme::BORDER_MUTED)
            });
            let rows = dyn_stack(
                children,
                |i| format!("{i:?}"),
                move |item| {
                    let project = app.project(&item.project_id);
                    item_row(app, b.clone(), item, project, Placement::Section(crate::items::ItemGroup::Project), o.clone())
                },
            )
            .style(|s| s.flex_col().width_full().padding_vert(6.0));
            Stack::vertical((heading, rows))
                .style(|s| s.flex_col().width_full())
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
                .into_any()
        },
    )
    .style(|s| s.width_full())
}

/// So the status line can name the issue.
pub fn describe(issue: &Issue) -> String {
    format!("{}#{}  \u{00b7}  {} comments", issue.repo_full_name, issue.number, issue.comments)
}
