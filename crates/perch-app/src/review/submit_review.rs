//! Submit a GitHub pull-request review from the title bar.
//!
//! This is deliberately separate from `review_launch`, which starts native
//! agent reviews of a branch. Here "review" is GitHub's COMMENT / APPROVE /
//! REQUEST_CHANGES mutation and the compact surface mirrors GitHub's finish-
//! review form without taking over the whole window. The drafts written
//! on lines are listed above the summary and go with it; the backend
//! reads them from the cache, so nothing here is passed on submit.

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::views::{
    dyn_container, dyn_stack, text_editor, Container, Decorators, Empty, Label, Stack,
};
use perch_github::PullReviewEvent;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{drafts_summary, Draft, ReviewState};
use crate::theme;
use crate::ui::widgets::{clickable, editor_box, icon_button, sidebar_glyph, text_button};

/// How much of a draft's text its row shows.
const EXCERPT_CHARS: usize = 60;

/// The upper-right action. It is live for a current pull request;
/// branches, issues, stale/offline checkouts and historical snapshots
/// cannot safely submit a remote verdict. While drafts exist it stays,
/// with their count, so the reader can see they are kept through a head
/// move or a spell offline, and does nothing until it is live again.
pub fn control(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Effect::new(move |_| {
        if !is_available(state) && state.submit_review_open.get() {
            close(state, false);
        }
    });

    // A Memo, so the threads changing under a button that is only held
    // does not build it again.
    let shown = Memo::new(move |_| is_shown(state));
    dyn_container(
        move || shown.get(),
        move |shown| {
            if shown {
                available_control(state, backend.clone()).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

pub(crate) fn is_available(state: ReviewState) -> bool {
    state.ready.get()
        && !state.offline.get()
        && state.stale_head.with(Option::is_none)
        && state.historical_review.with(Option::is_none)
        && state
            .pull_request
            .with(|pull| pull.as_ref().is_some_and(|pull| pull.is_open()))
}

/// Whether the button is drawn: live, or holding drafts for a pull
/// request that is still open.
pub(crate) fn is_shown(state: ReviewState) -> bool {
    is_available(state)
        || (state.ready.get()
            && state
                .pull_request
                .with(|pull| pull.as_ref().is_some_and(|pull| pull.is_open()))
            && state.comment_threads.with(|threads| {
                threads
                    .iter()
                    .any(|t| t.messages.iter().any(|m| m.is_draft()))
            }))
}

fn available_control(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let count = Memo::new(move |_| state.drafts().len());
    let inner = Stack::horizontal((
        Label::derived(move || {
            if state.submit_review_pending.with(Option::is_some) {
                "Submitting…".to_string()
            } else {
                match count.get() {
                    0 => "Submit review".to_string(),
                    n => format!("Submit review \u{b7} {n}"),
                }
            }
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::CANVAS)
                .selectable(false)
                .pointer_events_none()
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::CANVAS)
            .style(|s| s.size(9.0, 9.0).margin_left(7.0).pointer_events_none()),
    ))
    .style(|s| s.items_center().pointer_events_none());
    let button = Container::new(inner);
    let button_id = button.view_id();
    let toggle: Rc<dyn Fn()> = Rc::new(move || {
        if state.submit_review_pending.with_untracked(Option::is_some) || !is_available(state) {
            return;
        }
        if state.submit_review_open.get_untracked() {
            cancel(state, true);
        } else {
            open(state, Some(button_id));
        }
    });
    let click = toggle.clone();
    let keyboard = toggle.clone();
    let button = button
        .on_event_stop(el::Click, move |_, _| click())
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                keyboard();
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            let pending = state.submit_review_pending.with(Option::is_some);
            let held = pending || !is_available(state);
            clickable(s)
                .height(30.0)
                .padding_horiz(12.0)
                .items_center()
                .background(theme::FG)
                .border_radius(9.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
                .set_disabled(held)
                .apply_if(held, |s| {
                    s.opacity(0.75_f32).cursor(CursorStyle::Default)
                })
                .apply_if(!held, |s| s.hover(|s| s.background(theme::ACTION_HOVER)))
        })
        .debug_name("submit-review-control");

    let popover_backend = backend.clone();
    // Built anew on each submission that comes back with something left
    // to say, so the summary box starts empty: what was in it posted.
    let shown = Memo::new(move |_| {
        (
            state.submit_review_open.get(),
            state.submit_review_generation.get(),
        )
    });
    Stack::new((
        button,
        dyn_container(
            move || shown.get(),
            move |(open, _)| {
                if open {
                    popover(state, popover_backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(34.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape
            && key.modifiers.is_empty()
            && state.submit_review_open.get_untracked()
        {
            cancel(state, true);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| s.position(Position::Relative).flex_shrink(0.0_f32))
}

fn open(state: ReviewState, opener: Option<floem::ViewId>) {
    if !is_available(state) {
        return;
    }
    crate::review::review_launch::close_launcher(state, false);
    state.submit_review_opener.set(opener);
    state.submit_review_open.set(true);
}

/// A visible dismissal never pretends to cancel a mutation already sent to
/// GitHub. External navigation can still force-close the surface through
/// `close`; reopening it preserves any eventual failure.
fn cancel(state: ReviewState, restore_focus: bool) {
    if state.submit_review_pending.with_untracked(Option::is_none) {
        close(state, restore_focus);
    }
}

pub(crate) fn close(state: ReviewState, restore_focus: bool) {
    state.submit_review_open.set(false);
    state.submit_review_focused.set(false);
    state.submit_review_confirm_discard.set(false);
    state.submit_review_outcome.set(None);
    if restore_focus {
        if let Some(opener) = state.submit_review_opener.get_untracked() {
            opener.request_focus();
        }
    }
    state.submit_review_opener.set(None);
}

fn popover(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let submit_backend = backend.clone();
    let submit: Rc<dyn Fn()> = Rc::new(move || submit(state, &submit_backend));
    let editor = review_editor(state, submit.clone());
    let drafts = drafts_list(state, backend);

    let close_button = icon_action(
        SidebarIcon::Close,
        "submit-review-close",
        move || state.submit_review_pending.with(Option::is_none),
        move || cancel(state, true),
    );

    let content = Stack::vertical((
        Stack::horizontal((
            Stack::vertical((
                Label::derived(|| "Submit review".to_string()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::FG)
                }),
                Label::derived(|| "Leave a summary and choose an outcome.".to_string()).style(
                    |s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::FG_SUBTLE)
                            .margin_top(2.0)
                    },
                ),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
            close_button,
        ))
        .style(|s| s.items_start().width_full().gap(8.0)),
        drafts,
        editor,
        outcome_group(state),
        dyn_container(
            move || state.submit_review_outcome.get(),
            |outcome| match outcome {
                Some(outcome) => Label::derived(move || outcome.clone())
                    .style(|s| {
                        s.width_full()
                            .font_size(theme::TEXT_BADGE)
                            .color(theme::CLOSED)
                            .text_wrap()
                    })
                    .debug_name("submit-review-outcome")
                    .into_any(),
                None => Empty::new().into_any(),
            },
        ),
        dyn_container(
            move || state.submit_review_error.get(),
            |error| match error {
                Some(error) => Label::derived(move || error.clone())
                    .style(|s| {
                        s.width_full()
                            .font_size(theme::TEXT_BADGE)
                            .color(theme::CLOSED)
                            .text_wrap()
                    })
                    .debug_name("submit-review-error")
                    .into_any(),
                None => Empty::new().into_any(),
            },
        ),
        Stack::horizontal((
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            action_button(
                || "Cancel".to_string(),
                "submit-review-cancel",
                false,
                move || state.submit_review_pending.with(Option::is_none),
                move || cancel(state, true),
            ),
            action_button(
                move || {
                    if state.submit_review_pending.with(Option::is_some) {
                        "Submitting…".to_string()
                    } else {
                        submit_label(state.submit_review_event.get()).to_string()
                    }
                },
                "submit-review-action",
                true,
                move || can_submit(state),
                move || submit(),
            ),
        ))
        .style(|s| s.items_center().width_full().gap(8.0)),
    ))
    .style(|s| s.flex_col().width_full().gap(12.0));

    Container::new(content)
        .style(|s| {
            s.width(430.0)
                .max_width_full()
                .padding(14.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(10.0)
                .box_shadow_blur(22.0)
                .box_shadow_v_offset(7.0)
                .box_shadow_color(theme::SHADOW)
        })
        .debug_name("submit-review-popover")
}

/// The drafts going with the review, one row each, with what they add up
/// to and Discard all under them. Nothing while there are none.
fn drafts_list(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let drafts = Memo::new(move |_| state.drafts());
    let head = Memo::new(move |_| {
        state
            .pull_request
            .with(|pull| pull.as_ref().map(|p| p.head_sha.clone()))
            .unwrap_or_default()
    });
    let rows_backend = backend.clone();
    // Keyed on the text too: a draft edited in its card while the form is
    // open gets a new row, with the new text.
    let rows = dyn_stack(
        move || drafts.get(),
        |draft| (draft.message_id.clone(), draft.body.clone()),
        move |draft| draft_row(state, rows_backend.clone(), draft, head),
    )
    .style(|s| s.flex_col().width_full());

    let discard_backend = backend.clone();
    let discard_all = move || {
        if state.submit_review_pending.with_untracked(Option::is_some) {
            return;
        }
        if let Some(item_id) = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone())) {
            discard_backend.discard_drafts(&item_id);
        }
        state.submit_review_confirm_discard.set(false);
    };
    let foot = dyn_container(
        move || state.submit_review_confirm_discard.get(),
        move |asking| {
            if asking {
                let discard_all = discard_all.clone();
                Stack::horizontal((
                    Label::derived(|| "Discard all drafts?".to_string()).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .font_weight(theme::weight_emphasis())
                            .color(theme::FG)
                            .selectable(false)
                    }),
                    Empty::new().style(|s| s.flex_grow(1.0_f32)),
                    text_button(|| "Discard".to_string(), SidebarIcon::Trash, discard_all)
                        .debug_name("discard-all-confirm"),
                    text_button(|| "Cancel".to_string(), SidebarIcon::Close, move || {
                        state.submit_review_confirm_discard.set(false)
                    }),
                ))
                .style(|s| {
                    s.items_center()
                        .width_full()
                        .gap(6.0)
                        .padding(8.0)
                        .background(theme::NOTICE_BG)
                        .border(1.0)
                        .border_color(theme::NOTICE_BORDER)
                        .border_radius(theme::RADIUS)
                })
                .into_any()
            } else {
                Stack::horizontal((
                    Label::derived(move || drafts.with(|d| drafts_summary(d)))
                        .debug_name("drafts-summary")
                        .style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::FG_MUTED)
                                .selectable(false)
                        }),
                    Empty::new().style(|s| s.flex_grow(1.0_f32)),
                    Container::new(
                        Label::derived(|| "Discard all".to_string()).style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::FG_MUTED)
                                .selectable(false)
                                .pointer_events_none()
                        }),
                    )
                    .on_event_stop(el::Click, move |_, _| {
                        if state.submit_review_pending.with_untracked(Option::is_none) {
                            state.submit_review_confirm_discard.set(true);
                        }
                    })
                    .style(|s| {
                        clickable(s)
                            .padding_horiz(6.0)
                            .padding_vert(3.0)
                            .border_radius(theme::RADIUS)
                            .hover(|s| s.background(theme::CANVAS_HOVER))
                    })
                    .debug_name("discard-all"),
                ))
                .style(|s| s.items_center().width_full())
                .into_any()
            }
        },
    )
    .style(|s| s.width_full());

    Stack::vertical((rows, foot))
        .debug_name("submit-review-drafts")
        .style(move |s| {
            s.flex_col()
                .width_full()
                .gap(4.0)
                .apply_if(drafts.with(Vec::is_empty), |s| s.hide())
        })
}

