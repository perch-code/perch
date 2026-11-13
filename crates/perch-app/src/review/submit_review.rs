//! The reader's review for GitHub, under the agents' in the bar's Review
//! popover (`review_menu`).
//!
//! Here "review" is GitHub's COMMENT / APPROVE / REQUEST_CHANGES mutation,
//! and the section mirrors GitHub's finish-review form. The drafts written
//! on lines, the agents' findings among them, are listed above the summary
//! and go with it; the backend reads them from the cache, so nothing here
//! is passed on submit.

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use perch_github::PullReviewEvent;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{drafts_summary, Draft, ReviewState};
use crate::review::review_menu;
use crate::review::timeline::short_sha;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{
    action_button, clickable, editor_box, icon_button, prose_editor, sidebar_glyph, text_button,
};

/// How much of a draft's text its row shows.
const EXCERPT_CHARS: usize = 60;

/// How tall a draft's row is.
const DRAFT_ROW_HEIGHT: f64 = 26.0;

/// How many rows the drafts' list keeps in view in a short window.
const DRAFT_ROWS_MIN: usize = 3;

/// Whether a verdict can go to GitHub now: a current pull request, open,
/// online, and not a historical snapshot. Branches and issues have none.
pub(crate) fn is_available(state: ReviewState) -> bool {
    state.ready.get()
        && !state.offline.get()
        && state.stale_head.with(Option::is_none)
        && state
            .item
            .with(|item| item.as_ref().is_none_or(|item| item.divergence().is_none()))
        && state.historical_review.with(Option::is_none)
        && state
            .pull_request
            .with(|pull| pull.as_ref().is_some_and(|pull| pull.is_open()))
}

/// Whether the section is drawn: live, or holding drafts for a pull
/// request that is still open, so the reader can see they are kept
/// through a head move or a spell offline.
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

