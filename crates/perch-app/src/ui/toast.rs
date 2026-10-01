//! The popup at the window's bottom right: an agent run that finished, or
//! a line the reader should see, such as something that could not be done
//! or a poll that failed. One at a time, each up for a few seconds before
//! it fades out.
//!
//! vger draws no view at less than full opacity, so a toast fades by
//! taking the alpha out of each of its colours, all through [`faded`].

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
use crate::ui::{agents, widgets};

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
            // What clicking anywhere on the card does, for a toast that
            // goes somewhere. Its own close button stops the click.
            let (body, open) = match toast.kind {
                ToastKind::AgentRun(id) => match state.agent_run(&id) {
                    Some(run) => {
                        let (body, open) =
                            agents::completion_toast(state, review, backend.clone(), token, run);
                        (body.into_any(), Some(open))
                    }
                    None => return Empty::new().into_any(),
                },
                ToastKind::Message { text, tone } => {
                    (message(state, token, text, tone).into_any(), None)
                }
            };
            let clickable = open.is_some();
            let mut card = Container::new(body);
            if let Some(open) = open {
                card = card.on_event_stop(el::Click, move |_, _| open());
            }
            card.style(move |s| {
                let fade = move |color| faded(state, token, color);
                let s = s
                    .width(328.0)
                    .max_width_full()
                    .padding_left(13.0)
                    .padding_right(7.0)
                    .padding_vert(9.0)
                    .background(fade(theme::CANVAS))
                    .border(1.0)
                    .border_color(fade(theme::BORDER))
                    .border_radius(8.0)
                    .box_shadow_blur(18.0)
                    .box_shadow_v_offset(5.0)
                    .box_shadow_color(fade(theme::SHADOW));
                if clickable {
                    widgets::clickable(s).hover(move |s| s.background(fade(theme::CANVAS_SUBTLE)))
                } else {
                    s
                }
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
        Tone::Success => theme::OPEN,
    };
    let fade = move |color| faded(state, token, color);
    let dismiss =
        agents::icon_action_faded(SidebarIcon::Close, "Dismiss message", fade, move || {
            state.dismiss_toast(token)
        });
    Stack::horizontal((
        dot(color, fade),
        Label::derived(move || text.clone()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .color(fade(theme::FG))
                .text_wrap()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        dismiss,
    ))
    .style(|s| s.items_center().width_full().gap(9.0))
}

/// `color` as the toast `token` draws it, which is fainter as it fades.
pub(crate) fn faded(state: AppState, token: u64, color: Color) -> Color {
    color.multiply_alpha(state.toast_opacity(token))
}

/// The coloured dot at a toast's left.
pub(crate) fn dot(color: Color, fade: impl Fn(Color) -> Color + 'static) -> impl IntoView {
    Empty::new().style(move |s| {
        s.size(9.0, 9.0)
            .border_radius(5.0)
            .background(fade(color))
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