/// One draft: where it goes, in the code font, `written against` its
/// commit when that is not the head and the draft opens a thread there
/// (a reply carries no commit), the start of its text, and a delete while
/// the pointer is over it.
fn draft_row(
    state: ReviewState,
    backend: Rc<Backend>,
    draft: Draft,
    head: Memo<String>,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let location = draft_location(&draft);
    let reply = draft.reply;
    let commit = draft.commit_sha.clone();
    let excerpt = excerpt(&draft.body);
    let message_id = draft.message_id.clone();
    let delete = move || {
        if state.submit_review_pending.with_untracked(Option::is_some) {
            return;
        }
        if let Some(item_id) = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone())) {
            backend.delete_draft(&item_id, &message_id);
        }
    };
    let written_against = move || {
        let head = head.get();
        (!reply && !commit.is_empty() && commit != head)
            .then(|| format!("written against {}", short_sha(&commit)))
    };
    crate::review::cards::track_hover(
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Reply, theme::FG_MUTED)
                .style(move |s| {
                    s.size(12.0, 12.0)
                        .margin_right(4.0)
                        .flex_shrink(0.0_f32)
                        .apply_if(!reply, |s| s.hide())
                })
                .debug_name("draft-reply-glyph"),
            Label::derived(move || location.clone())
                .debug_name("draft-location")
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::FG)
                        .flex_shrink(0.0_f32)
                        .selectable(false)
                }),
            dyn_container(written_against, |note| match note {
                Some(note) => Label::derived(move || note.clone())
                    .debug_name("draft-written-against")
                    .style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .color(theme::FG_MUTED)
                            .margin_left(6.0)
                            .flex_shrink(0.0_f32)
                            .selectable(false)
                    })
                    .into_any(),
                None => Empty::new().into_any(),
            }),
            Label::derived(move || excerpt.clone())
                .debug_name("draft-excerpt")
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .margin_left(8.0)
                        .min_width(0.0)
                        .flex_grow(1.0_f32)
                        .text_ellipsis()
                        .selectable(false)
                }),
            crate::review::cards::tip_with(
                icon_button(|| SidebarIcon::Trash, || theme::FG_SUBTLE, delete)
                    .debug_name("delete-draft")
                    .style(move |s| {
                        s.size(22.0, 22.0)
                            .flex_shrink(0.0_f32)
                            .apply_if(!hovered.get(), |s| s.hide())
                    }),
                || "Delete".to_string(),
            ),
        ))
        .style(|s| {
            s.items_center()
                .width_full()
                .min_height(26.0)
                .padding_horiz(6.0)
                .border_radius(theme::RADIUS)
                .hover(|s| s.background(theme::CANVAS_SUBTLE))
        })
        .debug_name("draft-row"),
        hovered,
    )
}