/// The drafts, the summary, the verdict and Submit, under a rule. While
/// drafts are held for a pull request that cannot take them now, it says
/// why and submits nothing.
pub(crate) fn section(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let submit_backend = backend.clone();
    let submit: Rc<dyn Fn()> = Rc::new(move || submit(state, &submit_backend));
    let editor = review_editor(app, state, backend.clone(), submit.clone());
    let back_backend = backend.clone();
    let note = Memo::new(move |_| unavailable_note(state));
    let drafts = drafts_list(state, backend);
    let message = |text: Option<String>, debug: &'static str| match text {
        Some(text) => Label::derived(move || text.clone())
            .style(|s| {
                s.width_full()
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::closed())
                    .text_wrap()
            })
            .debug_name(debug)
            .into_any(),
        None => Empty::new().into_any(),
    };

    Stack::vertical((
        Label::derived(|| "Your review".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
        }),
        dyn_container(
            move || note.get(),
            move |note| match note {
                Some((note, snapshot)) => Stack::vertical((
                    Label::derived(move || note.clone())
                        .style(|s| {
                            s.width_full()
                                .font_size(theme::TEXT_META)
                                .color(theme::fg_subtle())
                                .text_wrap()
                        })
                        .debug_name("submit-review-held"),
                    if snapshot {
                        crate::review::summary::back_to_current(app, state, back_backend.clone())
                            .style(|s| {
                                s.border(1.0)
                                    .border_color(theme::border())
                                    .padding_horiz(8.0)
                                    .height(24.0)
                            })
                            .into_any()
                    } else {
                        Empty::new().into_any()
                    },
                ))
                .style(|s| s.flex_col().width_full().items_start().gap(8.0))
                .into_any(),
                None => Empty::new().into_any(),
            },
        ),
        drafts,
        editor,
        outcome_group(state),
        dyn_container(
            move || state.submit_review_outcome.get(),
            move |outcome| message(outcome, "submit-review-outcome"),
        ),
        dyn_container(
            move || state.submit_review_error.get(),
            move |error| message(error, "submit-review-error"),
        ),
        Stack::horizontal((
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            action_button(
                || "Cancel".to_string(),
                "submit-review-cancel",
                false,
                move || state.submit_review_pending.with(Option::is_none),
                move || review_menu::cancel(state, true),
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
    .style(|s| {
        s.flex_col()
            .width_full()
            .min_height(0.0)
            .flex_shrink(1.0_f32)
            .gap(12.0)
            .padding_top(14.0)
            .border_top(1.0)
            .border_color(theme::border_muted())
    })
    .debug_name("submit-review")
}

/// The section folded to a line while the agents' settings or one of
/// their reviews are open over it: what it holds, and Open to go back to
/// it, which folds them.
pub(crate) fn folded(state: ReviewState) -> impl IntoView {
    let drafts = Memo::new(move |_| state.drafts());
    Stack::horizontal((
        Label::derived(|| "Your review".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || {
            drafts.with(|d| {
                if d.is_empty() {
                    "No drafts".to_string()
                } else {
                    drafts_summary(d)
                }
            })
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
        }),
        action_button(
            || "Open".to_string(),
            "submit-review-unfold",
            false,
            || true,
            move || {
                floem::reactive::Effect::batch(|| {
                    state.review_agents_expanded.set(false);
                    state.review_expanded.set(None);
                })
            },
        ),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .gap(8.0)
            .padding_top(14.0)
            .border_top(1.0)
            .border_color(theme::border_muted())
    })
    .debug_name("submit-review-folded")
}

/// Why drafts held for an open pull request cannot go now, and whether
/// the way out is Back to current, off a review's snapshot.
fn unavailable_note(state: ReviewState) -> Option<(String, bool)> {
    if is_available(state) {
        None
    } else if state.offline.get() {
        Some((
            "Offline. The drafts are kept until GitHub can be reached.".into(),
            false,
        ))
    } else if let Some(number) = state
        .historical_review
        .with(|review| review.as_ref().map(|r| r.group.number))
    {
        Some((
            format!(
                "Review #{number}'s snapshot is read-only. Go back to the current \
                 pull request to submit; the drafts are kept."
            ),
            true,
        ))
    } else {
        Some((
            "The pull request has new commits. Update it to submit; the drafts are kept.".into(),
            false,
        ))
    }
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
    // open gets a new row, with the new text. The rows are the one part of
    // the popover that gives: past what the window has room for, they
    // scroll, so the summary and Submit stay on screen.
    let rows = dyn_stack(
        move || drafts.get(),
        |draft| (draft.message_id.clone(), draft.body.clone()),
        move |draft| draft_row(state, rows_backend.clone(), draft, head),
    )
    .style(|s| s.flex_col().width_full())
    .scroll()
    .debug_name("submit-review-draft-list")
    .style(move |s| {
        let least = drafts.with(Vec::len).min(DRAFT_ROWS_MIN) as f64 * DRAFT_ROW_HEIGHT;
        s.width_full()
            .min_height(least)
            .flex_shrink(1.0_f32)
            .overflow_x(floem::taffy::Overflow::Hidden)
    });

    let discard_backend = backend.clone();
    let discard_all = move || {
        if state.submit_review_pending.with_untracked(Option::is_some) {
            return;
        }
        if let Some(item_id) = state
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        {
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
                            .color(theme::fg())
                            .selectable(false)
                    }),
                    Empty::new().style(|s| s.flex_grow(1.0_f32)),
                    text_button(|| "Discard".to_string(), SidebarIcon::Trash, discard_all)
                        .debug_name("discard-all-confirm"),
                    text_button(
                        || "Cancel".to_string(),
                        SidebarIcon::Close,
                        move || state.submit_review_confirm_discard.set(false),
                    ),
                ))
                .style(|s| {
                    s.items_center()
                        .width_full()
                        .gap(6.0)
                        .padding(8.0)
                        .background(theme::notice_bg())
                        .border(1.0)
                        .border_color(theme::notice_border())
                        .border_radius(theme::RADIUS)
                })
                .into_any()
            } else {
                Stack::horizontal((
                    Label::derived(move || drafts.with(|d| drafts_summary(d)))
                        .debug_name("drafts-summary")
                        .style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::fg_muted())
                                .selectable(false)
                        }),
                    Empty::new().style(|s| s.flex_grow(1.0_f32)),
                    Container::new(Label::derived(|| "Discard all".to_string()).style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .color(theme::fg_muted())
                            .selectable(false)
                            .pointer_events_none()
                    }))
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
                            .hover(|s| s.background(theme::canvas_hover()))
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
                .min_height(0.0)
                .flex_shrink(1.0_f32)
                .gap(4.0)
                .apply_if(drafts.with(Vec::is_empty), |s| s.hide())
        })
}

