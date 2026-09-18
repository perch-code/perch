//! The issue's header: its state, number and title, a word on who opened
//! it, and Start work. The body is Conversation, drawn from
//! `review::cards`, and the labels, assignees and what Perch started from
//! the issue are the strip under the header.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::text::FontWeight;
use floem::views::{Decorators, Empty, Label, Stack, dyn_container};
use perch_github::Issue;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::header;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::text_button;

/// The header row: the state pill, number and title, a word on who opened
/// it, with Start work at the right.
pub fn issue_header(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let issue = move || state.issue.get();
    dyn_container(issue, move |issue| {
        let Some(issue) = issue else {
            return Empty::new().into_any();
        };
        let title = issue.title.clone();
        let number = issue.number;
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
            header::state_pill(state),
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
                Label::derived(move || opened.clone()).style(|s| {
                    s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE).margin_top(2.0).text_ellipsis().min_width(0.0)
                }),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32).margin_left(10.0)),
            start_work_button(app, state, backend),
        ))
        .style(|s| s.items_center().width_full().min_width(0.0))
        .into_any()
    })
    .style(|s| s.flex_grow(1.0_f32).min_width(0.0))
}

/// The state the sidebar's glyphs draw an issue in.
pub fn state_of(issue: &Issue) -> perch_github::SubjectState {
    if issue.is_open() {
        perch_github::SubjectState::IssueOpen
    } else if issue.state_reason.as_deref() == Some("not_planned") {
        perch_github::SubjectState::IssueNotPlanned
    } else {
        perch_github::SubjectState::IssueClosed
    }
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

/// So the status line can name the issue.
pub fn describe(issue: &Issue) -> String {
    format!("{}#{}  \u{00b7}  {} comments", issue.repo_full_name, issue.number, issue.comments)
}