/// `src/x.rs:41`, or `Reply` for a draft with no lines, which can only
/// be a reply under a thread that has none of its own.
fn draft_location(draft: &Draft) -> String {
    match &draft.anchor {
        Some(anchor) => anchor.label(),
        None => "Reply".to_string(),
    }
}

/// The first sixty characters of a draft on one line, with an ellipsis
/// when there is more.
fn excerpt(body: &str) -> String {
    let one_line: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one_line.chars().take(EXCERPT_CHARS).collect();
    if one_line.chars().count() > EXCERPT_CHARS {
        out.push('\u{2026}');
    }
    out
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn review_editor(state: ReviewState, submit: Rc<dyn Fn()>) -> impl View {
    use floem::views::editor::command::CommandExecuted;

    use crate::review::threads::cmd_enter;

    let editor = text_editor(state.submit_review_draft.get_untracked())
        .placeholder("Leave a review…".to_string())
        .editor_style(|s| s.hide_gutter(true).selection_color(theme::SELECTION_BG));
    let styling_doc = editor.editor().doc();
    let styling_id = state.next_token();
    let editor = editor.styling(crate::review::composer_styling::ComposerStyling::new(
        styling_doc,
        styling_id,
    ));
    let key_submit = submit.clone();
    let editor = editor
        // Cmd-Enter submits, before the editor can take it for a line.
        .pre_command(move |cmd| {
            if cmd_enter(&cmd) {
                key_submit();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(editor) = update.editor {
                state
                    .submit_review_draft
                    .set(editor.doc().text().to_string());
                state.submit_review_error.set(None);
            }
        })
        .on_event_cont(el::FocusGained, move |_, _| {
            state.submit_review_focused.set(true)
        })
        .on_event_cont(el::FocusLost, move |_, _| {
            state.submit_review_focused.set(false)
        })
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    cancel(state, true);
                    EventPropagation::Stop
                } else {
                    EventPropagation::Continue
                }
            },
        )
        .style(move |s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .set_disabled(state.submit_review_pending.with(Option::is_some))
        })
        .debug_name("submit-review-editor");

    let inner = editor.editor().editor_view_id;
    let asked = Rc::new(Cell::new(false));
    let editor = editor.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if asked.replace(true) {
            return;
        }
        if state.submit_review_autofocus.get_untracked() {
            if let Some(id) = inner.get_untracked() {
                id.request_focus();
            }
        }
    });
    editor_box(editor, 96.0, true).style(move |s| {
        s.apply_if(state.submit_review_pending.with(Option::is_some), |s| {
            s.opacity(0.6_f32)
        })
    })
}

#[derive(Clone, Copy)]
struct OutcomeFocus {
    comment: floem::ViewId,
    approve: floem::ViewId,
    request_changes: floem::ViewId,
}

