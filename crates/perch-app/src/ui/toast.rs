//! The popup at the window's bottom right: an agent run that finished, or
//! a line the reader should see, such as something that could not be done
//! or a poll that failed. One at a time, each up for a few seconds.

use std::rc::Rc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::{AppState, ToastKind, Tone};
use crate::theme;
use crate::ui::agents;

pub fn toast(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || {
            // A run's toast waits for the run's summary.
            state.current_toast().filter(|toast| match &toast.kind {
                ToastKind::AgentRun(id) => state.agent_run(id).is_some(),
                ToastKind::Message { .. } => true,
            })
        },
        move |toast| {
            let Some(toast) = toast else {
                return Empty::new().into_any();
            };
            let token = toast.token;
            let body = match toast.kind {
                ToastKind::AgentRun(id) => match state.agent_run(&id) {
                    Some(run) => {
                        agents::completion_toast(state, review, backend.clone(), token, run)
                            .into_any()
                    }
                    None => return Empty::new().into_any(),
                },
                ToastKind::Message { text, tone } => message(state, token, text, tone).into_any(),
            };
            Container::new(body)
                .style(|s| {
                    s.width(328.0)
                        .max_width_full()
                        .padding_left(13.0)
                        .padding_right(7.0)
                        .padding_vert(9.0)
                        .background(theme::CANVAS)
                        .border(1.0)
                        .border_color(theme::BORDER)
                        .border_radius(8.0)
                        .box_shadow_blur(18.0)
                        .box_shadow_v_offset(5.0)
                        .box_shadow_color(theme::SHADOW)
                })
                .into_any()
        },
    )
    .style(|s| {
        s.position(Position::Absolute)
            .inset_right(12.0)
            .inset_bottom(12.0)
            .z_index(80)
    })
    .debug_name("toast")
}

fn message(state: AppState, token: u64, text: String, tone: Tone) -> impl IntoView {
    let color = match tone {
        Tone::Error => theme::CLOSED,
        Tone::Warning => theme::DRAFT,
    };
    let dismiss = agents::icon_action(SidebarIcon::Close, "Dismiss message", move || {
        state.dismiss_toast(token)
    });
    Stack::horizontal((
        dot(color),
        Label::derived(move || text.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::FG)
                .text_wrap()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        dismiss,
    ))
    .style(|s| s.items_center().width_full().gap(9.0))
}

/// The coloured dot at a toast's left.
pub(crate) fn dot(color: Color) -> impl IntoView {
    Empty::new().style(move |s| {
        s.size(9.0, 9.0)
            .border_radius(5.0)
            .background(color)
            .flex_shrink(0.0_f32)
    })
}

#[cfg(test)]
mod tests {
    use crate::review::header::tests::{named, pull_request_harness};

    /// A long message wraps inside the card, sitting in the window's
    /// bottom right corner, and the close button takes it down.
    #[test]
    fn a_long_message_wraps_in_the_corner_and_closes() {
        let (mut harness, app, _) = pull_request_harness();
        app.complain(
            "Could not archive the worktree: it has uncommitted changes in \
             three files, and archiving it would lose them",
        );
        harness.rebuild();
        harness.paint();
        let toast = named(harness.root_id(), "toast").expect("no toast");
        let card = toast.get_visual_rect_no_clip();
        assert!((card.x1 - (1400.0 - 12.0)).abs() < 1.0, "{card:?}");
        assert!((card.y1 - (900.0 - 12.0)).abs() < 1.0, "{card:?}");
        let text = named(toast, "Could not archive").expect("no text");
        let text = text.get_visual_rect_no_clip();
        assert!(text.height() > 30.0, "more than one line: {text:?}");
        assert!(text.x1 <= card.x1, "inside the card: {text:?} {card:?}");

        let close = named(toast, "Dismiss message").expect("no close button");
        let at = close.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert!(app.current_toast().is_none());
        assert!(named(harness.root_id(), "Could not archive").is_none());
    }
}
