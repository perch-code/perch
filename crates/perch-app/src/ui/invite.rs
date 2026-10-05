//! The invite button in the drawer's footer, and the popover over it: a
//! GitHub username and a note, sent as an issue on Perch's repository
//! asking for them to be added with read access. The owner adds them.
//!
//! What is typed is the window's, and outlives the popover: a press
//! outside or Escape shuts it and the button brings it back as it was.
//! Only sending ends it.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack, TextInput};

use crate::backend::{Backend, Event};
use crate::feedback::REPO;
use crate::icons::{self, SidebarIcon};
use crate::invite::{self, InviteDraft};
use crate::review::cards::tip;
use crate::review::model::ReviewState;
use crate::state::{AppState, ToastOpen, Tone};
use crate::theme;
use crate::ui::widgets::{action_button, icon_in, sidebar_glyph};

/// The invite button in the drawer's footer, and the popover over it
/// while it is open.
pub fn button(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let color = move || theme::nav_muted();
    let button = Container::new(
        icon_in(
            move || icons::sidebar_icon(SidebarIcon::Invite, color()),
            color,
        )
        .style(|s| s.pointer_events_none()),
    );
    let button_id = button.view_id();
    let button = button
        .on_event_stop(el::Click, move |_, _| toggle(app, review, Some(button_id)))
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(app, review, Some(button_id));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| crate::ui::sidebar::footer_button(s, review.invite_open.get()))
        .debug_name("invite-button");

    let stack = Stack::new((
        tip(button, "Invite someone".to_string()),
        dyn_container(
            move || review.invite_open.get(),
            move |open| {
                if open {
                    popover(app, review, backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(crate::ui::sidebar::footer_popover),
    ));
    // A press outside the button and what hangs from it shuts it: see
    // `ui::dismiss_popovers`.
    let anchor = stack.view_id();
    Effect::new(move |_| {
        if review.invite_open.get() {
            review.invite_anchor.set(Some(anchor));
        }
    });
    stack.style(|s| {
        s.position(Position::Relative)
            .flex_shrink(0.0_f32)
            .pointer_events_auto()
    })
}

/// Open the popover, or shut it when it is open.
pub fn toggle(app: AppState, review: ReviewState, opener: Option<floem::ViewId>) {
    if review.invite_open.get_untracked() {
        close(review, true);
    } else {
        open(app, review, opener);
    }
}

/// Open the popover over whatever else hangs from the bar. It hangs from
/// the drawer's footer, so a shut drawer opens with it.
pub fn open(app: AppState, review: ReviewState, opener: Option<floem::ViewId>) {
    app.sidebar_open.set(true);
    crate::review::review_menu::close(review, false);
    crate::ui::agents::dropdown::close(review, false);
    crate::ui::usage::close(review, false);
    crate::ui::feedback::close(review, false);
    review.invite_opener.set(opener);
    review.invite_open.set(true);
}

/// Shut the popover, keeping what was typed, and give the keyboard back
/// to what had it when it opened when `restore_focus`.
pub fn close(review: ReviewState, restore_focus: bool) {
    if !review.invite_open.get_untracked() {
        return;
    }
    review.invite_open.set(false);
    let opener = review.invite_opener.get_untracked();
    review.invite_opener.set(None);
    if restore_focus {
        if let Some(opener) = opener {
            opener.request_focus();
        }
    }
}

/// The backend's answer, in the window that asked.
pub fn apply(app: AppState, review: ReviewState, event: Event) {
    match event {
        Event::InviteSent {
            number,
            login,
            existing,
        } => {
            review.invite.set(InviteDraft::default());
            close(review, false);
            let text = if existing {
                format!("{login} has been asked for already, in #{number}")
            } else {
                format!("Asked for {login} to be added, in #{number}")
            };
            app.notify_with(
                text,
                Tone::Success,
                Some(ToastOpen::Subject {
                    repo_full_name: REPO.to_string(),
                    kind: perch_github::ItemKind::Issue,
                    number,
                }),
            );
        }
        Event::InviteFailed(error) => review.invite.update(|draft| {
            draft.sending = false;
            draft.error = Some(error.message);
            draft.forbidden = error.forbidden;
        }),
        _ => {}
    }
}

/// Send: check what was typed here, and hand the rest to the backend.
fn send(app: AppState, review: ReviewState, backend: &Backend) {
    let draft = review.invite.get_untracked();
    if draft.sending {
        return;
    }
    let Some(login) = invite::parse_login(&draft.login) else {
        return;
    };
    let me = app
        .viewer
        .with_untracked(|v| v.as_ref().map(|(login, _)| login.clone()));
    if me.is_some_and(|me| me.eq_ignore_ascii_case(&login)) {
        review
            .invite
            .update(|d| d.error = Some("That's you.".into()));
        return;
    }
    review.invite.update(|d| {
        d.sending = true;
        d.error = None;
        d.forbidden = false;
    });
    backend.invite(&login, &draft.note);
}

/// The token can't open issues on Perch's repository: GitHub's own form,
/// filled in.
fn open_on_github(review: ReviewState) {
    let draft = review.invite.get_untracked();
    let Some(login) = invite::parse_login(&draft.login) else {
        return;
    };
    let title = percent_encode(&invite::title(&login));
    let body = percent_encode(&invite::body(&login, &draft.note));
    crate::ui::widgets::open_in_browser(&format!(
        "https://github.com/{REPO}/issues/new?title={title}&body={body}"
    ));
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A field that writes back into the draft as it's typed.
fn field(
    review: ReviewState,
    placeholder: &'static str,
    debug: &'static str,
    read: fn(&InviteDraft) -> String,
    write: fn(&mut InviteDraft, String),
) -> TextInput {
    let text = RwSignal::new(review.invite.with_untracked(read));
    Effect::new(move |_| {
        let typed = text.get();
        if review.invite.with_untracked(|d| read(d) != typed) {
            review.invite.update(|d| {
                write(d, typed);
                d.error = None;
                d.forbidden = false;
            });
        }
    });
    TextInput::new(text)
        .placeholder(placeholder)
        .debug_name(debug)
        .style(move |s| {
            s.width_full()
                .height(32.0)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(10.0)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::accent()))
                .set_disabled(review.invite.with(|d| d.sending))
        })
}

fn popover(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let can_send = move || {
        review
            .invite
            .with(|d| !d.sending && invite::parse_login(&d.login).is_some())
    };
    let header = Stack::vertical((
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Invite, theme::fg)
                .style(|s| s.size(15.0, 15.0).flex_shrink(0.0_f32)),
            Label::derived(|| "Invite someone".to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_strong())
                    .color(theme::fg())
                    .selectable(false)
            }),
        ))
        .style(|s| s.items_center().gap(8.0)),
        Label::derived(|| {
            format!(
                "Opens an issue on {REPO} asking for them to be added with read access. \
                 They'll get an invitation from GitHub once they are."
            )
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .text_wrap()
                .width_full()
                .selectable(false)
        }),
    ))
    .style(|s| s.flex_col().gap(4.0).width_full());

    let login = field(
        review,
        "GitHub username",
        "invite-login",
        |d| d.login.clone(),
        |d, text| d.login = text,
    );
    let login_id = login.view_id();
    let note = field(
        review,
        "How do you know them? (optional)",
        "invite-note",
        |d| d.note.clone(),
        |d, text| d.note = text,
    );

    let error = Label::derived(move || review.invite.with(|d| d.error.clone().unwrap_or_default()))
        .debug_name("invite-error")
        .style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::closed())
                .text_wrap()
                .width_full()
                .apply_if(review.invite.with(|d| d.error.is_none()), |s| s.hide())
        });

    let send_backend = backend.clone();
    let footer = Stack::horizontal((
        action_button(
            || "Open on GitHub".to_string(),
            "invite-github",
            false,
            || true,
            move || open_on_github(review),
        )
        .style(move |s| s.apply_if(!review.invite.with(|d| d.forbidden), |s| s.hide())),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            move || {
                if review.invite.with(|d| d.sending) {
                    "Sending…".to_string()
                } else {
                    "Send request".to_string()
                }
            },
            "invite-send",
            true,
            can_send,
            move || send(app, review, &send_backend),
        ),
    ))
    .style(|s| {
        s.items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(16.0)
            .padding_vert(12.0)
            .border_top(1.0)
            .border_color(theme::border_muted())
    });

    // The keyboard goes to the username as the popover opens.
    let asked = Rc::new(std::cell::Cell::new(false));
    Stack::vertical((
        Stack::vertical((header, login, note, error)).style(|s| {
            s.flex_col()
                .width_full()
                .gap(12.0)
                .padding(16.0)
                .padding_bottom(14.0)
        }),
        footer,
    ))
    .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            login_id.request_focus();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Enter && can_send() {
            send(app, review, &backend);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        s.width(theme::FOOTER_POPOVER_WIDTH)
            .flex_col()
            .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
    })
    .debug_name("invite-popover")
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    use floem::headless::{HeadlessHarness, TestRoot};

    use crate::backend::Command;
    use crate::feedback::SendError;
    use crate::review::header::tests::{named, rect};

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
    }

    /// The window with nothing open, signed in as `me`, its commands
    /// recorded.
    fn empty_window() -> (
        HeadlessHarness,
        AppState,
        ReviewState,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let app = AppState::new();
        let review = ReviewState::new();
        app.review.set(Some(review));
        app.viewer.set(Some(("me".into(), String::new())));
        let (backend, commands) = Backend::recording();
        let root = TestRoot::new();
        let view = crate::ui::app_view(app, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        settle(&mut harness);
        (harness, app, review, commands)
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, name).center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    fn type_text(harness: &mut HeadlessHarness, text: &str) {
        for c in text.chars() {
            let code = match c {
                '-' => Code::Minus,
                c => Code::from_str(&format!("Key{}", c.to_ascii_uppercase())).unwrap(),
            };
            harness.dispatch_event(floem::event::Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Character(c.to_string()),
                code,
                location: Default::default(),
                modifiers: Modifiers::empty(),
                repeat: false,
                is_composing: false,
            }));
        }
        settle(harness);
    }

    fn invites(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Vec<(String, String)> {
        std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|command| match command {
                Command::Invite { login, note } => Some((login, note)),
                _ => None,
            })
            .collect()
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| {
            let r = id.get_visual_rect_no_clip();
            r.width() > 0.0 && r.height() > 0.0
        })
    }

    #[test]
    fn the_button_leads_the_footer_and_raises_the_popover_over_itself() {
        let (mut harness, _app, review, _commands) = empty_window();
        let footer = rect(&harness, "drawer footer");
        let button = rect(&harness, "invite-button");
        let bug = rect(&harness, "feedback-button");
        assert!(footer.contains(button.center()));
        assert!(
            button.x1 <= bug.x0 && bug.x0 - button.x1 < 16.0,
            "{button:?} {bug:?}"
        );

        click(&mut harness, "invite-button");
        assert!(review.invite_open.get());
        let popover = rect(&harness, "invite-popover");
        assert!(popover.y1 <= button.y0, "{popover:?} {button:?}");
        assert!((popover.x0 - button.x0).abs() < 1.0, "left edges aligned");
        assert!((popover.width() - theme::FOOTER_POPOVER_WIDTH).abs() < 1.0);
        assert!(!shown(&harness, "invite-error"));
        assert!(!shown(&harness, "invite-github"));

        click(&mut harness, "invite-button");
        assert!(!review.invite_open.get(), "a second click did not shut it");
    }

    #[test]
    fn what_is_typed_goes_to_the_backend_once() {
        let (mut harness, _app, review, mut commands) = empty_window();
        click(&mut harness, "invite-button");
        click(&mut harness, "invite-send");
        assert!(invites(&mut commands).is_empty(), "sent with no username");

        click(&mut harness, "invite-login");
        type_text(&mut harness, "octo-cat");
        assert_eq!(review.invite.with(|d| d.login.clone()), "octo-cat");
        click(&mut harness, "invite-note");
        type_text(&mut harness, "slatedb");
        click(&mut harness, "invite-send");
        assert_eq!(
            invites(&mut commands),
            [("octo-cat".to_string(), "slatedb".to_string())]
        );
        assert!(review.invite.with(|d| d.sending));
        click(&mut harness, "invite-send");
        assert!(invites(&mut commands).is_empty(), "sent twice");
    }

    #[test]
    fn a_profile_link_is_sent_as_its_login_and_yourself_is_refused() {
        let (mut harness, _app, review, mut commands) = empty_window();
        review
            .invite
            .update(|d| d.login = "https://github.com/Me".into());
        click(&mut harness, "invite-button");
        click(&mut harness, "invite-send");
        assert!(invites(&mut commands).is_empty());
        assert!(shown(&harness, "invite-error"));
        assert_eq!(
            review.invite.with(|d| d.error.clone()).as_deref(),
            Some("That's you.")
        );

        review
            .invite
            .update(|d| d.login = "github.com/octo-cat/".into());
        click(&mut harness, "invite-send");
        assert_eq!(
            invites(&mut commands),
            [("octo-cat".to_string(), String::new())]
        );
    }

    #[test]
    fn a_sent_invite_clears_the_draft_and_toasts_the_issue() {
        let (mut harness, app, review, _commands) = empty_window();
        review.invite.update(|d| {
            d.login = "octo-cat".into();
            d.sending = true;
        });
        click(&mut harness, "invite-button");
        apply(
            app,
            review,
            Event::InviteSent {
                number: 77,
                login: "Octo-Cat".into(),
                existing: false,
            },
        );
        settle(&mut harness);
        assert!(!review.invite_open.get());
        assert_eq!(review.invite.get(), InviteDraft::default());
        let toast = app.current_toast().unwrap();
        assert!(matches!(
            toast.kind,
            crate::state::ToastKind::Message {
                ref text,
                open: Some(ToastOpen::Subject { number: 77, .. }),
                ..
            } if text == "Asked for Octo-Cat to be added, in #77"
        ));
    }

    #[test]
    fn a_refusal_says_why_and_a_token_that_cannot_file_offers_github() {
        let (mut harness, app, review, _commands) = empty_window();
        review.invite.update(|d| {
            d.login = "octo-cat".into();
            d.note = "From SlateDB".into();
            d.sending = true;
        });
        click(&mut harness, "invite-button");
        apply(
            app,
            review,
            Event::InviteFailed(SendError {
                message: "Your token can't open issues on perch-code/perch.".into(),
                forbidden: true,
            }),
        );
        settle(&mut harness);
        assert!(!review.invite.with(|d| d.sending));
        assert!(shown(&harness, "invite-error"));
        click(&mut harness, "invite-github");
        let opened = crate::ui::widgets::opened();
        assert_eq!(opened.len(), 1);
        assert!(
            opened[0].starts_with(
                "https://github.com/perch-code/perch/issues/new?title=Invite%20%40octo-cat&body=Please%20add"
            ),
            "{opened:?}"
        );
        assert!(opened[0].contains("From%20SlateDB"), "{opened:?}");
    }
}