impl OutcomeFocus {
    fn for_event(self, event: PullReviewEvent) -> floem::ViewId {
        match event {
            PullReviewEvent::Comment => self.comment,
            PullReviewEvent::Approve => self.approve,
            PullReviewEvent::RequestChanges => self.request_changes,
        }
    }
}

fn outcome_group(state: ReviewState) -> impl IntoView {
    let comment = outcome_row_content(state, PullReviewEvent::Comment);
    let approve = outcome_row_content(state, PullReviewEvent::Approve);
    let request_changes = outcome_row_content(state, PullReviewEvent::RequestChanges);
    let focus = OutcomeFocus {
        comment: comment.view_id(),
        approve: approve.view_id(),
        request_changes: request_changes.view_id(),
    };

    Stack::vertical((
        wire_outcome_row(comment, state, PullReviewEvent::Comment, focus),
        wire_outcome_row(approve, state, PullReviewEvent::Approve, focus),
        wire_outcome_row(
            request_changes,
            state,
            PullReviewEvent::RequestChanges,
            focus,
        ),
    ))
    .style(|s| s.flex_col().width_full().gap(2.0))
}

fn outcome_row_content(state: ReviewState, event: PullReviewEvent) -> impl View {
    Stack::horizontal((
        dyn_container(
            move || state.submit_review_event.get() == event,
            |selected| {
                Container::new(Empty::new().style(move |s| {
                    s.size(7.0, 7.0).border_radius(4.0).background(if selected {
                        theme::FG
                    } else {
                        theme::CANVAS
                    })
                }))
                .style(move |s| {
                    s.size(17.0, 17.0)
                        .items_center()
                        .justify_center()
                        .border(1.5)
                        .border_color(if selected {
                            theme::FG
                        } else {
                            theme::FG_SUBTLE
                        })
                        .border_radius(9.0)
                })
            },
        )
        .style(|s| s.flex_shrink(0.0_f32).margin_top(2.0)),
        Stack::vertical((
            Label::derived(move || outcome_label(event).to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
                    .selectable(false)
                    .pointer_events_none()
            }),
            Label::derived(move || outcome_description(event).to_string()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .selectable(false)
                    .pointer_events_none()
                    .margin_top(1.0)
            }),
        ))
        .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
    ))
    .debug_name(format!("submit-review-option:{}", event_slug(event)))
}

