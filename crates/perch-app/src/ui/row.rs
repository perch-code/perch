//! One notification row.

use floem::prelude::*;
use floem::views::{Container, Decorators, Empty, Label, Stack, dyn_container};
use floem::text::{Alignment, FontWeight};
use perch_github::{SubjectKind, Thread};

use crate::filter::Group;
use crate::ui::list::{MarkAction, OnMark, OpenThread};
use crate::icons::{self, SidebarIcon};
use crate::state::{AppState, Checked};
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{avatar, checkbox, icon_button, icon_in, reason_pill};

pub fn row(state: AppState, thread: Thread, on_open: OpenThread, on_mark: OnMark) -> impl IntoView {
    let unread = thread.unread;
    let hover_id = thread.id.clone();
    let leave_id = thread.id.clone();

    // A pull request opens the review view. Everything else has no view of
    // its own yet, so it opens where it lives.
    let review_target = if thread.subject.kind == SubjectKind::PullRequest {
        thread
            .number_hint()
            .map(|n| (thread.repository.full_name.clone(), n, thread.id.clone()))
    } else {
        None
    };
    let web_url = thread.html_url();

    Stack::horizontal((
        unread_dot(unread),
        row_checkbox(state, thread.clone()),
        state_icon(state, thread.clone()),
        text_column(state, thread.clone()),
        row_actions(state, thread.clone(), on_mark),
        reason_pill({
            let reason = thread.reason.clone();
            move || reason.label().to_string()
        })
        .style(|s| s.flex_shrink(0.0_f32).margin_right(10.0)),
        author_avatar(state, thread.clone()),
        timestamp(state, thread),
    ))
    .on_event_stop(el::Click, move |_, _| match &review_target {
        Some((repo, number, thread_id)) => on_open(repo, *number, thread_id),
        None => crate::ui::widgets::open_in_browser(&web_url),
    })
    // A bare keystroke acts on the row under the pointer, so the row has to
    // say when it is under it.
    .on_event_cont(el::PointerEnter, move |_, _| {
        state.hovered.set(Some(hover_id.clone()));
    })
    .on_event_cont(el::PointerLeave, move |_, _| {
        state.hovered.update(|h| {
            if h.as_deref() == Some(leave_id.as_str()) {
                *h = None;
            }
        });
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

/// The selection box, where the web puts it.
///
/// Always drawn, never only on hover: a half-made selection has to stay
/// legible while the reader looks down the list for the next one.
fn row_checkbox(state: AppState, thread: Thread) -> impl IntoView {
    let id = thread.id.clone();
    let toggle_id = thread.id.clone();

    checkbox(
        move || {
            if state.is_selected(&id) {
                Checked::Full
            } else {
                Checked::Empty
            }
        },
        move |extend| state.toggle_selected(&toggle_id, extend),
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// Save and dismiss, at the end of the row.
///
/// The space is reserved whether or not the buttons are showing, so a row
/// does not reflow under the pointer. The bookmark stays visible on a saved
/// thread: that is the only thing in the inbox that says it is saved.
fn row_actions(state: AppState, thread: Thread, on_mark: OnMark) -> impl IntoView {
    // Two closures, two clones: a closure capturing a String is not Copy, and
    // both halves of the pair need to ask the same question.
    let over_save = thread.id.clone();
    let hovering_save = move || state.hovered.with(|h| h.as_deref() == Some(over_save.as_str()));
    let over_done = thread.id.clone();
    let hovering_done = move || state.hovered.with(|h| h.as_deref() == Some(over_done.as_str()));

    let saved_id = thread.id.clone();
    let saved = move || state.is_saved(&saved_id);

    let save_id = thread.id.clone();
    let save_mark = on_mark.clone();
    let save = move || {
        let want = !state.is_saved(&save_id);
        save_mark(vec![save_id.clone()], MarkAction::Saved(want));
    };

    let done_id = thread.id.clone();
    let done_mark = on_mark;
    // The button follows the thread, not the view. Saved can hold a thread
    // that is also done, and offering to dismiss it again would be a lie.
    let act_id = thread.id.clone();
    let dismiss = move || {
        let restoring = state.is_done(&act_id);
        done_mark(vec![done_id.clone()], MarkAction::Done(!restoring));
    };
    let glyph_id = thread.id.clone();
    let restoring = move || state.is_done(&glyph_id);

    Stack::horizontal((
        dyn_container(
            move || (hovering_save(), saved()),
            move |(hovered, saved)| {
                if !hovered && !saved {
                    return Empty::new().into_any();
                }
                let save = save.clone();
                icon_button(
                    move || {
                        if saved {
                            SidebarIcon::BookmarkFilled
                        } else {
                            SidebarIcon::Bookmark
                        }
                    },
                    move || if saved { theme::ACCENT } else { theme::FG_SUBTLE },
                    save,
                )
                .into_any()
            },
        )
        .style(|s| s.size(26.0, 26.0)),
        dyn_container(
            move || (hovering_done(), restoring()),
            move |(hovered, restoring)| {
                if !hovered {
                    return Empty::new().into_any();
                }
                let dismiss = dismiss.clone();
                icon_button(
                    move || {
                        if restoring {
                            SidebarIcon::Undo
                        } else {
                            SidebarIcon::Check
                        }
                    },
                    || theme::FG_SUBTLE,
                    dismiss,
                )
                .into_any()
            },
        )
        .style(|s| s.size(26.0, 26.0)),
    ))
    .style(|s| s.items_center().gap(2.0).margin_right(8.0).flex_shrink(0.0_f32))
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
    let color_id = thread.id.clone();
    icon_in(
        move || match state.detail_for(&id) {
            Some(d) => icons::state_icon(d.state),
            None => icons::kind_icon(&kind),
        },
        move || match state.detail_for(&color_id) {
            Some(d) => icons::state_color(d.state),
            None => theme::FG_SUBTLE,
        },
    )
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
