//! Submit a GitHub pull-request review from the title bar.
//!
//! This is deliberately separate from `review_launch`, which starts native
//! agent reviews of a branch. Here "review" is GitHub's COMMENT / APPROVE /
//! REQUEST_CHANGES mutation and the compact surface mirrors GitHub's finish-
//! review form without taking over the whole window.

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::views::{dyn_container, text_editor, Container, Decorators, Empty, Label, Stack};
use perch_github::PullReviewEvent;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::theme;
use crate::ui::widgets::{clickable, sidebar_glyph};

/// The upper-right action. It only exists for a live, current pull request;
/// branches, issues, stale/offline checkouts and historical snapshots cannot
/// safely submit a remote verdict.
pub fn control(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Effect::new(move |_| {
        if !is_available(state) && state.submit_review_open.get() {
            close(state, false);
        }
    });

    dyn_container(
        move || is_available(state),
        move |available| {
            if available {
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

fn available_control(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let inner = Stack::horizontal((
        Label::derived(move || {
            if state.submit_review_pending.with(Option::is_some) {
                "Submitting…".to_string()
            } else {
                "Submit review".to_string()
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
        if state.submit_review_pending.with_untracked(Option::is_some) {
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
            clickable(s)
                .height(30.0)
                .padding_horiz(12.0)
                .items_center()
                .background(theme::FG)
                .border_radius(9.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
                .set_disabled(pending)
                .apply_if(pending, |s| {
                    s.opacity(0.75_f32).cursor(CursorStyle::Default)
                })
                .apply_if(!pending, |s| s.hover(|s| s.background(theme::ACTION_HOVER)))
        })
        .debug_name("submit-review-control");

    let popover_backend = backend.clone();
    Stack::new((
        button,
        dyn_container(
            move || state.submit_review_open.get(),
            move |open| {
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
    if restore_focus {
        if let Some(opener) = state.submit_review_opener.get_untracked() {
            opener.request_focus();
        }
    }
    state.submit_review_opener.set(None);
}

fn popover(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let submit: Rc<dyn Fn()> = Rc::new(move || submit(state, &backend));
    let editor = review_editor(state, submit.clone());

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
        editor,
        outcome_group(state),
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
            let pending = state.submit_review_pending.with(Option::is_some);
            s.width_full()
                .min_height(96.0)
                .max_height(220.0)
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .padding(10.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS_LG)
                .focus(|s| s.border_color(theme::ACCENT))
                .set_disabled(pending)
                .apply_if(pending, |s| s.opacity(0.6_f32))
        })
        .debug_name("submit-review-editor");

    let inner = editor.editor().editor_view_id;
    let asked = Rc::new(Cell::new(false));
    editor.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if asked.replace(true) {
            return;
        }
        if state.submit_review_autofocus.get_untracked() {
            if let Some(id) = inner.get_untracked() {
                id.request_focus();
            }
        }
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
    )
}

fn review_body_is_valid(event: PullReviewEvent, body: &str) -> bool {
    matches!(event, PullReviewEvent::Approve) || !body.trim().is_empty()
}

fn submit(state: ReviewState, backend: &Backend) {
    if state.submit_review_pending.with_untracked(Option::is_some) {
        return;
    }
    let event = state.submit_review_event.get_untracked();
    let body = state.submit_review_draft.get_untracked();
    if !review_body_is_valid(event, &body) {
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
    use perch_github::{Item, ItemCheckout, PullRequest};
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
    fn only_approval_allows_an_empty_body() {
        assert!(!review_body_is_valid(PullReviewEvent::Comment, " \n"));
        assert!(review_body_is_valid(
            PullReviewEvent::Comment,
            "Looks good overall"
        ));
        assert!(review_body_is_valid(PullReviewEvent::Approve, ""));
        assert!(!review_body_is_valid(PullReviewEvent::RequestChanges, ""));
        assert!(review_body_is_valid(
            PullReviewEvent::RequestChanges,
            "Please cover the failure path"
        ));
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