fn wire_outcome_row(
    row: impl View + 'static,
    state: ReviewState,
    event: PullReviewEvent,
    focus: OutcomeFocus,
) -> impl View {
    let select: Rc<dyn Fn()> = Rc::new(move || {
        if state.submit_review_pending.with_untracked(Option::is_none) {
            state.submit_review_event.set(event);
            state.submit_review_error.set(None);
        }
    });
    let click = select.clone();
    let keyboard = select.clone();
    row.on_event_stop(el::Click, move |_, _| {
        click();
        if state.submit_review_pending.with_untracked(Option::is_none) {
            focus.for_event(event).request_focus();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        let destination = if key.modifiers.is_empty() {
            match key.code {
                Code::ArrowLeft | Code::ArrowUp => Some(adjacent_outcome(event, false)),
                Code::ArrowRight | Code::ArrowDown => Some(adjacent_outcome(event, true)),
                _ => None,
            }
        } else {
            None
        };
        if let Some(destination) = destination {
            if state.submit_review_pending.with_untracked(Option::is_none) {
                state.submit_review_event.set(destination);
                state.submit_review_error.set(None);
                focus.for_event(destination).request_focus();
            }
            return EventPropagation::Stop;
        }

        if matches!(key.code, Code::Enter | Code::Space) {
            keyboard();
            return EventPropagation::Stop;
        }
        EventPropagation::Continue
    })
    .style(move |s| {
        let enabled = state.submit_review_pending.with(Option::is_none);
        let selected = state.submit_review_event.get() == event;
        clickable(s)
            .width_full()
            .items_start()
            .gap(9.0)
            .padding_vert(7.0)
            .padding_horiz(8.0)
            .border_radius(theme::RADIUS)
            // A radio group is one Tab stop. Arrow-key selection moves the
            // focusable-but-not-tabbable peers into the active slot.
            .apply_if(selected, |s| s.keyboard_navigable())
            .apply_if(!selected, |s| s.focusable())
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .set_disabled(!enabled)
            .apply_if(enabled, |s| s.hover(|s| s.background(theme::CANVAS_SUBTLE)))
            .apply_if(!enabled, |s| {
                s.opacity(0.5_f32).cursor(CursorStyle::Default)
            })
    })
    .into_view()
}

fn adjacent_outcome(event: PullReviewEvent, forward: bool) -> PullReviewEvent {
    match (event, forward) {
        (PullReviewEvent::Comment, true) | (PullReviewEvent::RequestChanges, false) => {
            PullReviewEvent::Approve
        }
        (PullReviewEvent::Approve, true) => PullReviewEvent::RequestChanges,
        (PullReviewEvent::Approve, false) => PullReviewEvent::Comment,
        (PullReviewEvent::RequestChanges, true) => PullReviewEvent::Comment,
        (PullReviewEvent::Comment, false) => PullReviewEvent::RequestChanges,
    }
}

fn can_submit(state: ReviewState) -> bool {
    if state.submit_review_pending.with(Option::is_some) || !is_available(state) {
        return false;
    }
    review_body_is_valid(
        state.submit_review_event.get(),
        &state.submit_review_draft.get(),
        !state.comment_threads.with(|threads| {
            threads
                .iter()
                .all(|t| t.messages.iter().all(|m| !m.is_draft()))
        }),
    )
}

/// Approve needs nothing. Comment needs a summary or a draft on a line.
/// Request changes needs a summary.
fn review_body_is_valid(event: PullReviewEvent, body: &str, has_drafts: bool) -> bool {
    match event {
        PullReviewEvent::Approve => true,
        PullReviewEvent::Comment => has_drafts || !body.trim().is_empty(),
        PullReviewEvent::RequestChanges => !body.trim().is_empty(),
    }
}

fn submit(state: ReviewState, backend: &Backend) {
    if state.submit_review_pending.with_untracked(Option::is_some) {
        return;
    }
    let event = state.submit_review_event.get_untracked();
    let body = state.submit_review_draft.get_untracked();
    let has_drafts = state.comment_threads.with_untracked(|threads| {
        threads
            .iter()
            .any(|t| t.messages.iter().any(|m| m.is_draft()))
    });
    if !review_body_is_valid(event, &body, has_drafts) {
        state.submit_review_error.set(Some(match event {
            PullReviewEvent::Comment => "Write a comment before submitting.".into(),
            PullReviewEvent::RequestChanges => {
                "Explain what needs to change before submitting.".into()
            }
            PullReviewEvent::Approve => unreachable!(),
        }));
        return;
    }
    let Some(item_id) = state
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
    else {
        state
            .submit_review_error
            .set(Some("This pull request is no longer open in Perch.".into()));
        return;
    };
    let Some(pull) = state.pull_request.get_untracked() else {
        state
            .submit_review_error
            .set(Some("This pull request is no longer open in Perch.".into()));
        return;
    };
    if !is_available(state) {
        state.submit_review_error.set(Some(
            "Refresh the current pull request before submitting a review.".into(),
        ));
        return;
    }

    match backend.submit_pull_request_review(
        &item_id,
        &pull.repo_full_name,
        pull.number,
        &pull.head_sha,
        body.trim(),
        event,
    ) {
        Some(attempt_id) => state.begin_submit_review(attempt_id),
        None => state.submit_review_error.set(Some(
            "A review is already being submitted. Wait for it to finish.".into(),
        )),
    }
}

fn action_button(
    label: impl Fn() -> String + 'static,
    debug: &'static str,
    prominent: bool,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl View {
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let event_enabled = enabled.clone();
    let style_enabled = enabled.clone();
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(Label::derived(label).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(theme::weight_emphasis())
            .color(if prominent {
                theme::CANVAS
            } else {
                theme::FG_MUTED
            })
            .selectable(false)
            .pointer_events_none()
    }))
    .on_event_stop(el::Click, move |_, _| {
        if event_enabled() {
            click();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) && enabled() {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let available = style_enabled();
        clickable(s)
            .height(30.0)
            .padding_horiz(11.0)
            .items_center()
            .justify_center()
            .background(if prominent { theme::FG } else { theme::CANVAS })
            .border(1.0)
            .border_color(if prominent { theme::FG } else { theme::BORDER })
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .set_disabled(!available)
            .apply_if(!available, |s| {
                s.opacity(0.45_f32).cursor(CursorStyle::Default)
            })
            .apply_if(available, |s| {
                s.hover(move |s| {
                    s.background(if prominent {
                        theme::ACTION_HOVER
                    } else {
                        theme::CANVAS_HOVER
                    })
                })
            })
    })
    .debug_name(debug)
    .into_view()
}

fn icon_action(
    icon: SidebarIcon,
    debug: &'static str,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl View {
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let event_enabled = enabled.clone();
    let style_enabled = enabled.clone();
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(
        sidebar_glyph(icon, theme::FG_MUTED).style(|s| s.size(14.0, 14.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| {
        if event_enabled() {
            click();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) && enabled() {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let available = style_enabled();
        clickable(s)
            .size(28.0, 28.0)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .set_disabled(!available)
            .apply_if(!available, |s| {
                s.opacity(0.45_f32).cursor(CursorStyle::Default)
            })
            .apply_if(available, |s| {
                s.hover(|s| s.background(theme::CANVAS_HOVER))
            })
    })
    .debug_name(debug)
    .into_view()
}

fn outcome_label(event: PullReviewEvent) -> &'static str {
    match event {
        PullReviewEvent::Comment => "Comment",
        PullReviewEvent::Approve => "Approve",
        PullReviewEvent::RequestChanges => "Request changes",
    }
}

fn outcome_description(event: PullReviewEvent) -> &'static str {
    match event {
        PullReviewEvent::Comment => "Submit feedback without an explicit verdict.",
        PullReviewEvent::Approve => "Approve merging these changes.",
        PullReviewEvent::RequestChanges => "Ask for changes before this is merged.",
    }
}

fn submit_label(event: PullReviewEvent) -> &'static str {
    match event {
        PullReviewEvent::Comment => "Submit comment",
        PullReviewEvent::Approve => "Approve",
        PullReviewEvent::RequestChanges => "Request changes",
    }
}

fn event_slug(event: PullReviewEvent) -> &'static str {
    match event {
        PullReviewEvent::Comment => "comment",
        PullReviewEvent::Approve => "approve",
        PullReviewEvent::RequestChanges => "request-changes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Command;
    use crate::state::AppState;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::{Key, KeyState, Modifiers, NamedKey};
    use perch_github::{
        Anchor, Author, CommentMessage, CommentThread, Item, ItemCheckout, PullRequest, Side,
        ThreadOrigin,
    };
    use std::sync::Arc;

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn key(code: Code) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: match code {
                Code::Enter => Key::Named(NamedKey::Enter),
                Code::Space => Key::Character(" ".into()),
                Code::Escape => Key::Named(NamedKey::Escape),
                Code::ArrowLeft => Key::Named(NamedKey::ArrowLeft),
                Code::ArrowRight => Key::Named(NamedKey::ArrowRight),
                Code::ArrowUp => Key::Named(NamedKey::ArrowUp),
                Code::ArrowDown => Key::Named(NamedKey::ArrowDown),
                _ => unreachable!("only review-control keys are used"),
            },
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    fn enter(modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Named(NamedKey::Enter),
            code: Code::Enter,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 42,
            title: "Review support".into(),
            html_url: "https://github.com/acme/repo/pull/42".into(),
            author_login: "octocat".into(),
            author_avatar_url: String::new(),
            head_sha: "head-42".into(),
            head_ref: "feature".into(),
            base_sha: "base".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 1,
            deletions: 0,
            changed_files: 1,
            repo_full_name: "acme/repo".into(),
            head_repo_full_name: Some("acme/repo".into()),
            updated_at: None,
            review_requested: true,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: Vec::new(),
            milestone: None,
            labels: Vec::new(),
            assignees: Vec::new(),
        }
    }

    fn review_state() -> ReviewState {
        let state = ReviewState::new();
        let mut item = Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-42".into();
        item.checkout = Some(ItemCheckout {
            branch: "feature".into(),
            base_ref: "main".into(),
            head_sha: "head-42".into(),
            path: "/tmp/perch-submit-review-test".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull())));
        state.ready.set(true);
        state.submit_review_autofocus.set(false);
        state
    }

    #[test]
    fn only_approval_allows_an_empty_body_without_drafts() {
        assert!(!review_body_is_valid(PullReviewEvent::Comment, " \n", false));
        assert!(review_body_is_valid(
            PullReviewEvent::Comment,
            "Looks good overall",
            false
        ));
        assert!(review_body_is_valid(PullReviewEvent::Approve, "", false));
        assert!(!review_body_is_valid(PullReviewEvent::RequestChanges, "", false));
        assert!(review_body_is_valid(
            PullReviewEvent::RequestChanges,
            "Please cover the failure path",
            false
        ));
        // Drafts on lines are a comment review's content; a request for
        // changes still says what to change.
        assert!(review_body_is_valid(PullReviewEvent::Comment, "", true));
        assert!(!review_body_is_valid(PullReviewEvent::RequestChanges, "", true));
    }

    #[test]
    fn a_drafts_text_is_cut_to_one_line_of_sixty() {
        assert_eq!(excerpt("short"), "short");
        assert_eq!(excerpt("two\nlines  here"), "two lines here");
        let long = "a".repeat(61);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), 61);
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(excerpt(&"a".repeat(60)), "a".repeat(60));
    }

    /// A thread on `item-42` with one draft, `{id}-d1`, at `src/x.rs:41`
    /// of `commit`, replying under GitHub's comment `root` when given.
    fn draft_thread(id: &str, commit: &str, root: Option<u64>, body: &str) -> CommentThread {
        let at: jiff::Timestamp = "2026-09-20T10:00:00Z".parse().unwrap();
        CommentThread {
            id: id.into(),
            worktree_id: "item-42".into(),
            origin: ThreadOrigin::GitHub,
            github_id: root,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: 41,
                line: 41,
                side: Side::Right,
                commit_sha: commit.into(),
            }),
            outdated: false,
            resolved: false,
            created_at: at,
            messages: vec![CommentMessage {
                id: format!("{id}-d1"),
                thread_id: id.into(),
                origin: ThreadOrigin::GitHub,
                author: Author::User,
                body: body.into(),
                created_at: at,
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            }],
        }
    }

    /// The first view whose debug name contains `text`.
    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    /// Every view whose debug name contains `text`, in tree order.
    fn find_all(id: floem::ViewId, text: &str, out: &mut Vec<floem::ViewId>) {
        if id.debug_name().contains(text) {
            out.push(id);
        }
        for child in id.children() {
            find_all(child, text, out);
        }
    }

    fn shown(harness: &HeadlessHarness, id: floem::ViewId) -> bool {
        let rect = harness.get_layout_rect(id);
        rect.width() > 0.0 && rect.height() > 0.0
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no view {name}"));
        let center = id.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        harness.rebuild();
        harness.paint();
    }

    #[test]
    fn the_button_counts_the_drafts_and_stays_while_they_are_kept() {
        let root = TestRoot::new();
        let state = review_state();
        state.comment_threads.set(Arc::new(vec![
            draft_thread("t1", "head-42", None, "one"),
            draft_thread("t2", "head-42", Some(31), "two"),
        ]));
        let view = control(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Submit review \u{b7} 2").is_some());

        // The head moved: the button stays, with the count, and does not
        // open the form.
        state.stale_head.set(Some("new-head".into()));
        harness.rebuild();
        harness.paint();
        assert!(is_shown(state));
        let control = named(harness.root_id(), "submit-review-control").unwrap();
        assert!(shown(&harness, control));
        assert!(find(harness.root_id(), "Submit review \u{b7} 2").is_some());
        click(&mut harness, "submit-review-control");
        assert!(!state.submit_review_open.get(), "unavailable while the head is stale");

        // Without drafts a stale head hides it, as before.
        state.comment_threads.set(Arc::new(Vec::new()));
        harness.rebuild();
        harness.paint();
        assert!(!is_shown(state));
        assert!(named(harness.root_id(), "submit-review-control").is_none());

        state.stale_head.set(None);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Label: \"Submit review\"").is_some());
    }

    #[test]
    fn the_form_lists_the_drafts_with_where_they_go_and_sums_them_up() {
        let root = TestRoot::new();
        let state = review_state();
        state.comment_threads.set(Arc::new(vec![
            draft_thread(
                "t1",
                "head-42",
                None,
                "Rename this, it reads as the count of the other thing entirely",
            ),
            draft_thread("t2", "0ld5ha1234", None, "Older"),
            draft_thread("t3", "head-42", Some(31), "A reply"),
        ]));
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let mut rows = Vec::new();
        find_all(harness.root_id(), "draft-row", &mut rows);
        assert_eq!(rows.len(), 3);
        let mut locations = Vec::new();
        find_all(harness.root_id(), "draft-location", &mut locations);
        assert_eq!(locations.len(), 3);
        for location in &locations {
            assert!(shown(&harness, *location));
        }
        let mut excerpts = Vec::new();
        find_all(harness.root_id(), "draft-excerpt", &mut excerpts);
        let first = excerpts[0].debug_name();
        assert!(
            first.contains("Rename this, it reads as the count of the other thing entire\u{2026}"),
            "{first}"
        );
        assert!(excerpts[1].debug_name().contains("Older"));

        // The draft written against an older head says so; the others say nothing.
        let mut notes = Vec::new();
        find_all(harness.root_id(), "draft-written-against", &mut notes);
        assert_eq!(notes.len(), 1);
        let note = notes[0].debug_name();
        assert!(note.contains("written against 0ld5ha1"), "{note}");

        // The reply glyph is drawn on the reply's row alone.
        let mut glyphs = Vec::new();
        find_all(harness.root_id(), "draft-reply-glyph", &mut glyphs);
        assert_eq!(glyphs.len(), 3);
        assert!(!shown(&harness, glyphs[0]));
        assert!(!shown(&harness, glyphs[1]));
        assert!(shown(&harness, glyphs[2]));

        assert!(find(harness.root_id(), "Label: \"1 reply, 2 comments\"").is_some());

        // Delete is drawn while the pointer is over the row, and deletes
        // that draft.
        let delete = find(rows[1], "delete-draft").unwrap();
        assert!(!shown(&harness, delete), "hidden until hovered");
        let center = locations[1].get_visual_rect_no_clip().center();
        harness.pointer_move(center.x, center.y);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, delete));
        let at = delete.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        match commands.try_recv() {
            Ok(Command::DeleteDraft { item_id, message_id }) => {
                assert_eq!(item_id, "item-42");
                assert_eq!(message_id, "t2-d1");
            }
            other => panic!("delete did not reach the backend: {other:?}"),
        }
    }

    #[test]
    fn a_comment_review_submits_with_drafts_and_no_summary() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        state.comment_threads.set(Arc::new(vec![draft_thread("t1", "head-42", None, "one")]));
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(can_submit(state));

        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::SubmitPullRequestReview { body, event, .. }) => {
                assert_eq!(body, "", "the drafts are the content; the backend reads them");
                assert_eq!(event, PullReviewEvent::Comment);
            }
            other => panic!("Cmd-Enter did not submit: {other:?}"),
        }

        // A request for changes still needs its summary.
        state.submit_review_pending.set(None);
        state.submit_review_event.set(PullReviewEvent::RequestChanges);
        assert!(!can_submit(state));
    }

    #[test]
    fn discard_all_asks_once_and_then_discards_every_draft() {
        let root = TestRoot::new();
        let state = review_state();
        state.comment_threads.set(Arc::new(vec![
            draft_thread("t1", "head-42", None, "one"),
            draft_thread("t2", "head-42", None, "two"),
        ]));
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        click(&mut harness, "discard-all");
        assert!(state.submit_review_confirm_discard.get());
        assert!(find(harness.root_id(), "Discard all drafts?").is_some());
        assert!(commands.try_recv().is_err(), "asking discards nothing");

        click(&mut harness, "discard-all-confirm");
        assert!(!state.submit_review_confirm_discard.get());
        match commands.try_recv() {
            Ok(Command::DiscardDrafts { item_id }) => assert_eq!(item_id, "item-42"),
            other => panic!("Discard did not reach the backend: {other:?}"),
        }

        // Closing the form drops the question.
        click(&mut harness, "discard-all");
        assert!(state.submit_review_confirm_discard.get());
        close(state, false);
        assert!(!state.submit_review_confirm_discard.get());

        // With no drafts there is no list.
        state.comment_threads.set(Arc::new(Vec::new()));
        open(state, None);
        harness.rebuild();
        harness.paint();
        let list = named(harness.root_id(), "submit-review-drafts").unwrap();
        assert!(!shown(&harness, list));
    }

    #[test]
    fn what_did_not_post_is_said_over_an_empty_summary_and_stays_through_typing() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        state.submit_review_draft.set("Looks good".into());
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        assert!(commands.try_recv().is_ok());
        let attempt = state.submit_review_pending.get_untracked().unwrap();

        let accepted = perch_github::Review {
            node_id: String::new(),
            reactions: Default::default(),
            id: 55,
            user: "reviewer".into(),
            body: "Looks good".into(),
            state: "COMMENTED".into(),
            submitted_at: None,
            html_url: String::new(),
        };
        assert!(state.finish_submit_review(
            attempt,
            Some(accepted),
            Some("Posted the review. 1 reply did not post: boom.".into()),
        ));
        harness.rebuild();
        harness.paint();

        assert!(state.submit_review_open.get_untracked());
        assert!(named(harness.root_id(), "submit-review-outcome").is_some());
        assert!(named(harness.root_id(), "submit-review-error").is_none());

        // The summary posted: the box is empty, so a keystroke is just
        // that keystroke, and the line about the reply stays above it.
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.submit_review_draft.get_untracked(), "\n");
        assert_eq!(
            state.submit_review_outcome.get_untracked().as_deref(),
            Some("Posted the review. 1 reply did not post: boom.")
        );
        assert!(named(harness.root_id(), "submit-review-outcome").is_some());

        // Choosing a verdict keeps it too; the next submission clears it.
        click(&mut harness, "submit-review-option:approve");
        assert!(state.submit_review_outcome.get_untracked().is_some());
        state.begin_submit_review(attempt + 1);
        assert!(state.submit_review_outcome.get_untracked().is_none());
    }

    #[test]
    fn a_draft_edited_while_the_form_is_open_shows_its_new_text() {
        let root = TestRoot::new();
        let state = review_state();
        state.comment_threads.set(Arc::new(vec![draft_thread("t1", "head-42", None, "first")]));
        open(state, None);
        let view = control(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();
        let mut excerpts = Vec::new();
        find_all(harness.root_id(), "draft-excerpt", &mut excerpts);
        assert_eq!(excerpts.len(), 1);
        assert!(excerpts[0].debug_name().contains("first"));

        // Saved again from its card, same id, new text.
        state.comment_threads.set(Arc::new(vec![draft_thread("t1", "head-42", None, "second")]));
        harness.rebuild();
        harness.paint();
        let mut excerpts = Vec::new();
        find_all(harness.root_id(), "draft-excerpt", &mut excerpts);
        assert_eq!(excerpts.len(), 1);
        let text = excerpts[0].debug_name();
        assert!(text.contains("second"), "{text}");
    }

    #[test]
    fn a_reply_under_an_outdated_thread_names_no_commit() {
        let root = TestRoot::new();
        let state = review_state();
        // The thread's lines are at an older commit, as GitHub reports an
        // outdated thread; the reply posts under its root and carries none.
        state.comment_threads.set(Arc::new(vec![draft_thread(
            "gh-31",
            "0ld5ha1234",
            Some(31),
            "A reply",
        )]));
        open(state, None);
        let view = control(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();
        let mut notes = Vec::new();
        find_all(harness.root_id(), "draft-written-against", &mut notes);
        assert!(notes.is_empty());
        assert!(find(harness.root_id(), "Label: \"1 reply\"").is_some());
    }

    #[test]
    fn stale_offline_and_closed_pull_requests_are_not_available() {
        let state = review_state();
        assert!(is_available(state));

        state.offline.set(true);
        assert!(!is_available(state));
        state.offline.set(false);

        state.stale_head.set(Some("new-head".into()));
        assert!(!is_available(state));
        state.stale_head.set(None);

        state.pull_request.update(|pull| {
            Arc::make_mut(pull.as_mut().unwrap()).state = "closed".into();
        });
        assert!(!is_available(state));
    }

    #[test]
    fn review_popovers_are_mutually_exclusive_and_pending_cannot_be_dismissed() {
        let state = review_state();
        state.review_launcher_open.set(true);
        open(state, None);
        assert!(state.submit_review_open.get_untracked());
        assert!(!state.review_launcher_open.get_untracked());

        state.submit_review_pending.set(Some(3));
        cancel(state, false);
        assert!(state.submit_review_open.get_untracked());
        state.submit_review_pending.set(None);

        let backend = Backend::detached();
        crate::review::review_launch::open_launcher(AppState::new(), state, &backend, None);
        assert!(!state.submit_review_open.get_untracked());
        assert!(state.review_launcher_open.get_untracked());
    }

    /// Enter breaks the line and Cmd-Enter submits, through the box's
    /// own keymap, which has a command of its own for Cmd-Enter.
    #[test]
    fn enter_breaks_the_line_and_cmd_enter_submits() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        state.submit_review_draft.set("Looks good".into());
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(
            state.submit_review_draft.get(),
            "\nLooks good",
            "Enter is a new line at the caret, which starts at the top"
        );
        assert!(commands.try_recv().is_err(), "Enter submitted the review");

        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::SubmitPullRequestReview { body, event, .. }) => {
                assert_eq!(body, "Looks good");
                assert_eq!(event, PullReviewEvent::Comment);
            }
            other => panic!("Cmd-Enter did not submit: {other:?}"),
        }
        assert!(state.submit_review_pending.get().is_some());
    }

    #[test]
    fn keyboard_opens_selects_and_closes_the_popover() {
        let root = TestRoot::new();
        let state = review_state();
        let view = control(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let control = named(harness.root_id(), "submit-review-control").unwrap();
        control.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        assert!(state.submit_review_open.get_untracked());
        assert!(named(harness.root_id(), "submit-review-popover").is_some());
        let approve = named(harness.root_id(), "submit-review-option:approve").unwrap();
        approve.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Space));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::Approve
        );

        state.submit_review_pending.set(Some(7));
        harness.dispatch_event(key(Code::Escape));
        harness.rebuild();
        assert!(
            state.submit_review_open.get_untracked(),
            "Escape cannot cancel a review already sent to GitHub"
        );

        state.submit_review_pending.set(None);
        harness.dispatch_event(key(Code::Escape));
        harness.rebuild();
        assert!(!state.submit_review_open.get_untracked());
    }

    #[test]
    fn outcome_arrows_select_wrap_and_move_focus_until_pending() {
        let root = TestRoot::new();
        let state = review_state();
        let view = control(state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        harness.rebuild();
        harness.paint();

        let control = named(harness.root_id(), "submit-review-control").unwrap();
        control.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        let comment = named(harness.root_id(), "submit-review-option:comment").unwrap();
        let approve = named(harness.root_id(), "submit-review-option:approve").unwrap();
        let request_changes =
            named(harness.root_id(), "submit-review-option:request-changes").unwrap();
        comment.request_focus();
        harness.rebuild();
        assert!(harness.is_focused(comment));

        harness.dispatch_event(key(Code::ArrowRight));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::Approve
        );
        assert!(harness.is_focused(approve));

        harness.dispatch_event(key(Code::ArrowDown));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::RequestChanges
        );
        assert!(harness.is_focused(request_changes));

        harness.dispatch_event(key(Code::ArrowRight));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::Comment,
            "forward navigation wraps to the first option"
        );
        assert!(harness.is_focused(comment));

        harness.dispatch_event(key(Code::ArrowLeft));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::RequestChanges,
            "backward navigation wraps to the last option"
        );
        assert!(harness.is_focused(request_changes));

        harness.dispatch_event(key(Code::ArrowUp));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::Approve
        );
        assert!(harness.is_focused(approve));

        state.submit_review_pending.set(Some(7));
        harness.rebuild();
        harness.dispatch_event(key(Code::ArrowDown));
        harness.rebuild();
        assert_eq!(
            state.submit_review_event.get_untracked(),
            PullReviewEvent::Approve,
            "pending review options ignore keyboard selection"
        );
    }
}
