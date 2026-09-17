//! One notification row.

use floem::prelude::*;
use floem::views::{Container, Decorators, Empty, Label, Stack, dyn_container};
use floem::text::{Alignment, FontWeight};
use perch_github::{SubjectKind, Thread};

use crate::filter::Group;
use crate::ui::list::OpenThread;
use crate::icons;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{avatar, icon, reason_pill};

pub fn row(state: AppState, thread: Thread, on_open: OpenThread) -> impl IntoView {
    let unread = thread.unread;

    // A pull request opens the review view. Everything else has no view of
    // its own yet, so it opens where it lives.
    let review_target = if thread.subject.kind == SubjectKind::PullRequest {
        thread
            .number_hint()
            .map(|n| (thread.repository.full_name.clone(), n))
    } else {
        None
    };
    let web_url = thread.html_url();

    Stack::horizontal((
        unread_dot(unread),
        state_icon(state, thread.clone()),
        text_column(state, thread.clone()),
        reason_pill({
            let reason = thread.reason.clone();
            move || reason.label().to_string()
        })
        .style(|s| s.flex_shrink(0.0_f32).margin_right(10.0)),
        author_avatar(state, thread.clone()),
        timestamp(state, thread),
    ))
    .on_event_stop(el::Click, move |_, _| match &review_target {
        Some((repo, number)) => on_open(repo, *number),
        None => crate::ui::widgets::open_in_browser(&web_url),
    })
    .style(move |s| {
        // The whole row is the button.
        let base = crate::ui::widgets::clickable(s)
            .height(theme::ROW_HEIGHT)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .gap(0.0);
        if unread {
            base.background(theme::CANVAS_UNREAD)
                .hover(|s| s.background(theme::CANVAS_HOVER))
        } else {
            base.hover(|s| s.background(theme::CANVAS_SUBTLE))
        }
    })
}

/// The blue dot. Its space is reserved when read, so titles stay aligned.
fn unread_dot(unread: bool) -> impl IntoView {
    Container::new(if unread {
        Empty::new()
            .style(|s| {
                s.size(8.0, 8.0)
                    .border_radius(4.0)
                    .background(theme::ACCENT)
            })
            .into_any()
    } else {
        Empty::new().into_any()
    })
    .style(|s| s.width(16.0).items_center().justify_center().flex_shrink(0.0_f32))
}

fn state_icon(state: AppState, thread: Thread) -> impl IntoView {
    let id = thread.id.clone();
    let kind = thread.subject.kind.clone();
    icon(move || match state.detail_for(&id) {
        Some(d) => icons::state_icon(d.state),
        None => icons::kind_icon(&kind),
    })
    .style(|s| s.margin_right(10.0).flex_shrink(0.0_f32))
}

fn text_column(state: AppState, thread: Thread) -> impl IntoView {
    let id = thread.id.clone();
    let repo = thread.repository.full_name.clone();
    let hint = thread.number_hint();
    let title = thread.subject.title.clone();
    let unread = thread.unread;

    // Inside a repository group the repo name is already in the header, so
    // the meta line carries only the number and would otherwise repeat it.
    let meta = move || {
        let number = state
            .detail_for(&id)
            .map(|d| d.number)
            .or(hint)
            .map(|n| format!("#{n}"));
        let show_repo = state.group.get() != Group::Repository;
        match (show_repo, number) {
            (true, Some(n)) => format!("{repo}  {n}"),
            (true, None) => repo.clone(),
            (false, Some(n)) => n,
            (false, None) => String::new(),
        }
    };

    Stack::vertical((
        Label::derived(meta).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .text_ellipsis()
                .max_width_full()
                .margin_bottom(2.0)
        }),
        Label::derived(move || title.clone()).style(move |s| {
            s.font_size(theme::TEXT_TITLE)
                .color(theme::FG)
                .font_weight(if unread {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_ellipsis()
                .max_width_full()
        }),
    ))
    .style(|s| {
        s.flex_col()
            .flex_grow(1.0_f32)
            // Without a zero minimum, a long title pushes the trailing
            // columns off the right edge instead of ellipsizing.
            .min_width(0.0)
            .justify_center()
            .margin_right(12.0)
    })
}

fn author_avatar(state: AppState, thread: Thread) -> impl IntoView {
    let id = thread.id.clone();
    let seed_id = thread.id.clone();
    let fallback = thread.repository.full_name.clone();

    let seed = move || {
        state
            .detail_for(&seed_id)
            .map(|d| d.author_login)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| fallback.clone())
    };

    dyn_container(seed, move |seed| {
        let id = id.clone();
        avatar(
            state,
            move || {
                state
                    .detail_for(&id)
                    .map(|d| d.author_avatar_url)
                    .unwrap_or_default()
            },
            20.0,
            seed,
        )
        .into_any()
    })
    .style(|s| s.size(20.0, 20.0).flex_shrink(0.0_f32).margin_right(12.0))
}

fn timestamp(state: AppState, thread: Thread) -> impl IntoView {
    let when = thread.updated_at;
    Label::derived(move || timefmt::relative(when, state.now.get())).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
            .width(96.0)
            .flex_shrink(0.0_f32)
            // justify_content lays out flex children; a label places its
            // own text with text_align.
            .text_align(Alignment::End)
            .text_ellipsis()
    })
}
