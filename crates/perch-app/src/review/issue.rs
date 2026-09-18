//! The issue view: title, labels and assignees, the body, the comments,
//! and what Perch has started from the issue. The thread column beside it
//! works as it does in a review, with no anchors.

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
use crate::ui::widgets::{avatar, hairline, sidebar_glyph, text_button};

/// The header row: state, number and title, then labels and assignees,
/// with Start work at the right.
pub fn issue_header(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let issue = move || state.issue.get();
    dyn_container(issue, move |issue| {
        let Some(issue) = issue else {
            return Empty::new().into_any();
        };
        let state_of = if issue.is_open() {
            perch_github::SubjectState::IssueOpen
        } else if issue.state_reason.as_deref() == Some("not_planned") {
            perch_github::SubjectState::IssueNotPlanned
        } else {
            perch_github::SubjectState::IssueClosed
        };
        let title = issue.title.clone();
        let number = issue.number;
        let labels = issue.labels.clone();
        let assignees = issue.assignees.clone();
        let backend = backend.clone();

        Stack::horizontal((
            floem::views::svg(move || icons::state_icon(state_of))
                .style(move |s| s.size(18.0, 18.0).color(icons::state_color(state_of)).flex_shrink(0.0_f32)),
            Stack::vertical((
                Stack::horizontal((
                    Label::derived(move || format!("#{number}")).style(|s| {
                        s.font_size(theme::TEXT_TITLE)
                            .color(theme::FG_SUBTLE)
                            .margin_right(8.0)
                            .flex_shrink(0.0_f32)
                    }),
                    Label::derived(move || title.clone()).style(|s| {
                        s.font_size(theme::TEXT_TITLE)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(theme::FG)
                            .text_ellipsis()
                            .min_width(0.0)
                    }),
                ))
                .style(|s| s.items_center().min_width(0.0)),
                Stack::horizontal((
                    dyn_stack(
                        move || labels.clone(),
                        |l| l.name.clone(),
                        |label| label_chip(label.name, label.color),
                    )
                    .style(|s| s.items_center().gap(4.0)),
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
                .style(|s| s.items_center().margin_top(3.0).min_width(0.0)),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32).margin_left(10.0)),
            start_work_button(state, backend),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0))
        .into_any()
    })
    .style(|s| s.flex_grow(1.0_f32).min_width(0.0))
}

fn label_chip(name: String, color: String) -> impl IntoView {
    let tint = label_color(&color);
    Label::derived(move || name.clone()).style(move |s| {
        s.font_size(theme::TEXT_SECTION)
            .color(tint)
            .border(1.0)
            .border_color(tint.with_alpha(0.5))
            .border_radius(9.0)
            .padding_horiz(6.0)
            .selectable(false)
    })
}

/// GitHub's label colour, darkened enough to read as text on white.
fn label_color(hex: &str) -> floem::peniko::Color {
    let parse = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    if hex.len() != 6 {
        return theme::FG_MUTED;
    }
    let (r, g, b) = (parse(0), parse(2), parse(4));
    let darken = |c: u8| (c as f32 * 0.65) as u8;
    floem::peniko::Color::from_rgb8(darken(r), darken(g), darken(b))
}

/// Cut a branch from the default branch and open it. When one exists
/// already, opens that.
fn start_work_button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.item.with(|i| i.as_ref().map(|i| i.id.clone())),
        move |id| {
            let Some(id) = id else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            text_button(|| "Start work".to_string(), SidebarIcon::Branch, move || backend.start_work(&id)).into_any()
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
    Stack::vertical((
        dyn_container(
            move || state.issue.get(),
            |issue| match issue {
                Some(issue) if !issue.body.trim().is_empty() => {
                    Container::new(markdown::render(&issue.body))
                        .style(|s| s.width_full().padding(20.0))
                        .into_any()
                }
                Some(_) => Label::derived(|| "No description.".to_string())
                    .style(|s| s.font_size(theme::TEXT_BODY).color(theme::FG_SUBTLE).padding(20.0))
                    .into_any(),
                None => Empty::new().into_any(),
            },
        )
        .style(|s| s.width_full()),
        hairline(),
        dyn_stack(
            move || state.issue_comments.get().as_ref().clone(),
            |c| c.id,
            move |comment| comment_view(app, state, comment),
        )
        .style(|s| s.flex_col().width_full()),
        in_perch(app, state, backend, open_item),
    ))
    .style(|s| s.flex_col().width_full().max_width(860.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0).background(theme::CANVAS))
}

fn comment_view(app: AppState, state: ReviewState, comment: IssueComment) -> impl IntoView {
    let login = comment.user.clone();
    let seed = comment.user.clone();
    let at = comment.created_at;
    let body = comment.body.clone();
    Stack::vertical((
        Stack::horizontal((
            avatar(app, || String::new(), 20.0, seed).style(|s| s.margin_right(8.0)),
            Label::derived(move || login.clone()).style(|s| {
                s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG)
            }),
            Label::derived(move || timefmt::relative(at, state.now_or_now(app))).style(|s| {
                s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE).margin_left(8.0)
            }),
        ))
        .style(|s| s.items_center().margin_bottom(8.0)),
        markdown::render(&body),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .padding_horiz(20.0)
            .padding_vert(14.0)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
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
            Stack::vertical((
                Stack::horizontal((
                    sidebar_glyph(SidebarIcon::Branch, theme::FG_SUBTLE).style(|s| s.size(12.0, 12.0).margin_right(6.0)),
                    Label::derived(|| "In Perch".to_string()).style(|s| {
                        s.font_size(theme::TEXT_SECTION)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(theme::FG_SUBTLE)
                            .selectable(false)
                    }),
                ))
                .style(|s| s.items_center().padding_horiz(20.0).padding_top(16.0).padding_bottom(4.0)),
                dyn_stack(
                    children,
                    |i| (i.id.clone(), i.subject.as_ref().map(|s| s.number()), i.checkout.as_ref().map(|c| c.head_sha.clone())),
                    move |item| {
                        let project = app.project(&item.project_id);
                        item_row(app, b.clone(), item, project, Placement::Section(crate::items::ItemGroup::Project), o.clone())
                    },
                )
                .style(|s| s.flex_col().width_full().padding_horiz(8.0).padding_bottom(16.0)),
            ))
            .style(|s| s.flex_col().width_full())
            .into_any()
        },
    )
    .style(|s| s.width_full())
}

impl ReviewState {
    /// The inbox's clock, so comment times read the same as its rows do.
    fn now_or_now(&self, app: AppState) -> jiff::Timestamp {
        app.now.get()
    }
}

/// So the status line can name the issue.
pub fn describe(issue: &Issue) -> String {
    format!("{}#{}  \u{00b7}  {} comments", issue.repo_full_name, issue.number, issue.comments)
}
