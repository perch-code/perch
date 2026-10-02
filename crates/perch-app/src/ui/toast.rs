//! The popup at the window's bottom right: an agent run that finished, a
//! line the reader should see, such as something that could not be done
//! or a poll that failed, or an offer to install a language server. One at
//! a time, each up for a few seconds but the offer, which waits. An error
//! or a warning has Report, which starts a bug report about it; a line
//! that names somewhere to go, such as a filed issue, goes there when
//! clicked.

use std::rc::Rc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::{AppState, ServerOffer, ToastKind, ToastOpen, Tone};
use crate::theme;
use crate::ui::{agents, widgets};

pub fn toast(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || {
            // A run's toast waits for the run's summary.
            state.current_toast().filter(|toast| match &toast.kind {
                ToastKind::AgentRun(id) => state.agent_run(id).is_some(),
                ToastKind::Message { .. } | ToastKind::Server(_) => true,
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
                ToastKind::Message { text, tone, open } => {
                    let reportable = tone != Tone::Success && open.is_none();
                    let go = open.map(|open| destination(state, review, backend.clone(), token, open));
                    (message(state, review, token, text, tone, reportable).into_any(), go)
                }
                ToastKind::Server(offer) => (
                    server_offer(state, backend.clone(), token, offer).into_any(),
                    None,
                ),
            };
            let clickable = open.is_some();
            let mut card = Container::new(body);
            if let Some(open) = open {
                card = card.on_event_stop(el::Click, move |_, _| open());
            }
            card.style(move |s| {
                let s = s
                    .width(328.0)
                    .max_width_full()
                    .padding_left(13.0)
                    .padding_right(7.0)
                    .padding_vert(9.0)
                    .background(theme::canvas())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(8.0)
                    .box_shadow_blur(18.0)
                    .box_shadow_v_offset(5.0)
                    .box_shadow_color(theme::shadow());
                if clickable {
                    widgets::clickable(s).hover(|s| s.background(theme::canvas_subtle()))
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

/// What clicking a toast that names somewhere does: it goes, and the
/// toast comes down.
fn destination(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    token: u64,
    open: ToastOpen,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        state.dismiss_toast(token);
        match &open {
            ToastOpen::Subject {
                repo_full_name,
                kind,
                number,
            } => {
                let navigation_id = backend.open_subject(repo_full_name, *kind, *number);
                state.begin_open(
                    match kind {
                        perch_github::ItemKind::PullRequest => "Reading the pull request",
                        perch_github::ItemKind::Issue => "Reading the issue",
                    },
                    navigation_id,
                );
            }
            ToastOpen::FeedbackPreview => crate::ui::feedback::open(
                state,
                review,
                crate::feedback::FeedbackKind::Bug,
                None,
            ),
        }
    })
}

fn message(
    state: AppState,
    review: ReviewState,
    token: u64,
    text: String,
    tone: Tone,
    reportable: bool,
) -> impl IntoView {
    let color: fn() -> Color = match tone {
        Tone::Error => theme::closed,
        Tone::Warning => theme::draft,
        Tone::Success => theme::open,
    };
    let dismiss = agents::icon_action(SidebarIcon::Close, "Dismiss message", move || {
        state.dismiss_toast(token)
    });
    let reported = text.clone();
    let report = Label::derived(|| "Report".to_string())
        .on_event_stop(el::Click, move |_, _| {
            state.dismiss_toast(token);
            crate::ui::feedback::report(state, review, reported.clone(), tone);
        })
        .style(move |s| {
            widgets::clickable(s)
                .font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .padding_horiz(7.0)
                .height(22.0)
                .items_center()
                .flex_shrink(0.0_f32)
                .border_radius(5.0)
                .hover(|s| s.color(theme::fg()).background(theme::badge_bg()))
                .apply_if(!reportable, |s| s.hide())
        })
        .debug_name("Report problem");
    Stack::horizontal((
        dot(color),
        Label::derived(move || text.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::fg())
                .text_wrap()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
        report,
        dismiss,
    ))
    .style(|s| s.items_center().width_full().gap(9.0))
}

/// An offer to install a language server: its line, the button that runs
/// the install, with the command as its tip, and the close that turns it
/// down for the session. While the install runs, the line says so and the
/// buttons go.
fn server_offer(state: AppState, backend: Rc<Backend>, token: u64, offer: ServerOffer) -> impl IntoView {
    let kind = offer.kind;
    let installing = move || state.installing_servers.with(|i| i.contains(&kind));
    let text = offer.text.clone();
    let line = Label::derived(move || {
        if installing() {
            format!("Installing {}…", kind.name())
        } else {
            text.clone()
        }
    })
    .style(|s| {
        s.font_size(theme::TEXT_BODY)
            .color(theme::fg())
            .text_wrap()
            .min_width(0.0)
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
    });
    let button = match offer.install.clone() {
        Some(install) => {
            let shown = install.shown();
            let run = move || {
                state.installing_servers.update(|i| _ = i.insert(kind));
                backend.install_language_server(kind, install.clone());
            };
            let button = offer_button(offer.action, run)
                .style(move |s| s.apply_if(installing(), |s| s.hide()));
            crate::review::cards::tip(button, shown).into_any()
        }
        None => Empty::new().into_any(),
    };
    let key = offer.key.clone();
    let close = agents::icon_action(SidebarIcon::Close, "Dismiss message", move || {
        state.decline_offer(token, &key)
    })
    .style(move |s| s.apply_if(installing(), |s| s.hide()));
    Stack::horizontal((dot(theme::draft), line, button, close))
        .style(|s| s.items_center().width_full().gap(9.0))
}

/// A toast's small bordered button.
fn offer_button(label: &'static str, on_click: impl Fn() + 'static) -> impl View {
    Container::new(Label::new(label).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg())
            .selectable(false)
    }))
    .on_event_stop(el::Click, move |_, _| on_click())
    .style(|s| {
        widgets::clickable(s)
            .height(26.0)
            .items_center()
            .padding_horiz(10.0)
            .flex_shrink(0.0_f32)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::canvas_hover()))
    })
    .debug_name(label)
}

