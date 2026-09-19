//! One-line footer: refresh state, token source, any error, and where the
//! backfill is while it runs.

use floem::peniko::Color;
use floem::prelude::*;
use floem::style::CursorStyle;
use floem::views::{Container, Decorators, Label, Stack, dyn_container};

use crate::backend::{Backend, Status};
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::{AppState, Archiving, Screen};
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{sidebar_glyph, spacer};

pub fn status_bar(
    state: AppState,
    review: ReviewState,
    backend: std::rc::Rc<Backend>,
) -> impl IntoView {
    Stack::horizontal((
        dot(state),
        Label::derived(move || message(state)).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if state.notice.with(Option::is_some) {
                    theme::CLOSED
                } else {
                    color_for(&state.status.get())
                })
                .text_ellipsis()
                .min_width(0.0)
        }),
        archiving(state),
        spacer(),
        dyn_container(
            move || (review.pull_request.with(|p| p.is_some()), state.screen.get()),
            move |(open, screen)| {
                if open {
                    crate::review::review_status(review).into_any()
                } else if screen == Screen::Sidebar {
                    Label::derived(move || {
                        let (active, archived) = state.items.with(|all| {
                            (
                                all.iter().filter(|i| i.is_active()).count(),
                                all.iter().filter(|i| i.is_archived()).count(),
                            )
                        });
                        match (active, archived) {
                            (1, 0) => "1 item".to_string(),
                            (n, 0) => format!("{n} items"),
                            (n, a) => format!("{n} active, {a} archived"),
                        }
                    })
                    .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE))
                    .into_any()
                } else {
                    Label::derived(move || {
                        let n = state.visible().len();
                        let total = state.threads.with(|t| t.len());
                        if n == total {
                            format!("{total} notifications")
                        } else {
                            format!("{n} of {total} notifications")
                        }
                    })
                    .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_SUBTLE))
                    .into_any()
                }
            },
        )
        .style(|s| s.min_width(0.0).margin_right(12.0)),
        refresh_button(backend),
    ))
    .style(|s| {
        s.height(theme::STATUS_HEIGHT)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .gap(7.0)
            .background(theme::CANVAS_SIDEBAR)
            .border_top(1.0)
            .border_color(theme::BORDER)
    })
}

fn dot(state: AppState) -> impl IntoView {
    floem::views::Empty::new().style(move |s| {
        s.size(7.0, 7.0)
            .border_radius(4.0)
            .flex_shrink(0.0_f32)
            .background(if state.notice.with(Option::is_some) {
                theme::CLOSED
            } else {
                color_for(&state.status.get())
            })
    })
}

fn color_for(status: &Status) -> Color {
    match status {
        Status::Idle { .. } => theme::OPEN,
        Status::Refreshing => theme::ACCENT,
        Status::RateLimited { .. } => theme::DRAFT,
        Status::Error { .. } | Status::NoToken { .. } => theme::CLOSED,
    }
}

fn message(state: AppState) -> String {
    // A refusal, such as archiving a worktree with work in it, outranks the
    // poll state until the next thing goes right.
    if let Some(notice) = state.notice.get() {
        return notice;
    }
    match state.status.get() {
        Status::Refreshing => "Refreshing…".into(),
        Status::Idle { at, source } => {
            format!("Updated {} · {}", timefmt::relative(at, jiff::Timestamp::now()), source)
        }
        Status::RateLimited { message } => message,
        Status::Error { message } => message,
        Status::NoToken { .. } => "Not signed in".into(),
    }
}

/// Where the backfill is, after the poll status, in the subdued colour
/// the counts on the right use: it is progress, not the poll's health.
/// Gone, not blank, when nothing is being archived, so the bar reads as
/// it did before there was a backfill.
fn archiving(state: AppState) -> impl IntoView {
    Label::derived(move || state.archiving.with(|a| a.as_ref().map(archiving_line).unwrap_or_default())).style(
        move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .text_ellipsis()
                .min_width(0.0)
                .apply_if(state.archiving.with(Option::is_none), |s| s.hide())
        },
    )
}

/// "Archiving slatedb/slatedb, 312 of about 2,140". About, because the
/// total is the open count plus what the first closed page's `Link`
/// header implied, and a page can be short.
fn archiving_line(archiving: &Archiving) -> String {
    format!(
        "Archiving {}, {} of about {}",
        archiving.repo,
        with_thousands(archiving.done),
        with_thousands(archiving.total)
    )
}

/// A count with a comma every three digits, so 2140 reads as 2,140.
fn with_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn refresh_button(backend: std::rc::Rc<Backend>) -> impl IntoView {
    Container::new(sidebar_glyph(SidebarIcon::Refresh, theme::FG_MUTED))
        .on_event_stop(el::Click, move |_, _| backend.refresh())
        .style(|s| {
            s.size(22.0, 22.0)
                .items_center()
                .justify_center()
                .border_radius(4.0)
                .hover(|s| s.background(theme::CANVAS_HOVER).cursor(CursorStyle::Pointer))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backfill_line_names_the_repository_and_the_counts_with_thousands_separated() {
        let line = archiving_line(&Archiving { repo: "slatedb/slatedb".into(), done: 312, total: 2140 });
        assert_eq!(line, "Archiving slatedb/slatedb, 312 of about 2,140");
    }

    #[test]
    fn a_comma_goes_every_three_digits_and_nowhere_else() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(999), "999");
        assert_eq!(with_thousands(1_000), "1,000");
        assert_eq!(with_thousands(12_345), "12,345");
        assert_eq!(with_thousands(1_234_567), "1,234,567");
    }
}
