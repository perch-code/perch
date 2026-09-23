//! One-line footer: when the list was last current, the token source,
//! and any error.

use floem::peniko::Color;
use floem::prelude::*;
use floem::views::{dyn_container, Decorators, Label, Stack};

use crate::backend::{Backend, Status};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::spacer;

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
        spacer(),
        dyn_container(
            move || review.pull_request.with(|p| p.is_some()),
            move |open| {
                if open {
                    crate::review::review_status(review).into_any()
                } else {
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
                }
            },
        )
        .style(|s| s.min_width(0.0).margin_right(12.0)),
        crate::ui::agents::control(state, backend),
    ))
    .debug_name("status bar")
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
            format!(
                "Updated {} · {}",
                timefmt::relative(at, jiff::Timestamp::now()),
                source
            )
        }
        Status::RateLimited { message } => message,
        Status::Error { message } => message,
        Status::NoToken { .. } => "Not signed in".into(),
    }
}

/// A count with a comma every three digits, so 2140 reads as 2,140.
pub(crate) fn with_thousands(n: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comma_goes_every_three_digits_and_nowhere_else() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(999), "999");
        assert_eq!(with_thousands(1_000), "1,000");
        assert_eq!(with_thousands(12_345), "12,345");
        assert_eq!(with_thousands(1_234_567), "1,234,567");
    }
}