/// The coloured dot at a toast's left.
pub(crate) fn dot(color: impl theme::Ink) -> impl IntoView {
    Empty::new().style(move |s| {
        s.size(9.0, 9.0)
            .border_radius(5.0)
            .background(color())
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

    /// An offer shows its line and Install. Pressing it says the install is
    /// under way and takes the buttons away until it ends.
    #[test]
    fn an_offer_installs_from_its_button() {
        use floem::prelude::*;
        let (mut harness, app, _) = pull_request_harness();
        let missing = perch_lsp::Missing {
            kind: perch_lsp::ServerKind::RustAnalyzer,
            needs: None,
            install: Some(Box::new(perch_lsp::Install::Command {
                program: "rustup".into(),
                args: vec!["component".into(), "add".into(), "rust-analyzer".into()],
                env: Vec::new(),
                into: None,
            })),
            toolchain: Some("1.98.1-aarch64-apple-darwin".into()),
        };
        app.offer_server(crate::state::ServerOffer::missing(&missing, None), false);
        harness.rebuild();
        harness.paint();
        let toast = named(harness.root_id(), "toast").expect("no toast");
        assert!(named(toast, "rust-analyzer isn't installed for Rust 1.98.1.").is_some());
        let install = named(toast, "Install").expect("no Install");
        let at = install.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert!(app
            .installing_servers
            .with_untracked(|i| i.contains(&perch_lsp::ServerKind::RustAnalyzer)));
        let toast = named(harness.root_id(), "toast").expect("the toast stays");
        assert!(named(toast, "Installing rust-analyzer…").is_some());
    }
}