/// One draft: where it goes, in the code font, `written against` its
/// commit when that is not the head and the draft opens a thread there
/// (a reply carries no commit), the start of its text, and a delete while
/// the pointer is over it. Where it goes is a link: it shuts the popover
/// and opens Code on the draft's first line, or Conversation on its card
/// when the thread is outdated and so has none in the code.
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
    let lines = draft
        .anchor
        .as_ref()
        .map(|a| (a.path.clone(), a.start_line.min(a.line) as usize));
    let linked = lines.is_some();
    let message_id = draft.message_id.clone();
    let thread_id = draft.thread_id.clone();
    let delete = move || {
        if state.submit_review_pending.with_untracked(Option::is_some) {
            return;
        }
        if let Some(item_id) = state
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        {
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
            sidebar_glyph(SidebarIcon::Reply, theme::fg_muted)
                .style(move |s| {
                    s.size(12.0, 12.0)
                        .margin_right(4.0)
                        .flex_shrink(0.0_f32)
                        .apply_if(!reply, |s| s.hide())
                })
                .debug_name("draft-reply-glyph"),
            Label::derived(move || location.clone())
                .on_event_stop(el::Click, move |_, _| {
                    let Some((path, line)) = &lines else { return };
                    review_menu::close(state, false);
                    // An outdated thread has no card in the code, and its
                    // line numbers are an older commit's: its card on
                    // Conversation is where the draft is. Read at the
                    // click, since a thread turns outdated under a row.
                    let outdated = state.comment_threads.with_untracked(|threads| {
                        threads.iter().any(|t| t.id == thread_id && t.outdated)
                    });
                    if outdated {
                        crate::review::notified::go_to(
                            state,
                            crate::review::doc::CardId::Thread(thread_id.clone()),
                        );
                    } else {
                        state.go_to_file(path, Some((line.saturating_sub(1), 0)));
                    }
                })
                .debug_name("draft-location")
                .style(move |s| {
                    s.font_size(theme::TEXT_BADGE)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::fg())
                        .flex_shrink(0.0_f32)
                        .selectable(false)
                        .apply_if(linked, |s| clickable(s).hover(|s| s.color(theme::github())))
                }),
            dyn_container(written_against, |note| match note {
                Some(note) => Label::derived(move || note.clone())
                    .debug_name("draft-written-against")
                    .style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .color(theme::fg_muted())
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
                        .color(theme::fg_muted())
                        .margin_left(8.0)
                        .min_width(0.0)
                        .flex_grow(1.0_f32)
                        .text_ellipsis()
                        .selectable(false)
                }),
            crate::review::cards::tip_with(
                icon_button(|| SidebarIcon::Trash, theme::fg_subtle, delete)
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
                .min_height(DRAFT_ROW_HEIGHT)
                .padding_horiz(6.0)
                .border_radius(theme::RADIUS)
                .hover(|s| s.background(theme::canvas_subtle()))
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

fn review_editor(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    submit: Rc<dyn Fn()>,
) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;

    use crate::review::threads::cmd_enter;

    let editor = prose_editor(
        state.submit_review_draft.get_untracked(),
        "Leave a review…",
        Some(state.next_token()),
    );
    // `@` offers people and files, as in a composer; the review goes to
    // GitHub, so a file goes in as a code span.
    let mentioner = crate::review::mention::attach(app, state, backend, editor.editor(), |path| {
        format!("`{path}` ")
    });
    let focus_mentioner = mentioner.clone();
    let blur_mentioner = mentioner.clone();
    let key_mentioner = mentioner.clone();
    let key_submit = submit.clone();
    let tabs = crate::review::markdown_toolbar::Tabs::new(editor.editor(), || false);
    let key_tabs = tabs.clone();
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
            state.submit_review_focused.set(true);
            focus_mentioner.focus(true);
        })
        .on_event_cont(el::FocusLost, move |_, _| {
            state.submit_review_focused.set(false);
            blur_mentioner.focus(false);
        })
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                if key_mentioner.key(key) || key_tabs.key(key) {
                    return EventPropagation::Stop;
                }
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    review_menu::cancel(state, true);
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
        .debug_name("submit-review-editor")
        .on_cleanup(move || mentioner.cleanup());

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
    let boxed = editor_box(editor, || (96.0, theme::COMPOSER_GROWN_HEIGHT), true).style(move |s| {
        s.apply_if(state.submit_review_pending.with(Option::is_some), |s| {
            s.opacity(0.6_f32)
        })
    });
    tabs.view(state, boxed, theme::COMPOSER_GROWN_HEIGHT)
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
                        theme::fg()
                    } else {
                        theme::canvas()
                    })
                }))
                .style(move |s| {
                    s.size(17.0, 17.0)
                        .items_center()
                        .justify_center()
                        .border(1.5)
                        .border_color(if selected {
                            theme::fg()
                        } else {
                            theme::fg_subtle()
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
                    .color(theme::fg())
                    .selectable(false)
                    .pointer_events_none()
            }),
            Label::derived(move || outcome_description(event).to_string()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
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
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .set_disabled(!enabled)
            .apply_if(enabled, |s| {
                s.hover(|s| s.background(theme::canvas_subtle()))
            })
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
    if let Some((note, _)) = unavailable_note(state) {
        state.submit_review_error.set(Some(note));
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
    use crate::review::review_menu::{cancel, close, control, open};
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

    /// The next command other than the dashboard load opening Review asks
    /// for.
    fn next_command(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Result<Command, tokio::sync::mpsc::error::TryRecvError> {
        loop {
            // What the popover asks for as it opens: the item's reviews,
            // and its runs, for their reviewers.
            match commands.try_recv() {
                Ok(Command::LoadReviewDashboard { .. } | Command::LoadItemRuns { .. }) => continue,
                other => return other,
            }
        }
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
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull())));
        state.ready.set(true);
        state.submit_review_autofocus.set(false);
        state
    }

    #[test]
    fn only_approval_allows_an_empty_body_without_drafts() {
        assert!(!review_body_is_valid(
            PullReviewEvent::Comment,
            " \n",
            false
        ));
        assert!(review_body_is_valid(
            PullReviewEvent::Comment,
            "Looks good overall",
            false
        ));
        assert!(review_body_is_valid(PullReviewEvent::Approve, "", false));
        assert!(!review_body_is_valid(
            PullReviewEvent::RequestChanges,
            "",
            false
        ));
        assert!(review_body_is_valid(
            PullReviewEvent::RequestChanges,
            "Please cover the failure path",
            false
        ));
        // Drafts on lines are a comment review's content; a request for
        // changes still says what to change.
        assert!(review_body_is_valid(PullReviewEvent::Comment, "", true));
        assert!(!review_body_is_valid(
            PullReviewEvent::RequestChanges,
            "",
            true
        ));
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
            node_id: None,
            can_resolve: false,
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
    fn the_button_counts_the_drafts_and_keeps_them_while_they_cannot_go() {
        let root = TestRoot::new();
        let state = review_state();
        state.comment_threads.set(Arc::new(vec![
            draft_thread("t1", "head-42", None, "one"),
            draft_thread("t2", "head-42", Some(31), "two"),
        ]));
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 700.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Review \u{b7} 2").is_some());

        // The head moved: the count stays, and the form under the agents
        // lists the drafts, says why they cannot go, and submits nothing.
        state.stale_head.set(Some("new-head".into()));
        harness.rebuild();
        harness.paint();
        assert!(is_shown(state));
        assert!(find(harness.root_id(), "Review \u{b7} 2").is_some());
        click(&mut harness, "review-control");
        assert!(state.review_open.get());
        let held = named(harness.root_id(), "submit-review-held").expect("no word on why");
        assert!(shown(&harness, held));
        assert!(!can_submit(state));

        // Without drafts a stale head leaves the agents alone.
        state.comment_threads.set(Arc::new(Vec::new()));
        harness.rebuild();
        harness.paint();
        assert!(!is_shown(state));
        assert!(named(harness.root_id(), "submit-review").is_none());
        assert!(named(harness.root_id(), "review-agents").is_some());
        assert!(find(harness.root_id(), "Label: \"Review\"").is_some());

        state.stale_head.set(None);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "submit-review").is_some());
        assert!(named(harness.root_id(), "submit-review-held").is_none());
    }

    #[test]
    fn a_worktree_at_another_commit_than_its_pull_request_cannot_submit() {
        let _root = TestRoot::new();
        let state = review_state();
        let with_pr_head = |head: &str| {
            let mut item = state.item.get_untracked().unwrap().as_ref().clone();
            let mut pr = pull();
            pr.head_sha = head.into();
            item.subject = Some(perch_github::ItemSubject::PullRequest(pr));
            state.item.set(Some(Arc::new(item)));
        };
        with_pr_head("head-42");
        assert!(is_available(state));
        // The reader's worktree has a commit the pull request hasn't.
        with_pr_head("head-41");
        assert!(!is_available(state));
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
        let view = control(AppState::new(), state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
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

        // Delete is drawn while the pointer is over the row, stays drawn
        // with the pointer on it, and deletes that draft. The pointer goes
        // onto Delete before the click, as a hand's would: that crosses
        // the edge of the box the form hangs out of, where floem once
        // handed the row its Leave last (see `track_hover`). Had Delete
        // hidden, the press and the release would land on different views,
        // which floem counts as a click only within 100ms of the press.
        let delete = find(rows[1], "delete-draft").unwrap();
        assert!(!shown(&harness, delete), "hidden until hovered");
        let center = locations[1].get_visual_rect_no_clip().center();
        harness.pointer_move(center.x, center.y);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, delete));
        let at = delete.get_visual_rect_no_clip().center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, delete), "the pointer on it keeps it");
        harness.click(at.x, at.y);
        match next_command(&mut commands) {
            Ok(Command::DeleteDraft {
                item_id,
                message_id,
            }) => {
                assert_eq!(item_id, "item-42");
                assert_eq!(message_id, "t2-d1");
            }
            other => panic!("delete did not reach the backend: {other:?}"),
        }
    }

    /// Enough drafts to outgrow the window scroll in their list, and
    /// Submit stays in the window; a few take only their own height.
    #[test]
    fn many_drafts_scroll_and_leave_submit_in_the_window() {
        let root = TestRoot::new();
        let state = review_state();
        let threads = (0..30)
            .map(|i| draft_thread(&format!("t{i}"), "head-42", None, "A comment"))
            .collect();
        state.comment_threads.set(Arc::new(threads));
        open(state, None);
        let app = AppState::new();
        app.window_height.set(700.0);
        let view = control(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 700.0);
        harness.rebuild();
        harness.paint();

        let submit = named(harness.root_id(), "submit-review-action").expect("no Submit");
        let at = submit.get_visual_rect_no_clip();
        assert!(at.height() > 0.0);
        assert!(at.y1 <= 700.0, "Submit ends at {} in a 700 window", at.y1);
        let rows = named(harness.root_id(), "submit-review-draft-list").expect("no rows");
        let height = harness.get_layout_rect(rows).height();
        assert!(
            height >= 3.0 * DRAFT_ROW_HEIGHT,
            "the list keeps three rows"
        );
        assert!(height < 30.0 * DRAFT_ROW_HEIGHT, "the list scrolls");

        state
            .comment_threads
            .set(Arc::new(vec![draft_thread("t1", "head-42", None, "one")]));
        harness.rebuild();
        harness.paint();
        let rows = named(harness.root_id(), "submit-review-draft-list").expect("no rows");
        assert_eq!(harness.get_layout_rect(rows).height(), DRAFT_ROW_HEIGHT);
    }

    #[test]
    fn a_drafts_location_shuts_the_popover_and_goes_to_its_line() {
        let root = TestRoot::new();
        let state = review_state();
        state
            .comment_threads
            .set(Arc::new(vec![draft_thread("t1", "head-42", None, "one")]));
        open(state, None);
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        click(&mut harness, "draft-location");
        assert!(!state.review_open.get());
        assert_eq!(state.tab.get(), crate::review::model::ItemTab::Code);
        assert_eq!(state.pending_jump.get(), Some(("src/x.rs".into(), 40, 0)));
    }

    /// An outdated draft's lines are an older commit's, and it has no card
    /// in the code: its row goes to the card on Conversation.
    #[test]
    fn an_outdated_drafts_location_goes_to_its_card_on_conversation() {
        let root = TestRoot::new();
        let state = review_state();
        state.tab.set(crate::review::model::ItemTab::Code);
        let mut thread = draft_thread("t1", "0ld5ha1", None, "one");
        thread.outdated = true;
        state.comment_threads.set(Arc::new(vec![thread]));
        open(state, None);
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        click(&mut harness, "draft-location");
        assert!(!state.review_open.get());
        assert_eq!(state.tab.get(), crate::review::model::ItemTab::Conversation);
        assert_eq!(
            state.pending_focus_card.get(),
            Some(crate::review::doc::CardId::Thread("t1".into()))
        );
        assert_eq!(state.pending_jump.get(), None);
    }

    #[test]
    fn a_comment_review_submits_with_drafts_and_no_summary() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        state
            .comment_threads
            .set(Arc::new(vec![draft_thread("t1", "head-42", None, "one")]));
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(AppState::new(), state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();
        assert!(can_submit(state));

        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match next_command(&mut commands) {
            Ok(Command::SubmitPullRequestReview { body, event, .. }) => {
                assert_eq!(
                    body, "",
                    "the drafts are the content; the backend reads them"
                );
                assert_eq!(event, PullReviewEvent::Comment);
            }
            other => panic!("Cmd-Enter did not submit: {other:?}"),
        }

        // A request for changes still needs its summary.
        state.submit_review_pending.set(None);
        state
            .submit_review_event
            .set(PullReviewEvent::RequestChanges);
        assert!(!can_submit(state));
    }

    #[test]
    fn a_snapshot_says_so_and_offers_the_way_back() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        state
            .comment_threads
            .set(Arc::new(vec![draft_thread("t1", "head-42", None, "one")]));
        let created_at = "2026-09-19T12:00:00Z".parse().unwrap();
        state
            .historical_review
            .set(Some(crate::review::model::ReviewGroupSummary {
                group: perch_github::ReviewGroup {
                    id: "group".into(),
                    item_id: "item-42".into(),
                    snapshot_id: "snapshot".into(),
                    number: 1,
                    status: perch_github::ReviewGroupStatus::Completed,
                    new_findings: 0,
                    still_present: 0,
                    verified_fixed: 0,
                    not_verified: 0,
                    created_at,
                    started_at: Some(created_at),
                    ended_at: Some(created_at),
                    instructions: None,
                },
                snapshot: perch_github::ReviewSnapshot {
                    id: "snapshot".into(),
                    item_id: "item-42".into(),
                    repository: "acme/repo".into(),
                    branch_label: Some("feature".into()),
                    requested_base_ref: "main".into(),
                    base_tip_sha: "base".into(),
                    merge_base_sha: "base".into(),
                    head_sha: "head-42".into(),
                    dirty_edits_excluded: false,
                    retention_key: "snapshot".into(),
                    retained_ref: None,
                    manifest_path: None,
                    diff_path: None,
                    created_at,
                    worktree_parent_sha: None,
                },
                attempts: Vec::new(),
            }));
        open(state, None);
        let (backend, mut commands) = Backend::recording();
        let view = control(AppState::new(), state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();
        assert!(!can_submit(state));
        let held = named(harness.root_id(), "submit-review-held").expect("no word on why");
        assert!(shown(&harness, held));
        assert!(find(harness.root_id(), "snapshot is read-only").is_some());
        assert!(find(harness.root_id(), "new commits").is_none());

        // Cmd-Enter says the same, by the button, and sends nothing.
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        assert!(state
            .submit_review_error
            .get()
            .is_some_and(|e| e.contains("snapshot is read-only")));
        assert!(matches!(
            next_command(&mut commands),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        let back = find(harness.root_id(), "back to current").expect("no way back");
        let center = back.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        harness.rebuild();
        match next_command(&mut commands) {
            Ok(Command::OpenItem { id, .. }) => assert_eq!(id, "item-42"),
            other => panic!("Back to current did not open the item: {other:?}"),
        }
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
        let view = control(AppState::new(), state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        click(&mut harness, "discard-all");
        assert!(state.submit_review_confirm_discard.get());
        assert!(find(harness.root_id(), "Discard all drafts?").is_some());
        assert!(
            next_command(&mut commands).is_err(),
            "asking discards nothing"
        );

        click(&mut harness, "discard-all-confirm");
        assert!(!state.submit_review_confirm_discard.get());
        match next_command(&mut commands) {
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
        let view = control(AppState::new(), state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        assert!(next_command(&mut commands).is_ok());
        let attempt = state.submit_review_pending.get_untracked().unwrap();

        let accepted = perch_github::Review {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            id: 55,
            user: "reviewer".into(),
            user_avatar_url: String::new(),
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

        assert!(state.review_open.get_untracked());
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
        state
            .comment_threads
            .set(Arc::new(vec![draft_thread("t1", "head-42", None, "first")]));
        open(state, None);
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();
        let mut excerpts = Vec::new();
        find_all(harness.root_id(), "draft-excerpt", &mut excerpts);
        assert_eq!(excerpts.len(), 1);
        assert!(excerpts[0].debug_name().contains("first"));

        // Saved again from its card, same id, new text.
        state.comment_threads.set(Arc::new(vec![draft_thread(
            "t1", "head-42", None, "second",
        )]));
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
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
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
    fn a_review_sent_to_github_cannot_be_dismissed() {
        let state = review_state();
        open(state, None);
        state.submit_review_pending.set(Some(3));
        cancel(state, false);
        assert!(state.review_open.get_untracked());
        state.submit_review_pending.set(None);
        cancel(state, false);
        assert!(!state.review_open.get_untracked());
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
        let view = control(AppState::new(), state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(
            state.submit_review_draft.get(),
            "\nLooks good",
            "Enter is a new line at the caret, which starts at the top"
        );
        assert!(
            next_command(&mut commands).is_err(),
            "Enter submitted the review"
        );

        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match next_command(&mut commands) {
            Ok(Command::SubmitPullRequestReview { body, event, .. }) => {
                assert_eq!(body, "Looks good");
                assert_eq!(event, PullReviewEvent::Comment);
            }
            other => panic!("Cmd-Enter did not submit: {other:?}"),
        }
        assert!(state.submit_review_pending.get().is_some());
    }

    /// The review box has the composer's styles and Preview; Escape from
    /// Preview goes back to Write and leaves the popover up.
    #[test]
    fn the_review_box_takes_styles_and_escape_leaves_preview_first() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        state.submit_review_draft.set("Looks good".into());
        open(state, None);
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();
        let cmd_shift = |code, c: &str| {
            floem::event::Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Character(c.into()),
                code,
                location: Default::default(),
                modifiers: crate::ui::OS_MOD | Modifiers::SHIFT,
                repeat: false,
                is_composing: false,
            })
        };

        harness.dispatch_event(cmd_shift(Code::Digit8, "8"));
        harness.rebuild();
        assert_eq!(state.submit_review_draft.get(), "- Looks good");

        harness.dispatch_event(cmd_shift(Code::KeyP, "p"));
        harness.rebuild();
        harness.paint();
        let shown = |harness: &HeadlessHarness, name: &str| {
            named(harness.root_id(), name)
                .is_some_and(|id| harness.get_layout_rect(id).height() > 0.0)
        };
        assert!(shown(&harness, "composer-preview"));
        harness.dispatch_event(key(Code::Escape));
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "composer-preview"));
        assert!(
            state.review_open.get(),
            "Escape from Preview closed the review"
        );
    }

    #[test]
    fn keyboard_opens_selects_and_closes_the_popover() {
        let root = TestRoot::new();
        let state = review_state();
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        let control = named(harness.root_id(), "review-control").unwrap();
        control.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(Code::Enter));
        harness.rebuild();
        harness.paint();

        assert!(state.review_open.get_untracked());
        assert!(named(harness.root_id(), "review-popover").is_some());
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
            state.review_open.get_untracked(),
            "Escape cannot cancel a review already sent to GitHub"
        );

        state.submit_review_pending.set(None);
        harness.dispatch_event(key(Code::Escape));
        harness.rebuild();
        assert!(!state.review_open.get_untracked());
    }

    #[test]
    fn outcome_arrows_select_wrap_and_move_focus_until_pending() {
        let root = TestRoot::new();
        let state = review_state();
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        let control = named(harness.root_id(), "review-control").unwrap();
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

    /// A summary longer than its box stays scrolled to the caret as it is
    /// typed into, as the question's box does: floem forgot the rows of a
    /// wrapped line with every key, and the box jumped to the top and back.
    #[test]
    fn a_long_summary_stays_scrolled_to_the_caret_as_it_is_typed() {
        let root = TestRoot::new();
        let state = review_state();
        state.submit_review_autofocus.set(true);
        let summary = (1..=60)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        state.submit_review_draft.set(summary);
        open(state, None);
        let view = control(AppState::new(), state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 800.0);
        harness.rebuild();
        harness.paint();

        let lines = find(harness.root_id(), "submit-review-editor")
            .and_then(|editor| find(editor, "Editor View"))
            .expect("the summary's lines");
        let scrolled = || {
            let shown = lines.get_visual_rect();
            let whole = lines.get_visual_rect_no_clip();
            (shown.y0 - whole.y0, whole.height() - shown.height())
        };
        let key = |key: Key, code: Code, modifiers: Modifiers| {
            floem::event::Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key,
                code,
                location: Default::default(),
                modifiers,
                repeat: false,
                is_composing: false,
            })
        };

        // The caret starts at the top; Cmd-Down takes it to the end, and
        // Ctrl-End off macOS.
        let (to_end, to_end_code, to_end_modifiers) = if cfg!(target_os = "macos") {
            (NamedKey::ArrowDown, Code::ArrowDown, Modifiers::META)
        } else {
            (NamedKey::End, Code::End, Modifiers::CONTROL)
        };
        harness.dispatch_event(key(Key::Named(to_end), to_end_code, to_end_modifiers));
        harness.paint();
        let (at, end) = scrolled();
        assert!(end > 0.0, "the summary is longer than its box");
        assert_eq!(at, end, "the box went down with the caret");

        for ch in "abc".chars() {
            harness.dispatch_event(key(
                Key::Character(ch.to_string()),
                Code::KeyX,
                Modifiers::empty(),
            ));
            harness.paint();
            assert_eq!(scrolled(), (end, end), "the box stayed down");
        }
    }
}
