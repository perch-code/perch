//! Thread views: questions anchored to lines, and the agent's answers.
//!
//! A thread is a comment thread, not a chat: short questions, short
//! answers, tied to lines. Anchored threads are cards in the code, and the
//! complete item history is on Conversation. A composer sits in the code
//! under selected lines or on Conversation for an item-level question; a
//! follow-up's composer sits under its thread.
//!
//! Both faces are drawn as github.com draws a thread: a box with a
//! header, each message laid out as a comment under its author's face and
//! byline, the Follow up box where GitHub has Reply, and a foot with
//! Resolve. The message, the run's steps and footer, the foot and the
//! composer are one set of views here, taking a [`ThreadFace`] for the
//! sizes that differ, so Conversation and inline cards cannot drift apart.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{BorderBottomLeftRadius, BorderBottomRightRadius, CursorStyle};
use floem::views::{
    dyn_container, dyn_stack, text_editor, Container, Decorators, Empty, Label, Stack,
};
use perch_github::{Author, CommentMessage, CommentThread, RunRecord, RunStatus, ThreadOrigin};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards;
use crate::review::doc::CardId;
use crate::review::model::{AgentChoice, Composer, Destination, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{
    clickable, editor_box, icon_button, lay_out_every_line, name_label, sidebar_glyph, text_button,
    Name,
};

/// A model alias as the screen shows it: "Sonnet" for `sonnet`. A full id
/// such as `claude-sonnet-5` is shown as typed.
pub fn model_label(model: &str) -> String {
    if model.contains('-') {
        return model.to_string();
    }
    let mut chars = model.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Whether the command a box's keymap resolved a key to is Cmd-Enter,
/// which sends. floem binds Cmd-Enter to `NewLineBelow`, a command of its
/// own, not to Enter's `InsertNewLine` with the modifier held; a box for
/// a question has no use for a line opened under the caret, so either
/// command with Cmd held is Send.
pub fn cmd_enter(cmd: &floem::views::editor::text_document::PreCommand<'_>) -> bool {
    use floem::views::editor::command::Command;
    use floem::views::editor::core::command::EditCommand;

    matches!(
        cmd.cmd,
        Command::Edit(EditCommand::InsertNewLine | EditCommand::NewLineBelow)
    ) && cmd.mods.contains(Modifiers::META)
}

/// What a fresh composer starts on: the last thing sent, else config.
pub fn defaults(app: AppState) -> AgentChoice {
    app.agent.with(default_choice)
}

/// Config's model and effort, unless that model is not on offer, which
/// happens when its agent is not installed: then the first model that is,
/// at the nearest effort it takes. With nothing installed the config's
/// own choice stands, and the run says what is missing.
fn default_choice(agent: &perch_github::AgentConfig) -> AgentChoice {
    let effort = perch_agent::Effort::parse(&agent.effort).unwrap_or_default();
    let offered = agent
        .models
        .iter()
        .chain(agent.codex_models())
        .any(|m| *m == agent.model);
    let model = if offered {
        agent.model.clone()
    } else {
        model_groups(agent)
            .into_iter()
            .flatten()
            .next()
            .map(|(_, m)| m)
            .unwrap_or_else(|| agent.model.clone())
    };
    (
        crate::ask::backend_for(agent, &model),
        model.clone(),
        nearest_effort(agent, &model, effort),
    )
}

/// Where a thread is drawn: as a card on Conversation, or inline in the
/// code under the lines it is about. The views are the same; the face picks
/// the room they take and the surface they sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadFace {
    Card,
    Inline,
}

impl ThreadFace {
    /// The face's slot in `ReviewState::last_destination`.
    pub fn index(self) -> usize {
        match self {
            ThreadFace::Inline => 0,
            ThreadFace::Card => 1,
        }
    }

    /// The avatar beside a comment.
    fn avatar(self) -> f64 {
        match self {
            ThreadFace::Inline => theme::INLINE_THREAD_AVATAR,
            ThreadFace::Card => theme::THREAD_AVATAR,
        }
    }

    /// The room at a thread's sides, and over its first comment.
    fn inset(self) -> f64 {
        match self {
            ThreadFace::Inline => theme::INLINE_THREAD_INSET,
            ThreadFace::Card => theme::THREAD_INSET,
        }
    }

    /// The room above and below a comment.
    fn comment_space(self) -> f64 {
        match self {
            ThreadFace::Inline => theme::INLINE_THREAD_COMMENT_SPACE,
            ThreadFace::Card => theme::THREAD_COMMENT_SPACE,
        }
    }
}

/// Where a message is in its thread: the first, which in the code sits
/// flush at the card's left, and the last, under which the line down the
/// avatars stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Place {
    pub first: bool,
    pub last: bool,
}

impl Place {
    /// Where the message with `id` is in `thread`; one the thread does not
    /// hold is taken for a message on its own.
    fn of(thread: &CommentThread, id: &str) -> Place {
        match thread.messages.iter().position(|m| m.id == id) {
            Some(i) => Place {
                first: i == 0,
                last: i + 1 == thread.messages.len(),
            },
            None => Place {
                first: true,
                last: true,
            },
        }
    }
}

/// Whether a message is marked as Perch's alone. In a thread that has
/// something on GitHub, a message that does not, the reader's question to
/// an agent and the agent's answer, is labelled Perch only in its byline,
/// so the reader never wonders what a colleague can see; the runs of them
/// are told apart from their first message on. Everything else, and every
/// message of a wholly private thread, is flush and unlabelled: the label
/// is a difference, not a decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    Flush,
    PrivateStart,
    PrivateContinue,
}

impl Lane {
    /// Whether each message of `thread` is marked Perch only, in order.
    pub fn of(thread: &CommentThread) -> Vec<Lane> {
        let mixed = thread.has_github_message();
        let mut after_private = false;
        thread
            .messages
            .iter()
            .map(|m| {
                let private = mixed && m.is_private();
                let lane = match (private, after_private) {
                    (false, _) => Lane::Flush,
                    (true, false) => Lane::PrivateStart,
                    (true, true) => Lane::PrivateContinue,
                };
                after_private = private;
                lane
            })
            .collect()
    }
}

// ---------------------------------------------------------------------
// Composer
// ---------------------------------------------------------------------

/// The box a question is typed into, with where it is anchored above and
/// the model, the effort and Send below. In the column it sits in a
/// margin of its own; as a card on Conversation it fills its place; in
/// the code it is a card under the lines it is about, placed by the
/// layer that draws the cards. Send closes it; a card in the code leaves
/// its rows for the thread's card to take when the thread arrives.
pub fn composer_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    composer: Composer,
    face: ThreadFace,
) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;
    use floem::views::editor::core::cursor::{Cursor, CursorAffinity, CursorMode};
    use floem::views::editor::core::selection::Selection;

    let anchor_label = match &composer.anchor {
        Some(a) => a.label(),
        None => state
            .item
            .with(|i| i.as_ref().and_then(|i| i.branch().map(str::to_string)))
            .unwrap_or_else(|| "Worktree".to_string()),
    };
    let outdated = composer
        .thread_id
        .as_ref()
        .and_then(|id| {
            state
                .comment_threads
                .with(|t| t.iter().find(|t| &t.id == id).map(|t| t.outdated))
        })
        .unwrap_or(false);
    let follow_up = composer.thread_id.is_some();
    let placeholder = if follow_up {
        "Follow up…".to_string()
    } else if composer.anchor.is_some() {
        "Ask about these lines…".to_string()
    } else {
        format!("Ask about this {}…", subject_word(state))
    };

    // The box starts on what has been typed so far, not on the draft the
    // composer opened with: the view is rebuilt when the tab changes, and
    // the question must not go with it.
    let draft = state.composer_draft.get_untracked();
    let editor = text_editor(draft.clone())
        .placeholder(placeholder)
        .editor_style(|s| s.hide_gutter(true).selection_color(theme::SELECTION_BG));
    // Markdown as it is typed: bold, italics, code and links take their
    // styles in the box, the way they will in the thread.
    let styling_doc = editor.editor().doc();
    let styling_id = state.next_token();
    let editor = editor.styling(crate::review::composer_styling::ComposerStyling::new(
        styling_doc,
        styling_id,
    ));
    // Every line laid out, so a long question does not jump as it is typed.
    lay_out_every_line(editor.editor());
    let doc_editor = editor.editor().clone();
    let read_text = Rc::new(move || doc_editor.doc().text().to_string());
    // `@` offers people and files. A file goes to an agent as `@path`, as
    // Claude Code and Codex take one, and to GitHub as a code span, where
    // `@` would mention someone.
    let mentioner = crate::review::mention::attach(
        app,
        state,
        backend.clone(),
        editor.editor(),
        move |path| {
            let github = state
                .composer
                .with_untracked(|c| c.as_ref().is_some_and(|c| c.destination.is_github()));
            if github {
                format!("`{path}` ")
            } else {
                format!("@{path} ")
            }
        },
    );
    let focus_mentioner = mentioner.clone();
    let blur_mentioner = mentioner.clone();
    let key_mentioner = mentioner.clone();
    // A draft the box starts with, a retried question or a quoted comment,
    // is typed under, not into.
    if !draft.is_empty() {
        let end = editor.editor().doc().text().len();
        editor.editor().cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(end, CursorAffinity::Backward)),
            None,
            None,
        ));
    }

    // Whether this composer's issue comment is on its way: the box and
    // the buttons are still until the answer. Another composer's comment
    // in flight is not this one's concern.
    let opening = composer.opening;
    let posting_here = move || state.posting_from(opening);
    let posting_here_now = move || floem::reactive::Effect::untrack(posting_here);

    let send: Rc<dyn Fn()> = {
        let read_text = read_text.clone();
        let backend = backend.clone();
        Rc::new(move || {
            // Nothing sends twice: the box is still until the answer.
            if posting_here_now() {
                return;
            }
            let question = read_text();
            if question.trim().is_empty() {
                return;
            }
            let Some(composer) = state.composer.get() else {
                return;
            };
            let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
                return;
            };
            // An issue comment posts at once, so it is asked about first:
            // the footer names the pull request, and the next send goes.
            if composer.destination.is_github() && state.posts_issue_comment(&composer) {
                if !state.composer_confirming.get_untracked() {
                    state.composer_confirming.set(true);
                    return;
                }
                let body = question.trim().to_string();
                let token = backend.post_issue_comment(&item_id, &body);
                state.last_destination[face.index()].set(Some(composer.destination.clone()));
                state.start_posting(token, body);
                return;
            }
            match &composer.destination {
                Destination::Agent {
                    backend: agent,
                    model,
                    effort,
                } => {
                    backend.ask(
                        &item_id,
                        composer.thread_id.clone(),
                        composer.anchor.clone(),
                        question.trim(),
                        *agent,
                        model,
                        *effort,
                        sending_mode(app, state, composer.mode),
                    );
                    state
                        .agent_last
                        .set(Some((*agent, model.clone(), *effort)));
                }
                // A draft: written and shown, and nothing asked. The
                // review form posts it.
                Destination::GitHub => backend.save_draft(
                    &item_id,
                    composer.thread_id.clone(),
                    composer.anchor.clone(),
                    question.trim(),
                    composer.editing.clone(),
                ),
            }
            state.last_destination[face.index()].set(Some(composer.destination.clone()));
            state.composer_sent();
        })
    };
    let key_send = send.clone();
    let escape_text = read_text.clone();

    // The box is still while an issue comment is posting; the text stays
    // for the answer to keep or give back.
    let still = editor.editor().read_only;
    Effect::new(move |_| still.set(posting_here()));
    let editor = editor
        // Cmd-Enter sends, before the editor can take it for a line. While
        // the footer is asking whether to post, Enter alone is the yes;
        // while the comment goes, the box is read-only and neither sends.
        .pre_command(move |cmd| {
            if posting_here_now() {
                return CommandExecuted::No;
            }
            if cmd_enter(&cmd) || (plain_enter(&cmd) && state.composer_confirming.get_untracked())
            {
                key_send();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        // The draft, for Escape and for the root key handler, neither of
        // which can reach into the document. A change to the text is a
        // new try, so a refusal under it goes; and there is nothing to
        // ask about posting once the text is gone.
        .update(move |update| {
            if let Some(editor) = update.editor {
                let text = editor.doc().text().to_string();
                let empty = text.trim().is_empty();
                state.composer_draft.set(text);
                if state.composer_error.with_untracked(Option::is_some) {
                    state.composer_error.set(None);
                }
                if empty && state.composer_confirming.get_untracked() {
                    state.composer_confirming.set(false);
                }
            }
        })
        .on_event_cont(el::FocusGained, move |_, _| {
            state.composer_focused.set(true);
            focus_mentioner.focus(true);
        })
        .on_event_cont(el::FocusLost, move |_, _| {
            state.composer_focused.set(false);
            blur_mentioner.focus(false);
        })
        // Escape closes an empty composer, or one editing a draft: the
        // draft's saved words come back as the message row, and whatever
        // was typed over them since is dropped, as the edit was never
        // sent. In the capture phase, because the editor stops every key
        // once it has focus and this has to run first.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                // An `@`'s list has the keys it moves and takes by, and
                // Escape puts it away before anything else.
                if key_mentioner.key(key) {
                    return floem::event::EventPropagation::Stop;
                }
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    // Escape takes the question back first, with the
                    // text intact; while posting there is nothing to do.
                    if posting_here_now() {
                        return floem::event::EventPropagation::Stop;
                    }
                    if state.composer_confirming.get_untracked() {
                        state.composer_confirming.set(false);
                        return floem::event::EventPropagation::Stop;
                    }
                    let editing = state
                        .composer
                        .with_untracked(|c| c.as_ref().is_some_and(|c| c.editing.is_some()));
                    if editing || escape_text().trim().is_empty() {
                        state.close_composer();
                    }
                    return floem::event::EventPropagation::Stop;
                }
                floem::event::EventPropagation::Continue
            },
        )
        .style(|s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
        });
    // Take the keyboard as soon as the composer appears.
    //
    // Two things have to be right for that. Focus is asked of the editor's
    // inner view, the one that is focusable and owns the key handler;
    // asking the wrapper finds no focusable ancestor and does nothing. And
    // it is asked from the first layout pass rather than at construction:
    // a request queued while the view is being built is processed before
    // the view is in the box tree, where it is invisible and dropped.
    //
    // A composer laid in place on Conversation also leaves its box where
    // `a` can find it and scrolls the page to itself: it sits at the end of
    // the conversation, or under a thread, and may have opened off screen.
    let inner = editor.editor().editor_view_id;
    let asked = std::rc::Rc::new(std::cell::Cell::new(false));
    let registered = std::rc::Rc::new(std::cell::Cell::new(None));
    let unregister = registered.clone();
    let editor = editor
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if asked.replace(true) {
                return;
            }
            let Some(id) = inner.get_untracked() else {
                return;
            };
            // The box `a` finds again is Conversation's; a follow-up's box
            // inside an inline card is under its thread, where the reader
            // just clicked.
            if face == ThreadFace::Card {
                registered.set(Some(id));
                state.composer_box.set(Some(id));
                id.scroll_to(None);
            }
            if state.composer_autofocus.get_untracked() {
                id.request_focus();
            }
        })
        .on_cleanup(move || {
            mentioner.cleanup();
            if let Some(id) = unregister.get() {
                if state.composer_box.get_untracked() == Some(id) {
                    state.composer_box.set(None);
                }
            }
        });

    Stack::vertical((
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Thread, theme::ACCENT)
                .style(|s| s.size(13.0, 13.0).margin_right(6.0)),
            Label::derived(move || anchor_label.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::FG_MUTED)
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            }),
            outdated_pill(outdated),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            // Put the box away. Nothing typed into it is kept. The button
            // is the size of the thread's `...` and sits where it sits,
            // since the thread's card takes the composer's place. While
            // the comment goes there is no putting it away.
            icon_button(
                || SidebarIcon::Close,
                || theme::FG_SUBTLE,
                move || {
                    if !posting_here_now() {
                        state.close_composer();
                    }
                },
            )
            .debug_name("composer-close")
            .style(move |s| {
                s.size(theme::THREAD_MENU, theme::THREAD_MENU)
                    .apply_if(posting_here(), |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().width_full().margin_bottom(8.0)),
        composer_editor_box(state, editor, face == ThreadFace::Inline),
        composer_error_line(state),
        composer_footer(app, state, send, posting_here),
        resize_cursor_sheet(state),
    ))
    .debug_name("composer-box")
    .style(move |s| {
        let base = s.flex_col().padding(12.0).border(1.0).border_radius(14.0);
        let base = match face {
            ThreadFace::Card => base
                .width_full()
                .background(theme::CANVAS_SUBTLE)
                .border_color(theme::BORDER_MUTED),
            // A card in the code is placed by its edges, and its width
            // follows from them.
            ThreadFace::Inline => base
                .background(theme::CANVAS)
                .border_color(theme::ACCENT_RING),
        };
        // Under a thread it is the thread's reply form, as GitHub's is:
        // white, in the thread's rule, with a box's corners.
        base.apply_if(follow_up, |s| {
            s.background(theme::CANVAS)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
        })
    })
}

/// The box the question is typed into, with the grip along the inside
/// of its bottom edge that drags it taller or shorter.
///
/// Until it is dragged the box opens at `COMPOSER_HEIGHT` and grows with
/// what is typed, as GitHub's does. A drag fixes it at a height of the
/// reader's, which it keeps whatever is typed into it, and which the
/// next composer opens at.
fn composer_editor_box(
    state: ReviewState,
    editor: impl IntoView + 'static,
    bordered: bool,
) -> impl IntoView {
    let boxed = editor_box(
        editor,
        move || match state.composer_height.get() {
            Some(height) => (height, height),
            None => (theme::COMPOSER_HEIGHT, theme::COMPOSER_GROWN_HEIGHT),
        },
        bordered,
    );
    let box_id = floem::View::id(&boxed);
    Stack::vertical((boxed, resize_handle(state, box_id)))
        .debug_name("composer editor box")
        .style(|s| s.width_full())
}

/// The band along the inside of the box's bottom edge that drags it
/// taller or shorter, with the grip that says so drawn in the middle of
/// it. It lies in the room under the last line, so it takes nothing
/// from the lines and no room of its own.
///
/// The band sits inside the box rather than over its bottom edge: the
/// error line and the footer are drawn after the box and take the
/// pointer from anything of the box's that hangs below it.
///
/// The height the drag sets is the box's at the grab plus how far the
/// pointer has gone since, measured from the grab rather than from the
/// last move, so a fast drag cannot drift. A box that has not been
/// dragged is grabbed at whatever height it has grown to.
fn resize_handle(state: ReviewState, box_id: floem::ViewId) -> impl IntoView {
    use floem::event::DragConfig;
    use floem::style::Position;

    let grab = RwSignal::new(theme::COMPOSER_HEIGHT);
    let hovered = RwSignal::new(false);

    // The grip is drawn by a child of the band and takes no pointer
    // events of its own: capture goes to whatever the pointer went down
    // on, and a drag from the middle of the band must be the band's like
    // one from either end.
    Container::new(
        Empty::new().style(move |s| {
            s.width(theme::COMPOSER_GRIP_WIDTH)
                .height(theme::COMPOSER_GRIP_MARK)
                .border_radius(theme::COMPOSER_GRIP_MARK / 2.0)
                .pointer_events_none()
                .background(match (state.composer_resizing.get(), hovered.get()) {
                    (true, _) => theme::ACCENT,
                    (false, true) => theme::FG_SUBTLE,
                    (false, false) => theme::BORDER,
                })
        }),
    )
    .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
    .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
    .on_event_stop(el::PointerDown, move |cx, e: &PointerButtonEvent| {
        grab.set(
            state
                .composer_height
                .get_untracked()
                .or_else(|| box_id.get_size().map(|size| size.height))
                .unwrap_or(theme::COMPOSER_HEIGHT),
        );
        if let Some(pointer) = e.pointer.pointer_id {
            if pointer.is_primary_pointer() {
                cx.request_pointer_capture(pointer);
            }
        }
    })
    .on_event_stop(el::GainedPointerCapture, |cx, token| {
        // No ghost and no snap-back animation: this is an edge being
        // moved, not a card being carried somewhere.
        cx.start_drag(
            *token,
            DragConfig {
                threshold: 1.0,
                animation_duration: std::time::Duration::ZERO,
                easing: Rc::new(floem::easing::Linear),
                custom_data: None,
                track_targets: false,
            },
            false,
        );
    })
    .on_event_stop(el::DragStart, move |_, _: &floem::event::DragStartEvent| {
        state.composer_resizing.set(true);
    })
    .on_event_stop(el::DragMove, move |_, e: &floem::event::DragMoveEvent| {
        let dy = e.current_state.logical_point().y - e.start_state.logical_point().y;
        let height = (grab.get_untracked() + dy)
            .clamp(theme::COMPOSER_MIN_HEIGHT, theme::COMPOSER_MAX_HEIGHT);
        state.composer_height.set(Some(height));
    })
    .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
        state.composer_resizing.set(false);
    })
    .on_event_stop(el::DragCancel, move |_, _: &floem::event::DragCancelEvent| {
        state.composer_resizing.set(false);
    })
    .debug_name("composer resize handle")
    .style(|s| {
        s.position(Position::Absolute)
            .inset_left(0.0)
            .inset_right(0.0)
            .inset_bottom(0.0)
            .height(theme::COMPOSER_GRIP)
            .items_center()
            .justify_center()
            .z_index(10)
            .hover(|s| s.cursor(CursorStyle::RowResize))
    })
}

/// A sheet over the card while the grip is being dragged, so the cursor
/// stays the row-resize one. The cursor belongs to whatever is under the
/// pointer, and the grip is only there once layout has caught up with
/// the last move; on a fast drag, or past either limit, the pointer is
/// over the lines being typed or the footer instead. The sheet reaches
/// as far above and below the box as the drag can take the pointer.
/// Pointer events still go to the grip, which holds capture; the sheet
/// only takes the hover.
fn resize_cursor_sheet(state: ReviewState) -> impl IntoView {
    use floem::style::Position;

    Empty::new()
        .debug_name("composer resize cursor sheet")
        .style(move |s| {
            s.position(Position::Absolute)
                .inset_left(0.0)
                .inset_right(0.0)
                .inset_top(-theme::COMPOSER_MAX_HEIGHT)
                .inset_bottom(-theme::COMPOSER_MAX_HEIGHT)
                .z_index(100)
                .cursor(CursorStyle::RowResize)
                .apply_if(!state.composer_resizing.get(), |s| s.hide())
        })
}

/// Whether the command a box's keymap resolved a key to is Enter on its
/// own, which the confirm under the box takes for a yes.
fn plain_enter(cmd: &floem::views::editor::text_document::PreCommand<'_>) -> bool {
    use floem::views::editor::command::Command;
    use floem::views::editor::core::command::EditCommand;

    matches!(cmd.cmd, Command::Edit(EditCommand::InsertNewLine)) && cmd.mods.is_empty()
}

/// Why the last issue comment did not post, under the box. Nothing
/// while there is nothing to say.
fn composer_error_line(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || state.composer_error.get(),
        |error| match error {
            Some(error) => Label::derived(move || error.clone())
                .style(|s| {
                    s.width_full()
                        .font_size(theme::TEXT_BADGE)
                        .color(theme::CLOSED)
                        .text_wrap()
                        .margin_top(6.0)
                })
                .debug_name("composer-error")
                .into_any(),
            None => Empty::new().into_any(),
        },
    )
}

/// What the composer is in the middle of, for its footer: taking text,
/// asking whether to post an issue comment, or posting one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FooterMode {
    Ready,
    Confirming,
    Posting,
}

/// The line under the box: the destination, the effort and Send. When
/// Comment was clicked once, the same line asks `Post to
/// github.com/owner/repo/pull/12?` with Post and Cancel; while this
/// composer's comment goes, it says so and offers nothing.
fn composer_footer(
    app: AppState,
    state: ReviewState,
    send: Rc<dyn Fn()>,
    posting: impl Fn() -> bool + Copy + 'static,
) -> impl IntoView {
    let mode = move || {
        if posting() {
            FooterMode::Posting
        } else if state.composer_confirming.get() {
            FooterMode::Confirming
        } else {
            FooterMode::Ready
        }
    };
    dyn_container(mode, move |mode| match mode {
        FooterMode::Ready => Stack::horizontal((
            mode_switch(app, state),
            destination_picker(app, state),
            // No effort to pick for GitHub.
            dyn_container(
                move || {
                    state
                        .composer
                        .with(|c| c.as_ref().is_some_and(|c| c.destination.is_github()))
                },
                move |github| {
                    if github {
                        Empty::new().into_any()
                    } else {
                        effort_picker(app, state).into_any()
                    }
                },
            ),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            send_button(move || send_label(state), send.clone()).debug_name("composer-send"),
        ))
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0))
        .into_any(),
        FooterMode::Confirming => Stack::horizontal((
            Label::derived(move || {
                let target = state.pull_request.with(|pull| {
                    pull.as_ref()
                        .map(|pull| {
                            format!("github.com/{}/pull/{}", pull.repo_full_name, pull.number)
                        })
                        .unwrap_or_else(|| "GitHub".to_string())
                });
                format!("Post to {target}?")
            })
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            })
            .debug_name("composer-confirm"),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            text_button(|| "Cancel".to_string(), SidebarIcon::Close, move || {
                state.composer_confirming.set(false)
            })
            .debug_name("composer-cancel-post"),
            send_button(|| "Post".to_string(), send.clone()).debug_name("composer-post"),
        ))
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0))
        .into_any(),
        FooterMode::Posting => Stack::horizontal((
            Label::derived(|| "Posting\u{2026}".to_string())
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::FG_MUTED)
                        .selectable(false)
                })
                .debug_name("composer-posting"),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
        ))
        .style(|s| s.items_center().width_full().height(30.0).margin_top(8.0))
        .into_any(),
    })
}

/// What the send does, on its button: `Send` to an agent, `Add to
/// review` for a draft, `Comment` for an issue comment that posts at
/// once. Tracked over the composer.
/// Whether the open item takes Work runs: a branch of the reader's own,
/// or a pull request they opened, on disk.
fn work_offered(app: AppState, state: ReviewState) -> bool {
    let viewer = app.viewer.with(|v| v.as_ref().map(|(login, _)| login.clone()));
    state.item.with(|item| {
        item.as_ref()
            .is_some_and(|item| crate::items::work_allowed(item, viewer.as_deref()))
    })
}

/// The mode a send goes with: the composer's, where the item takes Work.
fn sending_mode(
    app: AppState,
    state: ReviewState,
    mode: perch_github::RunMode,
) -> perch_github::RunMode {
    if floem::reactive::Effect::untrack(|| work_offered(app, state)) {
        mode
    } else {
        perch_github::RunMode::Ask
    }
}

/// Ask or Work, before the model, on an item that takes Work and while
/// the text is bound for an agent. Absent anywhere else, where every
/// message is a question.
fn mode_switch(app: AppState, state: ReviewState) -> impl IntoView {
    use perch_github::RunMode;

    let shown = move || {
        work_offered(app, state)
            && state
                .composer
                .with(|c| c.as_ref().is_some_and(|c| !c.destination.is_github()))
    };
    let segment = move |mode: RunMode| {
        let current = move || {
            state
                .composer
                .with(|c| c.as_ref().map(|c| c.mode) == Some(mode))
        };
        Label::derived(move || mode.label().to_string())
            .on_event_stop(el::Click, move |_, _| state.set_composer_mode(mode))
            .debug_name(format!("composer-mode:{}", mode.name()))
            .style(move |s| {
                let on = current();
                clickable(s)
                    .height(24.0)
                    .padding_horiz(9.0)
                    .items_center()
                    .font_size(theme::TEXT_BADGE)
                    .border_radius(6.0)
                    .color(if on { theme::FG } else { theme::FG_MUTED })
                    .apply_if(on, |s| {
                        s.background(theme::CANVAS)
                            .font_weight(theme::weight_emphasis())
                    })
                    .apply_if(!on, |s| s.hover(|s| s.color(theme::FG)))
            })
    };
    dyn_container(shown, move |shown| {
        if shown {
            Stack::horizontal((segment(RunMode::Ask), segment(RunMode::Work)))
                .debug_name("composer-mode")
                .style(|s| {
                    s.items_center()
                        .padding(2.0)
                        .gap(2.0)
                        .background(theme::CANVAS_SUBTLE)
                        .border(1.0)
                        .border_color(theme::BORDER)
                        .border_radius(8.0)
                })
                .into_any()
        } else {
            Empty::new().into_any()
        }
    })
}

fn send_label(state: ReviewState) -> String {
    state.composer.with(|c| match c.as_ref() {
        Some(c) if c.destination.is_github() => {
            if state.posts_issue_comment(c) {
                "Comment".to_string()
            } else {
                "Add to review".to_string()
            }
        }
        _ => "Send".to_string(),
    })
}

/// The one high-contrast action in the composer, matching Codex's dark
/// primary controls while the model and effort remain quiet selectors.
/// `Send` to an agent; `Add to review` when the text is bound for GitHub;
/// `Comment`, then `Post`, for an issue comment.
fn send_button(label: impl Fn() -> String + 'static, on_click: Rc<dyn Fn()>) -> impl IntoView {
    Container::new(
        Stack::horizontal((
            Label::derived(label).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::CANVAS)
                    .margin_right(6.0)
                    .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                    .selectable(false)
            }),
            sidebar_glyph(SidebarIcon::ChevronRight, theme::CANVAS).style(|s| s.size(13.0, 13.0)),
        ))
        .style(|s| s.items_center()),
    )
    .on_event_stop(el::Click, move |_, _| on_click())
    .style(|s| {
        clickable(s)
            .height(30.0)
            .items_center()
            .padding_horiz(12.0)
            .background(theme::FG)
            .border_radius(9.0)
            .hover(|s| s.background(theme::ACTION_HOVER))
    })
}

/// What the item is, for a placeholder: "pull request", "issue" or
/// "branch".
pub fn subject_word(state: ReviewState) -> &'static str {
    if state.pull_request.with(Option::is_some) {
        "pull request"
    } else if state.issue.with(Option::is_some) {
        "issue"
    } else {
        "branch"
    }
}

/// Where the text goes, from what config offers: Claude Code's models,
/// then Codex's under a line when there are any, then GitHub under
/// another. A model carries its backend, so picking one sets both, and
/// keeps the effort where the model takes it. GitHub can be picked where
/// a draft has somewhere to land, a composer on lines or under a thread
/// that is on GitHub, and where the text posts at once as an issue
/// comment: the foot of a pull request's conversation, or under a thread
/// with neither lines nor a place on GitHub. A draft being edited stays a
/// draft: the models are greyed out, since sending would replace its
/// body, not ask.
fn destination_picker(app: AppState, state: ReviewState) -> impl IntoView {
    use floem::menu::Menu;

    let current = move || {
        state.composer.with(|c| {
            c.as_ref()
                .map(|c| destination_label(&c.destination))
                .unwrap_or_default()
        })
    };
    menu_button(current, move || {
        let Some(composer) = state.composer.get_untracked() else {
            return Menu::new();
        };
        let selected = composer.destination.agent_choice().map(|(_, m, _)| m);
        let models_offered = composer.editing.is_none();
        let mut menu = Menu::new();
        for (i, group) in model_groups(&app.agent.get_untracked())
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                menu = menu.separator();
            }
            for (backend, model) in group {
                let label = model_label(&model);
                let pick = model.clone();
                let checked = selected.as_deref() == Some(model.as_str());
                menu = menu.item(label, move |item| {
                    let pick = pick.clone();
                    item.checked(checked).enabled(models_offered).action(move || {
                        let agent = app.agent.get_untracked();
                        // The effort stays where it was, or where it was
                        // last for an agent when coming back from GitHub.
                        let effort = state
                            .composer
                            .with_untracked(|c| c.as_ref().and_then(|c| c.destination.agent_choice()))
                            .map(|(_, _, e)| e)
                            .or_else(|| state.agent_last.get_untracked().map(|(_, _, e)| e))
                            .unwrap_or_else(|| defaults(app).2);
                        state.set_destination(Destination::Agent {
                            backend,
                            model: pick.clone(),
                            effort: nearest_effort(&agent, &pick, effort),
                        })
                    })
                });
            }
        }
        let github_offered = state.github_offered(&composer);
        menu.separator().item("GitHub", move |item| {
            item.checked(composer.destination.is_github())
                .enabled(github_offered)
                .action(move || state.set_destination(Destination::GitHub))
        })
    })
}

/// The picker's face: `Claude · Opus`, `Codex · gpt-5.5`, or `GitHub`.
pub fn destination_label(destination: &Destination) -> String {
    match destination {
        Destination::Agent { backend, model, .. } => {
            let agent = match backend {
                perch_agent::Backend::ClaudeCode => "Claude",
                perch_agent::Backend::Codex => "Codex",
            };
            format!("{agent} \u{00b7} {}", model_label(model))
        }
        Destination::GitHub => "GitHub".to_string(),
    }
}

/// The picker's sections: one per backend that has models, Claude Code
/// first. Each entry names the backend the model runs on, which is
/// config's call, not the section's, so a model listed under both stays
/// with Claude Code wherever it appears.
pub(crate) fn model_groups(agent: &perch_github::AgentConfig) -> Vec<Vec<(perch_agent::Backend, String)>> {
    let entry = |m: &String| (crate::ask::backend_for(agent, m), m.clone());
    let claude: Vec<_> = agent.models.iter().map(entry).collect();
    let codex: Vec<_> = agent.codex_models().iter().map(entry).collect();
    [claude, codex]
        .into_iter()
        .filter(|g| !g.is_empty())
        .collect()
}

/// The effort a model keeps when the one chosen is not among those it
/// takes: the highest it does take, so a reader who asked for Ultra of a
/// model without it gets Max and sees so, rather than Codex quietly
/// running Max under the name Ultra.
pub(crate) fn nearest_effort(
    agent: &perch_github::AgentConfig,
    model: &str,
    wanted: perch_agent::Effort,
) -> perch_agent::Effort {
    if agent.effort_supported(model, wanted.name()) {
        return wanted;
    }
    perch_agent::Effort::ALL
        .into_iter()
        .rev()
        .find(|e| agent.effort_supported(model, e.name()))
        .unwrap_or(wanted)
}

/// The effort to run with. A level the chosen model does not take, by
/// Codex's own catalog, is shown but cannot be picked.
fn effort_picker(app: AppState, state: ReviewState) -> impl IntoView {
    use floem::menu::Menu;

    let choice = move || {
        state
            .composer
            .with(|c| c.as_ref().and_then(|c| c.destination.agent_choice()))
    };
    let current = move || {
        choice()
            .map(|(_, _, e)| e.label().to_string())
            .unwrap_or_default()
    };
    menu_button(current, move || {
        // Only drawn for an agent; nothing to list otherwise.
        let Some((_, model, selected)) = floem::reactive::Effect::untrack(choice) else {
            return Menu::new();
        };
        let agent = app.agent.get_untracked();
        let mut menu = Menu::new();
        for effort in perch_agent::Effort::ALL {
            let takes = agent.effort_supported(&model, effort.name());
            menu = menu.item(effort.label(), move |item| {
                item.checked(effort == selected)
                    .enabled(takes)
                    .action(move || {
                        state.composer.update(|c| {
                            if let Some(Composer {
                                destination: Destination::Agent { effort: e, .. },
                                ..
                            }) = c
                            {
                                *e = effort;
                            }
                        })
                    })
            });
        }
        menu
    })
}

/// A small pop-up button, like the toolbar's, sized for the composer.
fn menu_button(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> floem::menu::Menu + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG)
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
            .style(|s| s.size(10.0, 10.0).pointer_events_none()),
    ))
    .popout_menu(menu)
    .style(|s| {
        s.items_center()
            .height(26.0)
            .padding_horiz(8.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .selectable(false)
            .hover(|s| {
                s.background(theme::CANVAS_SUBTLE)
                    .cursor(CursorStyle::Pointer)
            })
    })
}

/// The Outdated mark beside an anchor whose lines have moved.
pub fn outdated_pill(outdated: bool) -> impl IntoView {
    if !outdated {
        return Empty::new().into_any();
    }
    Label::derived(|| "Outdated".to_string())
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::NOTICE_FG)
                .background(theme::NOTICE_BG)
                .border(1.0)
                .border_color(theme::NOTICE_BORDER)
                .border_radius(9.0)
                .padding_horiz(6.0)
                .margin_left(8.0)
                .selectable(false)
        })
        .into_any()
}

// ---------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------

/// A thread inline in the code, as github.com's Files changed draws one:
/// a box under the lines it is about, headed "Comment on line R12" with
/// the chevron that collapses it to one row, then its comments, the
/// Follow up box and the foot with Resolve. A gutter marker click
/// flashes its border. One of the reader's own has a dashed border and
/// one of GitHub's a solid one, so neither can be taken for the other.
pub fn thread_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
) -> impl IntoView {
    let card_id = CardId::Thread(thread.id.clone());
    let flashing = RwSignal::new(false);
    let origin = thread.origin;

    let collapsed = {
        let card_id = card_id.clone();
        Memo::new(move |_| state.is_collapsed(&card_id))
    };
    let open_thread = thread.clone();
    let open_backend = backend.clone();
    let card = dyn_container(
        move || collapsed.get(),
        move |collapsed| {
            if collapsed {
                collapsed_row(app, state, open_thread.clone()).into_any()
            } else {
                open_inline_card(app, state, open_backend.clone(), open_thread.clone()).into_any()
            }
        },
    )
    .style(move |s| {
        // Collapsed, it is its one row exactly, on the grey GitHub folds
        // a thread to. The layer sets its width from the edges it is
        // placed against.
        let folded = collapsed.get();
        let base = s
            .apply_if(folded, |s| s.height(theme::CODE_LINE_HEIGHT as f64))
            .background(if folded { theme::CANVAS_SUBTLE } else { theme::CANVAS })
            .border_radius(theme::RADIUS)
            .border_color(if flashing.get() { theme::ACCENT } else { theme::BORDER });
        match origin {
            ThreadOrigin::Local => {
                base.border(floem::kurbo::Stroke::new(1.0).with_dashes(0.0, theme::THREAD_DASH))
            }
            ThreadOrigin::GitHub => base.border(1.0),
        }
    });

    // The editor layer scrolls to inline cards before asking them to flash.
    wire_flash(state, card_id, flashing);
    card
}

/// A gutter marker was clicked on the card's thread: `flashing` goes on
/// for the flash's length. The first run only subscribes: a card built
/// after a click must not take the click for its own.
pub(crate) fn wire_flash(state: ReviewState, card_id: CardId, flashing: RwSignal<bool>) {
    Effect::new(move |first: Option<()>| {
        let focused = state.focus_card.with(|f| f.as_ref() == Some(&card_id));
        if first.is_none() || !focused {
            return;
        }
        flashing.set(true);
        floem::action::exec_after(
            std::time::Duration::from_millis(theme::CARD_FLASH_MILLIS),
            move |_| {
                flashing.set(false);
            },
        );
    });
}

/// A thread's card open: its header, a finding's controls and the delete
/// confirmation when one is asked for, the comments, and the foot. The
/// card is as tall as its thread, however long: nothing in it scrolls.
fn open_inline_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
) -> impl IntoView {
    let menu = thread_menu(state, &thread, theme::THREAD_MENU).into_any();
    let messages = messages_view(
        app,
        state,
        backend.clone(),
        thread.id.clone(),
        ThreadFace::Inline,
    );
    Stack::vertical((
        inline_header(app, state, &thread, menu).style(|s| s.flex_shrink(0.0_f32)),
        notices(app, state, backend.clone(), &thread, ThreadFace::Inline)
            .style(|s| s.flex_shrink(0.0_f32)),
        messages,
        follow_up(app, state, backend, thread.id.clone(), ThreadFace::Inline)
            .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(|s| s.flex_col().width_full())
}

/// The header across a thread in the code, as Files changed has it: the
/// chevron that collapses the card, the lines the thread is on,
/// Outdated, Local on one of the reader's own, Hide resolved on one that
/// is resolved, and the `...` menu. The chevron sits where the collapsed
/// row has it, so it does not move out from under the pointer.
fn inline_header(
    app: AppState,
    state: ReviewState,
    thread: &CommentThread,
    menu: floem::AnyView,
) -> impl IntoView {
    let card_id = CardId::Thread(thread.id.clone());
    let toggle_id = card_id.clone();
    Stack::horizontal((
        chevron(state, card_id, true),
        lines_heading(app, state, thread),
        outdated_pill(thread.outdated),
        local_chip(!thread.is_github_bound()),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        resolved_toggle(
            state,
            thread.id.clone(),
            || false,
            ThreadFace::Inline,
            move || state.toggle_card(&toggle_id),
        ),
        menu,
    ))
    .debug_name("thread-header")
    .style(|s| {
        s.items_center()
            .width_full()
            .height(theme::INLINE_THREAD_HEADER)
            .padding_horiz(theme::COLLAPSED_CARD_PADDING)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// What a thread in the code is on, as Files changed words it: "Comment
/// on line R12", or on lines R10 to R12, L for the base's side. A thread
/// of the reader's own is a question, and a finding's a finding.
fn lines_heading(app: AppState, state: ReviewState, thread: &CommentThread) -> floem::AnyView {
    let Some(anchor) = thread.anchor.clone() else {
        return Empty::new().into_any();
    };
    let side = match anchor.side {
        perch_github::Side::Left => "L",
        perch_github::Side::Right => "R",
    };
    let id = thread.id.clone();
    let github = thread.is_github_bound();
    let range = anchor.start_line != anchor.line;
    let words = move || {
        let noun = if github {
            "Comment"
        } else if cards::finding_of(app, state, &id).is_some() {
            "Finding"
        } else {
            "Question"
        };
        format!("{noun} on {}", if range { "lines" } else { "line" })
    };
    let muted = |s: floem::style::Style| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::FG_MUTED)
            .selectable(false)
            .flex_shrink(0.0_f32)
    };
    let strong = |s: floem::style::Style| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(theme::weight_strong())
            .color(theme::FG)
            .selectable(false)
            .margin_left(4.0)
            .flex_shrink(0.0_f32)
    };
    let first = format!("{side}{}", anchor.start_line);
    let last = format!("{side}{}", anchor.line);
    Stack::horizontal((
        Label::derived(words).style(muted),
        Label::derived(move || first.clone()).style(strong),
        if range {
            Stack::horizontal((
                Label::derived(|| "to".to_string()).style(move |s| muted(s).margin_left(4.0)),
                Label::derived(move || last.clone()).style(strong),
            ))
            .style(|s| s.items_center())
            .into_any()
        } else {
            Empty::new().into_any()
        },
    ))
    .debug_name("lines-heading")
    .style(|s| s.items_center().margin_left(6.0).min_width(0.0))
    .into_any()
}

/// The mark that says a thread is Perch's and not GitHub's, where
/// `shown`.
pub(crate) fn local_chip(shown: bool) -> floem::AnyView {
    if !shown {
        return Empty::new().into_any();
    }
    cards::tip(
        Container::new(Label::derived(|| "Local".to_string()).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_MUTED)
                .selectable(false)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }))
        .style(|s| {
            s.height(theme::CHIP_HEIGHT)
                .padding_horiz(7.0)
                .margin_left(8.0)
                .items_center()
                .justify_center()
                .background(theme::BADGE_BG)
                .border_radius(theme::CHIP_HEIGHT / 2.0)
                .flex_shrink(0.0_f32)
        }),
        "This thread is on this machine and is not on GitHub.".to_string(),
    )
    .into_any()
}

/// Show resolved on a resolved thread that is `folded` away, and Hide
/// resolved on one the reader opened, as github.com offers either at
/// the end of a resolved thread's header. Hidden on a thread that is not
/// resolved.
pub(crate) fn resolved_toggle(
    state: ReviewState,
    thread_id: String,
    folded: impl Fn() -> bool + Copy + 'static,
    face: ThreadFace,
    toggle: impl Fn() + 'static,
) -> impl IntoView {
    let resolved = Memo::new(move |_| state.is_resolved(&thread_id));
    let (height, size) = match face {
        ThreadFace::Card => (theme::THREAD_HEADER_BUTTON, theme::TEXT_META),
        ThreadFace::Inline => (theme::COLLAPSED_CARD_MENU, theme::TEXT_BADGE),
    };
    Stack::horizontal((
        crate::ui::widgets::icon_in(
            move || {
                let icon = if folded() {
                    SidebarIcon::Unfold
                } else {
                    SidebarIcon::Fold
                };
                crate::icons::sidebar_icon(icon, theme::FG_MUTED)
            },
            || theme::FG_MUTED,
        )
        .style(|s| s.size(14.0, 14.0).margin_right(4.0)),
        Label::derived(move || {
            if folded() {
                "Show resolved".to_string()
            } else {
                "Hide resolved".to_string()
            }
        })
        .style(move |s| s.font_size(size).color(theme::FG_MUTED).selectable(false)),
    ))
    .on_event_stop(el::Click, move |_, _| toggle())
    .debug_name("resolved-toggle")
    .style(move |s| {
        clickable(s)
            .items_center()
            .height(height)
            .padding_horiz(6.0)
            .border_radius(theme::RADIUS)
            .flex_shrink(0.0_f32)
            .hover(|s| s.background(theme::BADGE_BG))
            .apply_if(!resolved.get(), |s| s.hide())
    })
}

/// A thread collapsed to its one row: who spoke first, the first line of
/// what they said, how many messages follow, Outdated, Show resolved on
/// one that is resolved, and the `...` menu, smaller than the open
/// card's so it fits the row.
fn collapsed_row(app: AppState, state: ReviewState, thread: CommentThread) -> impl IntoView {
    let first = thread.messages.first();
    let author = first.map(|m| m.author.clone()).unwrap_or(Author::User);
    let line = first
        .map(|m| crate::review::timeline::first_line(&m.body))
        .unwrap_or_default();
    let card_id = CardId::Thread(thread.id.clone());
    let toggle_id = card_id.clone();
    // Set off from the pill before it as the pills are from what is
    // before them: the pill's fill and the menu's hover fill are one
    // colour, and would touch.
    let trailing = Stack::horizontal((
        resolved_toggle(
            state,
            thread.id.clone(),
            || true,
            ThreadFace::Inline,
            move || state.toggle_card(&toggle_id),
        )
        .style(|s| s.margin_left(theme::COLLAPSED_CARD_PILL_GAP)),
        thread_menu(state, &thread, theme::COLLAPSED_CARD_MENU)
            .style(|s| s.margin_left(theme::COLLAPSED_CARD_PILL_GAP)),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
    .into_any();
    let avatar = author_avatar(app, state, &author, theme::COLLAPSED_CARD_PILL);
    collapsed_card_row(
        state,
        card_id,
        avatar,
        move || author_name(app, &author),
        line,
        thread.messages.len().saturating_sub(1),
        thread.outdated,
        trailing,
    )
}

/// A card in the code collapsed to its one row: the chevron, the face
/// and name of the `author`, the first `line` of the first message, how
/// many `more` messages follow, Outdated, and at the end whatever the
/// card puts there. The author is read each time it is drawn, since who
/// the reader is can land after the card is built.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collapsed_card_row(
    state: ReviewState,
    card_id: CardId,
    avatar: floem::AnyView,
    author: impl Fn() -> Name + 'static,
    line: String,
    more: usize,
    outdated: bool,
    trailing: floem::AnyView,
) -> impl IntoView {
    Stack::horizontal((
        chevron(state, card_id, false),
        avatar.style(|s| s.margin_left(6.0).flex_shrink(0.0_f32)),
        name_label(author, |s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_strong())
                .color(theme::FG)
                .selectable(false)
                .margin_left(6.0)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || line.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG_MUTED)
                .selectable(false)
                .margin_left(6.0)
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
        }),
        more_pill(more),
        outdated_pill(outdated),
        trailing,
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .height_full()
            .padding_horiz(theme::COLLAPSED_CARD_PADDING)
    })
}

/// Who wrote a message, as a collapsed card names them: the reader by
/// their login once GitHub has said it, the agent by its model. A login
/// is a link to its page; a model and "You" lead nowhere.
fn author_name(app: AppState, author: &Author) -> Name {
    match author {
        Author::User => app
            .viewer
            .get()
            .map(|(login, _)| login)
            .filter(|l| !l.is_empty())
            .map(Name::Login)
            .unwrap_or_else(|| Name::Plain("You".into())),
        Author::Agent { model, .. } => Name::Plain(model_label(model)),
        Author::GitHubUser(login) => Name::Login(login.clone()),
    }
}

/// The face beside a message, `size` across: a login's picture, the
/// reader's own, or an agent's mark.
pub(crate) fn author_avatar(
    app: AppState,
    state: ReviewState,
    author: &Author,
    size: f64,
) -> floem::AnyView {
    match author {
        Author::GitHubUser(login) => crate::ui::widgets::avatar(
            app,
            cards::avatar_url_of(state, login.clone()),
            size,
            login.clone(),
        )
        .into_any(),
        Author::User => viewer_avatar(app, size).into_any(),
        Author::Agent { .. } => agent_avatar(size).into_any(),
    }
}

/// The reader's face, once GitHub has said who the token belongs to, and
/// their initial on a tint until then. Read live, since that answer can
/// land after the view is built.
fn viewer_avatar(app: AppState, size: f64) -> impl IntoView {
    let viewer = move || app.viewer.get().filter(|(l, _)| !l.is_empty());
    let seed = viewer().map(|(l, _)| l).unwrap_or_else(|| "you".into());
    crate::ui::widgets::avatar(
        app,
        move || viewer().map(|(_, a)| a).unwrap_or_default(),
        size,
        seed,
    )
}

/// The reader's login, once GitHub has said it.
fn viewer_login(app: AppState) -> Option<String> {
    app.viewer
        .get()
        .map(|(login, _)| login)
        .filter(|l| !l.is_empty())
}

/// An agent's mark in place of a face: its glyph on a grey disc.
fn agent_avatar(size: f64) -> impl IntoView {
    Container::new(
        sidebar_glyph(SidebarIcon::Agents, theme::FG_MUTED)
            .style(move |s| s.size(size * 0.6, size * 0.6)),
    )
    .debug_name("agent-avatar")
    .style(move |s| {
        s.size(size, size)
            .border_radius(size / 2.0)
            .background(theme::BADGE_BG)
            .items_center()
            .justify_center()
            .flex_shrink(0.0_f32)
    })
}

/// "3 more": how many messages a collapsed card's first line stands in
/// for. Nothing when none do.
fn more_pill(more: usize) -> impl IntoView {
    if more == 0 {
        return Empty::new().into_any();
    }
    Label::derived(move || format!("{more} more"))
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_SUBTLE)
                .background(theme::BADGE_BG)
                .height(theme::COLLAPSED_CARD_PILL)
                .border_radius(theme::COLLAPSED_CARD_PILL / 2.0)
                .padding_horiz(6.0)
                .margin_left(theme::COLLAPSED_CARD_PILL_GAP)
                .items_center()
                .justify_center()
                .selectable(false)
                .flex_shrink(0.0_f32)
        })
        .into_any()
}

/// The chevron at a card's left: pointing right while the card is
/// collapsed and down while it is open, as a run's steps have it, in a
/// slot the size of the gutter's controls. Click toggles the card.
fn chevron(state: ReviewState, card_id: CardId, open: bool) -> impl IntoView {
    chevron_button(
        move || open,
        theme::CHEVRON_SLOT,
        theme::CHEVRON_GLYPH,
        move || state.toggle_card(&card_id),
    )
}

/// A chevron `glyph` across in a square button `slot` across, pointing
/// down while what it opens is `open` and right while it is not. The
/// glyph is the one for the way it points rather than one turned: vger
/// draws an SVG square to the screen, and a turn would only move it.
pub(crate) fn chevron_button(
    open: impl Fn() -> bool + 'static,
    slot: f64,
    glyph: f64,
    toggle: impl Fn() + 'static,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let color = move || {
        if hovered.get() {
            theme::ACCENT
        } else {
            theme::FG_SUBTLE
        }
    };
    Container::new(
        crate::ui::widgets::icon_in(
            move || {
                let icon = if open() {
                    SidebarIcon::ChevronDown
                } else {
                    SidebarIcon::ChevronRight
                };
                crate::icons::sidebar_icon(icon, color())
            },
            color,
        )
        .style(move |s| s.size(glyph, glyph)),
    )
    .on_event_stop(el::Click, move |_, _| toggle())
    .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
    .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
    .debug_name("thread-chevron")
    .style(move |s| {
        clickable(s)
            .size(slot, slot)
            .items_center()
            .justify_center()
            .border_radius(if slot > theme::CHEVRON_SLOT { theme::RADIUS } else { 3.0 })
            .flex_shrink(0.0_f32)
            .hover(|s| s.background(theme::BADGE_BG))
    })
}

/// "Delete this thread?" with Delete and Cancel, under the thread that
/// asked, on its card. Nothing else deletes. On a thread of GitHub's,
/// `github_bound`, it is Perch's messages that go: the questions and
/// answers asked there, and never the thread or anything on GitHub.
pub fn delete_confirmation(
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
    github_bound: bool,
) -> impl IntoView {
    let asking = Memo::new(move |_| {
        state
            .confirm_thread
            .with(|c| c.as_deref() == Some(thread_id.as_str()))
    });
    dyn_container(
        move || asking.get(),
        move |asking| {
            if !asking {
                return Empty::new().into_any();
            }
            let backend = backend.clone();
            let (title, copy) = if github_bound {
                (
                    "Delete Perch's messages?",
                    "The questions and answers asked here go. Nothing on GitHub is touched.",
                )
            } else {
                (
                    "Delete this thread?",
                    "The question, its answers and the anchor go. Nothing on GitHub is touched.",
                )
            };
            Stack::vertical((
                Label::derived(move || title.to_string()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::FG)
                        .selectable(false)
                }),
                Label::derived(move || copy.to_string())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .text_wrap()
                        .width_full()
                        .margin_top(2.0)
                }),
                Stack::horizontal((
                    text_button(
                        || "Delete".to_string(),
                        SidebarIcon::Trash,
                        move || {
                            if let Some(id) = state.confirm_thread.get_untracked() {
                                if github_bound {
                                    backend.delete_private_messages(&id);
                                } else {
                                    backend.delete_comment_thread(&id);
                                }
                            }
                            state.confirm_thread.set(None);
                        },
                    ),
                    text_button(
                        || "Cancel".to_string(),
                        SidebarIcon::Close,
                        move || state.confirm_thread.set(None),
                    ),
                ))
                .style(|s| s.gap(6.0).margin_top(8.0)),
            ))
            .on_event_stop(el::Click, |_, _| {})
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .padding(10.0)
                    .background(theme::NOTICE_BG)
                    .border(1.0)
                    .border_color(theme::NOTICE_BORDER)
                    .border_radius(theme::RADIUS)
            })
            .into_any()
        },
    )
    // Hidden rather than empty while nothing is asked, so it takes no
    // share of the card's gap.
    .style(move |s| s.width_full().apply_if(!asking.get(), |s| s.hide()))
}

/// The `...` at the end of a thread's header, and the menu under it,
/// in a round button `size` across. A thread of the reader's own offers
/// Delete Thread. One of GitHub's offers to delete Perch's messages under
/// it, and only while there are any; with none, there is no menu at all,
/// since nothing here may touch what is on GitHub.
pub fn thread_menu(state: ReviewState, thread: &CommentThread, size: f64) -> impl IntoView {
    use floem::menu::Menu;

    let github_bound = thread.is_github_bound();
    if github_bound && !thread.messages.iter().any(|m| m.is_private()) {
        return Empty::new().into_any();
    }
    let thread_id = thread.id.clone();
    Container::new(
        sidebar_glyph(SidebarIcon::Ellipsis, theme::FG_SUBTLE).style(|s| s.pointer_events_none()),
    )
    .popout_menu(move || {
        let id = thread_id.clone();
        let label = if github_bound {
            "Delete Perch's Messages\u{2026}"
        } else {
            "Delete Thread\u{2026}"
        };
        // Delete asks first, under the byline, wherever the thread is
        // showing; the confirmation is the one thing that deletes.
        Menu::new().item(label, move |item| {
            let id = id.clone();
            item.action(move || state.confirm_thread.set(Some(id.clone())))
        })
    })
    .style(move |s| {
        clickable(s)
            .size(size, size)
            .items_center()
            .justify_center()
            .border_radius(size / 2.0)
            .flex_shrink(0.0_f32)
            .hover(|s| s.background(theme::BADGE_BG))
    })
    .into_any()
}

/// A thread's messages, read from the state rather than from the thread
/// the caller was built with, so a run's steps and its answer show as they
/// arrive: the row that holds the thread keeps its key through a run, and
/// a kept row is not rebuilt. Set in from the thread's sides; in the code
/// the room at the right is less the scroll bar's, which draws there.
pub fn messages_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
    face: ThreadFace,
) -> impl IntoView {
    let list_id = thread_id.clone();
    // Which draft the composer is editing, if any: that message's row
    // holds the composer instead of the message. On the opening as well,
    // so an edit opened again after Escape gets a fresh box.
    let editing = Memo::new(move |_| {
        state.composer.with(|c| {
            c.as_ref()
                .and_then(|c| c.editing.clone().map(|id| (id, c.opening)))
        })
    });
    dyn_stack(
        move || {
            let editing = editing.get();
            state.comment_threads.with(|t| {
                t.iter()
                    .find(|t| t.id == list_id)
                    .map(|t| {
                        let count = t.messages.len();
                        t.messages
                            .iter()
                            .zip(Lane::of(t))
                            .enumerate()
                            .map(|(i, (m, lane))| {
                                let opening = editing
                                    .as_ref()
                                    .filter(|(id, _)| *id == m.id)
                                    .map(|(_, opening)| *opening);
                                let place = Place {
                                    first: i == 0,
                                    last: i + 1 == count,
                                };
                                (m.clone(), opening, lane, place)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
        },
        // The key follows what the row shows: an edit on GitHub changes
        // the body, a draft posted gains its id, a draft being edited is
        // the composer, a draft added to a thread that was wholly private
        // labels the rest of it Perch only, and a message answered is no
        // longer the last, under which the line down the avatars stops.
        |(m, editing, lane, place)| {
            (
                m.id.clone(),
                m.origin.name(),
                m.github_id,
                cards::body_hash(&m.body),
                m.run
                    .as_ref()
                    .map(|r| (r.status.name(), r.steps.len(), r.error.clone())),
                *editing,
                *lane,
                *place,
            )
        },
        move |(m, editing, lane, _)| {
            let thread = state
                .comment_threads
                .with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
            match (thread, editing) {
                (Some(_), Some(_)) => match state.composer.get_untracked() {
                    Some(c) => composer_view(app, state, backend.clone(), c, face)
                        .style(move |s| s.width_full().padding_vert(face.comment_space()))
                        .into_any(),
                    None => Empty::new().into_any(),
                },
                (Some(thread), None) => {
                    message_view(app, state, backend.clone(), thread, m, face, lane).into_any()
                }
                (None, _) => Empty::new().into_any(),
            }
        },
    )
    .debug_name("thread-messages")
    .style(move |s| {
        let s = s.flex_col().width_full();
        match face {
            ThreadFace::Card => s.padding_horiz(theme::THREAD_INSET),
            ThreadFace::Inline => s.padding_horiz(theme::INLINE_THREAD_INSET),
        }
    })
}

/// What is said about a thread before its comments, each set in from its
/// sides: a finding's provenance and what can be done with it, and the
/// delete confirmation while one is asked for.
pub(crate) fn notices(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    face: ThreadFace,
) -> impl IntoView {
    let inset = face.inset();
    let finding_id = thread.id.clone();
    let finding = Memo::new(move |_| cards::finding_of(app, state, &finding_id).is_some());
    Stack::vertical((
        Container::new(cards::finding_controls(app, state, backend.clone(), thread.id.clone()))
            .style(move |s| {
                s.width_full()
                    .padding_top(inset)
                    .padding_horiz(inset)
                    .apply_if(!finding.get(), |s| s.hide())
            }),
        delete_confirmation(state, backend, thread.id.clone(), thread.is_github_bound())
            .style(move |s| s.padding_top(inset).padding_horiz(inset)),
    ))
    .style(|s| s.flex_col().width_full())
}

/// The foot of a thread: the Follow up box, or the composer when one is
/// open for the thread, and under it the foot with Resolve. On
/// Conversation the box sits in GitHub's grey reply bar beside the
/// reader's face; in the code it sits under the comments.
///
/// The composer is drawn in one place on the visible surface. A thread with
/// a card in the open file takes it inside that card; Conversation's card
/// always takes it when Conversation is visible.
pub fn follow_up(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
    face: ThreadFace,
) -> impl IntoView {
    let composer_thread = thread_id.clone();
    let running_thread = thread_id.clone();
    let reply_thread = thread_id.clone();
    // Keyed on whether a run is going as well as on the composer: the box
    // is absent while the agent works and has to appear when it is done,
    // which only the thread list says.
    let running = Memo::new(move |_| {
        state.comment_threads.with(|t| {
            t.iter()
                .find(|t| t.id == running_thread)
                .is_some_and(|t| t.running_run().is_some())
        })
    });
    // Keyed on what the composer is about and which opening it is, not on
    // the composer: picking a destination writes the composer, and a
    // rebuild on that would throw the typed text away and drop the box's
    // focus.
    let composer = Memo::new(move |_| {
        let composer = state.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.thread_id.as_deref() == Some(composer_thread.as_str()))
                .map(|c| (c.anchor.clone(), c.opening, c.editing.clone()))
        });
        let drawn_here = match face {
            ThreadFace::Inline => state.follow_up_is_inline(&composer_thread),
            ThreadFace::Card => true,
        };
        composer.filter(|_| drawn_here)
    });
    // The composer's box takes the foot; a draft edited up among the
    // messages leaves the box here, and Resolve under it.
    let composing = Memo::new(move |_| matches!(composer.get(), Some((_, _, None))));
    // Nothing in the box's place: an agent at work with no composer open,
    // or a draft being edited up among the comments.
    let empty = Memo::new(move |_| match composer.get() {
        Some((_, _, Some(_))) => true,
        Some(_) => false,
        None => running.get(),
    });
    let offered = resolve_offered(app, state, thread_id.clone());
    let foot_hidden = Memo::new(move |_| !offered.get() || composing.get());
    let reply_backend = backend.clone();
    let reply = dyn_container(
        move || (composer.get(), running.get()),
        move |(composer, _)| match composer {
            // A draft being edited has the composer in its own row, up
            // among the messages; nothing goes here.
            Some((_, _, Some(_))) => Empty::new().into_any(),
            // Under a thread the composer is a child of this container,
            // not a card placed by its edges, and takes the width here.
            Some(_) => match state.composer.get_untracked() {
                Some(c) => composer_view(app, state, reply_backend.clone(), c, face)
                    .style(|s| s.width_full())
                    .into_any(),
                None => Empty::new().into_any(),
            },
            None => {
                let thread = state
                    .comment_threads
                    .with_untracked(|t| t.iter().find(|t| t.id == reply_thread).cloned());
                match thread {
                    Some(thread) => reply_line(app, state, thread, face).into_any(),
                    None => Empty::new().into_any(),
                }
            }
        },
    )
    .style(|s| s.flex_grow(1.0_f32).min_width(0.0));
    let bar = match face {
        ThreadFace::Card => Stack::horizontal((
            viewer_avatar(app, theme::THREAD_AVATAR).style(move |s| {
                s.margin_right(theme::THREAD_AVATAR_GAP)
                    .flex_shrink(0.0_f32)
                    .apply_if(composing.get(), |s| s.hide())
            }),
            reply,
        ))
        .debug_name("reply-bar")
        .style(move |s| {
            // The box's corners, while nothing is under the bar.
            let radius = if foot_hidden.get() {
                theme::RADIUS - 1.0
            } else {
                0.0
            };
            s.items_center()
                .width_full()
                .padding_vert(8.0)
                .padding_horiz(theme::THREAD_INSET)
                .background(theme::CANVAS_SUBTLE)
                .border_top(1.0)
                .border_color(theme::BORDER)
                .set(BorderBottomLeftRadius, radius)
                .set(BorderBottomRightRadius, radius)
                .apply_if(empty.get(), |s| s.hide())
        })
        .into_any(),
        ThreadFace::Inline => Container::new(reply)
            .debug_name("reply-bar")
            .style(move |s| {
                s.width_full()
                    .padding_horiz(theme::INLINE_THREAD_INSET)
                    .padding_bottom(theme::INLINE_THREAD_INSET)
                    .apply_if(empty.get(), |s| s.hide())
            })
            .into_any(),
    };
    Stack::vertical((
        bar,
        resolve_foot(state, backend, thread_id, face, offered, foot_hidden),
    ))
    .style(|s| s.flex_col().width_full())
}

/// Whether a thread's foot offers Resolve: on one of GitHub's where
/// GitHub lets the reader, on any other always, and never on a
/// finding's, which has Dismiss and Reopen instead.
fn resolve_offered(app: AppState, state: ReviewState, thread_id: String) -> Memo<bool> {
    Memo::new(move |_| {
        let resolvable = state.comment_threads.with(|threads| {
            threads
                .iter()
                .find(|t| t.id == thread_id)
                .is_some_and(CommentThread::resolvable)
        });
        resolvable && cards::finding_of(app, state, &thread_id).is_none()
    })
}

/// The foot under a thread with Resolve in it, or Unresolve on one that
/// is resolved, as github.com words them: Resolve conversation on
/// Conversation and Resolve comment in the code. On one of GitHub's it
/// changes github.com at once, as the button there does; on any other it
/// changes only Perch. The thread shows the click before the answer, and
/// resolved, it folds, as one resolved on GitHub arrives. While the ask
/// is out the button is dimmed and takes no click. The foot is `hidden`
/// where Resolve is not `offered`, and while the composer has the
/// thread, whose own buttons are the ones to press.
fn resolve_foot(
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
    face: ThreadFace,
    offered: Memo<bool>,
    hidden: Memo<bool>,
) -> impl IntoView {
    let resolved_id = thread_id.clone();
    let resolved = Memo::new(move |_| state.is_resolved(&resolved_id));
    let button = dyn_container(
        move || offered.get().then(|| resolved.get()),
        move |shown| {
            let Some(resolved) = shown else {
                return Empty::new().into_any();
            };
            let (label, name) = match (face, resolved) {
                (ThreadFace::Card, false) => ("Resolve conversation", "thread-resolve"),
                (ThreadFace::Card, true) => ("Unresolve conversation", "thread-unresolve"),
                (ThreadFace::Inline, false) => ("Resolve comment", "thread-resolve"),
                (ThreadFace::Inline, true) => ("Unresolve comment", "thread-unresolve"),
            };
            let backend = backend.clone();
            let id = thread_id.clone();
            let busy_id = thread_id.clone();
            foot_button(label, face, move || {
                let Some(item_id) = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
                else {
                    return;
                };
                if state.begin_resolve(&id, !resolved) {
                    backend.resolve_thread(&item_id, &id, !resolved);
                }
            })
            .debug_name(name)
            .style(move |s| {
                s.apply_if(state.is_resolving(&busy_id), |s| {
                    s.color(theme::FG_SUBTLE).cursor(CursorStyle::Default)
                })
            })
            .into_any()
        },
    );
    Container::new(button)
        .debug_name("thread-foot")
        .style(move |s| {
            s.width_full()
                .padding_vert(8.0)
                .padding_horiz(face.inset())
                .border_top(1.0)
                .border_color(theme::BORDER)
                .apply_if(hidden.get(), |s| s.hide())
        })
}

/// A button in a thread's foot, as GitHub draws a secondary one: the
/// word on grey, in a rule, with a box's corners. Smaller in the code.
fn foot_button(label: &'static str, face: ThreadFace, action: impl Fn() + 'static) -> Label {
    Label::derived(move || label.to_string())
        .on_event_stop(el::Click, move |_, _| action())
        .style(move |s| {
            let (height, size) = match face {
                ThreadFace::Card => (theme::THREAD_CONTROL, theme::TEXT_BODY),
                ThreadFace::Inline => (theme::THREAD_CONTROL - 4.0, theme::TEXT_META),
            };
            clickable(s)
                .height(height)
                .padding_horiz(12.0)
                .items_center()
                .font_size(size)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG)
                .background(theme::CANVAS_SUBTLE)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .hover(|s| s.background(theme::CANVAS_HOVER))
        })
}

/// One message, laid out as github.com lays out a comment: a face, a
/// byline with the name, when it was said and its labels, and what was
/// said under it. A comment from GitHub has Reply, Ask about this, the
/// reaction picker and its link at the end of its byline under the
/// pointer, and the reactions on it under its text. A draft has the
/// Draft mark, and edit and delete. The reader's question to an agent
/// and the agent's answer are Perch's alone; in a thread that has
/// something on GitHub each is labelled Perch only, so the reader never
/// wonders what a colleague can see.
pub fn message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    message: CommentMessage,
    face: ThreadFace,
    lane: Lane,
) -> floem::AnyView {
    let place = Place::of(&thread, &message.id);
    if let (ThreadOrigin::GitHub, Author::GitHubUser(login)) = (message.origin, &message.author) {
        let login = login.clone();
        return github_message_view(app, state, backend, &thread, &message, login, face, place);
    }
    if message.is_draft() {
        return draft_message_view(app, state, backend, &thread, &message, face, place);
    }
    if message.from_github() {
        return posted_message_view(app, state, backend, &message, face, place);
    }
    local_message_view(app, state, backend, thread, message, face, place, lane != Lane::Flush)
}

/// The height of a comment's byline: the buttons at its end, which the
/// avatar beside it is centred on.
const BYLINE_ROW: f64 = 24.0;

/// The room between a comment's byline and its text.
const COMMENT_BODY_GAP: f64 = 6.0;

/// A message laid out as github.com lays out a comment: the avatar at the
/// left of its byline, and its text under the byline, indented past the
/// avatar, with a line down the avatars from each message to the next.
/// The first message in the code has its text flush under its avatar
/// instead, as Files changed sets the comment a thread opens with, and
/// the line starts under it.
fn comment_frame(
    face: ThreadFace,
    place: Place,
    avatar: floem::AnyView,
    byline: floem::AnyView,
    body: floem::AnyView,
) -> floem::AnyView {
    let space = face.comment_space();
    let top = if place.first { face.inset() } else { space };
    if face == ThreadFace::Inline && place.first {
        return Stack::vertical((
            Stack::horizontal((
                avatar.style(|s| s.margin_right(theme::THREAD_AVATAR_GAP).flex_shrink(0.0_f32)),
                byline,
            ))
            .style(|s| s.items_center().width_full().min_width(0.0)),
            body,
        ))
        .style(move |s| {
            s.flex_col()
                .width_full()
                .gap(COMMENT_BODY_GAP)
                .padding_top(top)
                .padding_bottom(space)
        })
        .into_any();
    }
    let rail = Stack::vertical((
        connector(!place.first).style(move |s| s.height(top).flex_shrink(0.0_f32)),
        Container::new(avatar).style(|s| {
            s.height(BYLINE_ROW)
                .items_center()
                .justify_center()
                .flex_shrink(0.0_f32)
        }),
        connector(!place.last).style(|s| s.flex_grow(1.0_f32)),
    ))
    .debug_name("comment-rail")
    .style(move |s| {
        s.flex_col()
            .items_center()
            .width(face.avatar())
            .flex_shrink(0.0_f32)
    });
    let content = Stack::vertical((byline, body)).style(move |s| {
        s.flex_col()
            .gap(COMMENT_BODY_GAP)
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .padding_top(top)
            .padding_bottom(space)
    });
    Stack::horizontal((rail, content))
        .style(|s| s.width_full().gap(theme::THREAD_AVATAR_GAP))
        .into_any()
}

/// A stretch of the line down the avatars, where it is `shown`, or the
/// room it would take.
fn connector(shown: bool) -> impl IntoView {
    Empty::new().style(move |s| {
        s.width(theme::THREAD_CONNECTOR)
            .apply_if(shown, |s| s.background(theme::BORDER_MUTED))
    })
}

/// A comment's byline: the `name`, then what comes `after` it, when it
/// was said and its labels, and at the end what can be done with it.
fn byline_row(
    name: impl IntoView + 'static,
    after: Vec<floem::AnyView>,
    trailing: floem::AnyView,
) -> floem::AnyView {
    let mut children = vec![name.into_any()];
    children.extend(after);
    children.push(Empty::new().style(|s| s.flex_grow(1.0_f32)).into_any());
    children.push(trailing);
    Stack::horizontal_from_iter(children)
        .debug_name("comment-byline")
        .style(|s| s.items_center().width_full().min_width(0.0).height(BYLINE_ROW))
        .into_any()
}

/// A name as a comment's byline sets it: GitHub's semibold, a step down
/// in the code.
fn name_style(s: floem::style::Style, face: ThreadFace) -> floem::style::Style {
    let size = match face {
        ThreadFace::Card => theme::TEXT_BODY,
        ThreadFace::Inline => theme::TEXT_META,
    };
    s.font_size(size)
        .font_weight(theme::weight_strong())
        .color(theme::FG)
        .flex_shrink(0.0_f32)
}

/// When a message was said, "3 hours ago", kept current by the clock.
fn when_label(
    app: AppState,
    at: jiff::Timestamp,
    face: ThreadFace,
    name: &'static str,
) -> impl IntoView {
    Label::derived(move || crate::timefmt::relative(at, app.now.get()))
        .debug_name(name)
        .style(move |s| {
            let size = match face {
                ThreadFace::Card => theme::TEXT_BODY,
                ThreadFace::Inline => theme::TEXT_BADGE,
            };
            s.font_size(size)
                .color(theme::FG_MUTED)
                .margin_left(6.0)
                .text_ellipsis()
                .min_width(0.0)
        })
}

/// A label in a comment's byline, as GitHub sets Author and its kin: the
/// word in a pill with a rule round it.
fn byline_label(text: &'static str, name: &'static str) -> Label {
    Label::derived(move || text.to_string())
        .debug_name(name)
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_MUTED)
                .padding_horiz(7.0)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::BYLINE_LABEL / 2.0)
                .margin_left(6.0)
                .selectable(false)
                .flex_shrink(0.0_f32)
        })
}

/// Author, beside a comment by whoever opened the pull request or the
/// issue, as GitHub marks them. `login` is read as it is drawn.
fn author_label(
    state: ReviewState,
    login: impl Fn() -> Option<String> + 'static,
) -> impl IntoView {
    let is_author = Memo::new(move |_| {
        let Some(login) = login() else {
            return false;
        };
        let pull = state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.author_login == login));
        pull.or_else(|| {
            state
                .issue
                .with(|i| i.as_ref().map(|i| i.author_login == login))
        })
        .unwrap_or(false)
    });
    byline_label("Author", "author-label")
        .style(move |s| s.apply_if(!is_author.get(), |s| s.hide()))
}

/// Perch only, in the byline of a message nobody on GitHub can see, in a
/// thread where they can see the rest.
fn perch_only_label() -> impl IntoView {
    cards::tip(
        byline_label("Perch only", "perch-only"),
        "Only Perch shows this; nothing here is on GitHub.".to_string(),
    )
}

/// The reader's question to an agent, or an agent's answer, under the
/// reader's face and name or the agent's mark and its model, `private`
/// marking it Perch only. A finished run sits on the answer, and reads in
/// the order it went: what the agent did, then what it said, then what
/// it cost. While it runs, or when it failed, the run sits under the
/// question whole. An answer that is done, or a finding, which has no run
/// of its own, has Post to GitHub and Copy at the end of its byline.
#[allow(clippy::too_many_arguments)]
fn local_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    message: CommentMessage,
    face: ThreadFace,
    place: Place,
    private: bool,
) -> floem::AnyView {
    let hovered = RwSignal::new(false);
    let is_agent = matches!(message.author, Author::Agent { .. });
    let body = message.body.clone();
    let finished = message
        .run
        .as_ref()
        .is_some_and(|r| r.status == RunStatus::Completed);
    let settled = is_agent
        && !body.trim().is_empty()
        && (finished || message.run.is_none());
    let actions = if settled {
        agent_actions(state, thread, body.clone(), hovered)
    } else {
        Empty::new().into_any()
    };
    let content = match message.run.clone() {
        Some(run) if is_agent && finished => Stack::vertical((
            run_surface(steps_view(state, run.clone())),
            crate::review::markdown::render_linked(&body, state),
            run_footer(backend, run),
        ))
        .style(|s| s.flex_col().width_full().gap(8.0))
        .into_any(),
        Some(run) => Stack::vertical((
            crate::review::markdown::render_linked(&body, state),
            run_block(state, backend, run),
        ))
        .style(|s| s.flex_col().width_full().gap(8.0))
        .into_any(),
        None => crate::review::markdown::render_linked(&body, state).into_any(),
    };
    let author = message.author.clone();
    let mut after = vec![when_label(app, message.created_at, face, "comment-when").into_any()];
    if private {
        after.push(perch_only_label().into_any());
    }
    let avatar = author_avatar(app, state, &author, face.avatar());
    let byline = byline_row(
        name_label(move || author_name(app, &author), move |s| name_style(s, face)),
        after,
        actions,
    );
    cards::track_hover(comment_frame(face, place, avatar, byline, content), hovered).into_any()
}

/// Post to GitHub, while the thread has somewhere on GitHub to go, and
/// Copy, at the end of an agent's byline, under the pointer only, as a
/// comment's are. Post opens the composer under the thread's last
/// message, bound for GitHub, with the answer as the text: a follow-up
/// the reader edits and sends as their own, a draft where the thread
/// takes one and an issue comment where it has no reply to make. Nothing
/// remembers where it came from.
fn agent_actions(
    state: ReviewState,
    thread: CommentThread,
    body: String,
    hovered: RwSignal<bool>,
) -> floem::AnyView {
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    let draft = body.clone();
    let post: Rc<dyn Fn()> = Rc::new(move || {
        state.open_composer_with(
            Some(thread_id.clone()),
            anchor.clone(),
            draft.clone(),
            Destination::GitHub,
            None,
        );
    });
    let copy: Rc<dyn Fn()> = Rc::new(move || crate::ui::widgets::copy_to_clipboard(body.clone()));
    // Absent, not greyed, where the message has nowhere to go: an item
    // that is not a pull request at all, or a closed one under a thread
    // whose draft would need a review. The pull request lands after the
    // threads, so this follows it rather than the message's own rebuild.
    let offered = Memo::new(move |_| state.github_offered_on(&thread));
    Stack::horizontal((
        dyn_container(
            move || offered.get(),
            move |offered| {
                if !offered {
                    return Empty::new().into_any();
                }
                cards::tip_with(
                    hover_icon(hovered, SidebarIcon::PullRequest, "post-to-github", post.clone()),
                    || "Post to GitHub".to_string(),
                )
                .into_any()
            },
        ),
        cards::tip_with(
            hover_icon(hovered, SidebarIcon::Copy, "copy-message", copy),
            || "Copy".to_string(),
        ),
    ))
    .debug_name("agent-actions")
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any()
}

/// A comment from GitHub in a thread, under its author's face and
/// byline, with the reactions on it under its text. Reply opens the
/// composer under the thread, bound for GitHub, whatever the thread's
/// last message was, while it has somewhere on GitHub to go; Ask about
/// this opens it to an agent, so the answer lands beside the comment it
/// is about; the thread's history carries the comment to the agent, and
/// nothing is quoted.
#[allow(clippy::too_many_arguments)]
fn github_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    message: &CommentMessage,
    login: String,
    face: ThreadFace,
    place: Place,
) -> floem::AnyView {
    let hovered = RwSignal::new(false);
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    // To an agent, whatever the thread's last message was: the point is
    // to ask about the comment, not to answer it.
    let ask: Rc<dyn Fn()> = Rc::new(move || {
        state.open_composer_with(
            Some(thread_id.clone()),
            anchor.clone(),
            String::new(),
            Destination::agent(state.agent_last.get_untracked().unwrap_or_else(|| defaults(app))),
            None,
        );
    });
    // To GitHub, whatever the thread's last message was: an answer to
    // the comment, under the agent's take on it if there is one. Absent
    // where it has nowhere to go, as Post to GitHub is.
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    let reply_to: Rc<dyn Fn()> = Rc::new(move || {
        state.open_composer_with(
            Some(thread_id.clone()),
            anchor.clone(),
            String::new(),
            Destination::GitHub,
            None,
        );
    });
    let for_reply = thread.clone();
    let offered = Memo::new(move |_| state.github_offered_on(&for_reply));
    let reply_button = dyn_container(
        move || offered.get(),
        move |offered| {
            if !offered {
                return Empty::new().into_any();
            }
            cards::text_action(hovered, SidebarIcon::Reply, "Reply", "reply-github", reply_to.clone())
                .into_any()
        },
    )
    .into_any();
    let react = message.node_id.as_deref().and_then(cards::reactable);
    let url = message.html_url.clone().unwrap_or_default();
    let reactions = cards::reactions_under(state, backend.clone(), &react);
    let name = login.clone();
    let author = login.clone();
    let byline = byline_row(
        name_label(move || Name::Login(name.clone()), move |s| name_style(s, face)),
        vec![
            when_label(app, message.created_at, face, "comment-when").into_any(),
            author_label(state, move || Some(author.clone())).into_any(),
        ],
        cards::actions(state, backend, hovered, Some(reply_button), Some(ask), react, url),
    );
    let avatar = crate::ui::widgets::avatar(
        app,
        cards::avatar_url_of(state, login.clone()),
        face.avatar(),
        login,
    )
    .into_any();
    let body = Stack::vertical((
        crate::review::markdown::render_linked(&message.body, state),
        reactions,
    ))
    .style(|s| s.flex_col().width_full())
    .into_any();
    cards::track_hover(comment_frame(face, place, avatar, byline, body), hovered).into_any()
}

/// The reader's message on its way to GitHub, under their face and name
/// with the Draft mark after it and no time, since it has not gone
/// anywhere yet; edit and delete on hover, and the text. Edit opens the
/// composer in this row with the text; delete asks nothing, a draft being
/// Perch's alone.
fn draft_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    message: &CommentMessage,
    face: ThreadFace,
    place: Place,
) -> floem::AnyView {
    let hovered = RwSignal::new(false);
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    let (edit_id, edit_body) = (message.id.clone(), message.body.clone());
    let edit: Rc<dyn Fn()> = Rc::new(move || {
        state.open_composer_with(
            Some(thread_id.clone()),
            anchor.clone(),
            edit_body.clone(),
            Destination::GitHub,
            Some(edit_id.clone()),
        );
    });
    let delete_id = message.id.clone();
    let delete: Rc<dyn Fn()> = Rc::new(move || {
        let Some(item_id) = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone())) else {
            return;
        };
        backend.delete_draft(&item_id, &delete_id);
    });
    let actions = Stack::horizontal((
        cards::tip_with(
            hover_icon(hovered, SidebarIcon::Pencil, "edit-draft", edit),
            || "Edit".to_string(),
        ),
        cards::tip_with(
            hover_icon(hovered, SidebarIcon::Trash, "delete-draft", delete),
            || "Delete".to_string(),
        ),
    ))
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any();
    let byline = byline_row(
        name_label(move || author_name(app, &Author::User), move |s| name_style(s, face)),
        vec![
            draft_pill().into_any(),
            author_label(state, move || viewer_login(app)).into_any(),
        ],
        actions,
    );
    let body = crate::review::markdown::render_linked(&message.body, state).into_any();
    let avatar = viewer_avatar(app, face.avatar()).into_any();
    cards::track_hover(comment_frame(face, place, avatar, byline, body), hovered).into_any()
}

/// A comment of the reader's that is on GitHub, under their face and
/// name with the comment's time, its link and the reaction picker on
/// hover, and the reactions on it under its text. No Draft mark, no edit
/// and no delete: what is on github.com is edited there.
fn posted_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    message: &CommentMessage,
    face: ThreadFace,
    place: Place,
) -> floem::AnyView {
    let hovered = RwSignal::new(false);
    let react = message.node_id.as_deref().and_then(cards::reactable);
    let url = message.html_url.clone().unwrap_or_default();
    let reactions = cards::reactions_under(state, backend.clone(), &react);
    let byline = byline_row(
        name_label(move || author_name(app, &Author::User), move |s| name_style(s, face)),
        vec![
            when_label(app, message.created_at, face, "posted-when").into_any(),
            author_label(state, move || viewer_login(app)).into_any(),
        ],
        cards::actions(state, backend, hovered, None, None, react, url),
    );
    let body = Stack::vertical((
        crate::review::markdown::render_linked(&message.body, state),
        reactions,
    ))
    .style(|s| s.flex_col().width_full())
    .into_any();
    let avatar = viewer_avatar(app, face.avatar()).into_any();
    cards::track_hover(comment_frame(face, place, avatar, byline, body), hovered).into_any()
}

/// The Draft mark after a draft's author, in GitHub's colour: this is
/// going there, and is not there yet.
pub fn draft_pill() -> impl IntoView {
    cards::tip(
        Label::derived(|| "Draft".to_string())
            .debug_name("draft-pill")
            .style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::GITHUB)
                    .background(theme::GITHUB_BG)
                    .border(1.0)
                    .border_color(theme::GITHUB_BG)
                    .border_radius(9.0)
                    .padding_horiz(6.0)
                    .margin_left(8.0)
                    .selectable(false)
            }),
        "Not on GitHub until the review is submitted.".to_string(),
    )
}

/// A small action drawn only while its message is under the pointer,
/// hidden rather than absent so the byline keeps its shape.
fn hover_icon(
    hovered: RwSignal<bool>,
    icon: SidebarIcon,
    name: &'static str,
    action: Rc<dyn Fn()>,
) -> impl IntoView {
    icon_button(move || icon, || theme::FG_SUBTLE, move || action())
        .debug_name(name)
        .style(move |s| {
            s.size(24.0, 24.0)
                .apply_if(!hovered.get(), |s| s.hide())
        })
}

/// What the agent did, under its answer or under the question while it
/// works: the steps, folded to one line until it is clicked, and the
/// footer or the Stop button.
pub fn run_block(state: ReviewState, backend: Rc<Backend>, run: RunRecord) -> impl IntoView {
    let partial = run
        .final_result
        .clone()
        .filter(|text| run.status != RunStatus::Completed && !text.trim().is_empty());
    let response = match partial {
        Some(text) => Container::new(crate::review::markdown::render_linked(&text, state))
            .style(|s| {
                s.width_full()
                    .padding_top(7.0)
                    .border_top(1.0)
                    .border_color(theme::BORDER_MUTED)
            })
            .into_any(),
        None => Empty::new().into_any(),
    };
    let changes = work_changes(state, backend.clone(), &run);
    let asks = approval_cards(state, backend.clone(), run.id.clone());
    run_surface(
        Stack::vertical((
            steps_view(state, run.clone()),
            asks,
            response,
            changes,
            run_footer(backend, run),
        ))
        .style(|s| s.flex_col().width_full().gap(6.0)),
    )
}

/// What a Work run is waiting on the reader for, a card each, with the
/// three answers. The card goes as soon as it is answered; the run's
/// steps say what was asked and what was said.
fn approval_cards(state: ReviewState, backend: Rc<Backend>, run_id: String) -> impl IntoView {
    let pending = floem::reactive::Memo::new(move |_| {
        state.approvals.with(|all| {
            all.iter()
                .filter(|a| a.run_id == run_id)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    floem::views::dyn_stack(
        move || pending.get(),
        |view| view.id.clone(),
        move |view| approval_card(state, backend.clone(), view),
    )
    .style(|s| s.flex_col().gap(8.0).width_full())
}

fn approval_card(
    state: ReviewState,
    backend: Rc<Backend>,
    view: crate::backend::ApprovalView,
) -> impl IntoView {
    use crate::backend::ApprovalAnswer;
    use crate::review::review_launch::action_button;

    let answer = {
        let (run_id, id) = (view.run_id.clone(), view.id.clone());
        Rc::new(move |how: ApprovalAnswer| {
            // Gone from the card at once, so a second click has nothing
            // to answer; the backend settles it once either way.
            state
                .approvals
                .update(|pending| pending.retain(|p| !(p.run_id == run_id && p.id == id)));
            backend.answer_approval(&run_id, &id, how);
        })
    };
    let title = view.title.clone();
    let command = view.command.clone();
    let detail = view.detail.clone();
    let (once, always, deny) = (answer.clone(), answer.clone(), answer);
    Stack::vertical((
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Warning, theme::DRAFT_YELLOW)
                .style(|s| s.size(14.0, 14.0).margin_right(6.0).flex_shrink(0.0_f32)),
            Label::derived(move || title.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
                    .min_width(0.0)
                    .text_wrap()
            }),
        ))
        .style(|s| s.items_center()),
        match command {
            Some(command) => Label::derived(move || command.clone())
                .style(|s| {
                    s.font_family(theme::CODE_FONT_STACK.to_string())
                        .font_size(theme::TEXT_BADGE)
                        .color(theme::FG)
                        .text_wrap()
                        .width_full()
                        .padding(6.0)
                        .background(theme::CANVAS)
                        .border_radius(theme::RADIUS)
                })
                .debug_name("approval-command")
                .into_any(),
            None => Empty::new().into_any(),
        },
        match detail {
            Some(detail) => Label::derived(move || detail.clone())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .text_wrap()
                        .width_full()
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        Stack::horizontal((
            action_button("Allow once", "approval-once", true, || true, move || once(ApprovalAnswer::Once)),
            action_button(&view.always, "approval-always", false, || true, move || {
                always(ApprovalAnswer::Always)
            }),
            action_button("Deny", "approval-deny", false, || true, move || deny(ApprovalAnswer::Deny)),
        ))
        .style(|s| s.items_center().gap(6.0).flex_wrap(floem::style::FlexWrap::Wrap)),
    ))
    .debug_name("approval-card")
    .style(|s| {
        s.flex_col()
            .gap(6.0)
            .width_full()
            .padding(10.0)
            .background(theme::NOTICE_BG)
            .border(1.0)
            .border_color(theme::NOTICE_BORDER)
            .border_radius(theme::RADIUS)
    })
}

/// What a finished Work run changed, `4 files changed, +120 −30`: a
/// click shows it on Files as the Run scope. Nothing for a question, for
/// a run still going, or for one that changed nothing.
fn work_changes(state: ReviewState, backend: Rc<Backend>, run: &RunRecord) -> floem::AnyView {
    let (Some(before), Some(after)) = (run.work_before.clone(), run.work_after.clone()) else {
        return Empty::new().into_any();
    };
    if run.mode != perch_github::RunMode::Work || before == after {
        return Empty::new().into_any();
    }
    let id = run.id.clone();
    if !state.run_changes.with_untracked(|m| m.contains_key(&id)) {
        backend.run_changes(&id);
    }
    let counts = floem::reactive::Memo::new(move |_| state.run_changes.with(|m| m.get(&id).copied()));
    dyn_container(
        move || counts.get(),
        move |counts| {
            let Some((files, added, removed)) = counts.filter(|(files, _, _)| *files > 0) else {
                return Empty::new().into_any();
            };
            let (before, after) = (before.clone(), after.clone());
            let backend = backend.clone();
            let noun = if files == 1 { "file" } else { "files" };
            Label::derived(move || format!("{files} {noun} changed, +{added} \u{2212}{removed}"))
                .on_event_stop(el::Click, move |_, _| {
                    let Some(item_id) = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
                    else {
                        return;
                    };
                    backend.set_scope(
                        &item_id,
                        perch_git::Scope::Run {
                            before: before.clone(),
                            after: after.clone(),
                        },
                    );
                    state.show_tab(crate::review::ItemTab::Files);
                })
                .debug_name("run-changes")
                .style(|s| {
                    clickable(s)
                        .font_size(theme::TEXT_BADGE)
                        .color(theme::ACCENT)
                        .hover(|s| s.color(theme::FG))
                })
                .into_any()
        },
    )
    .into_any()
}

/// The box a run's lines sit in. Tool activity is an inset neutral surface,
/// separate from the prose without becoming a second heavy card.
fn run_surface(inner: impl IntoView + 'static) -> impl IntoView {
    Container::new(inner).style(|s| {
        s.width_full()
            .padding(10.0)
            .border_radius(theme::RADIUS)
            .background(theme::CANVAS_SUBTLE)
    })
}

/// The footer under a run: Stop while it goes, cost and time once it is
/// done, the error and Retry when it is not.
fn run_footer(backend: Rc<Backend>, run: RunRecord) -> floem::AnyView {
    let cancel_id = run.id.clone();
    match run.status {
        RunStatus::Queued => text_button(
            || "Cancel".to_string(),
            SidebarIcon::Stop,
            move || backend.cancel_run(&cancel_id),
        )
        .into_any(),
        RunStatus::Starting | RunStatus::Running => text_button(
            || "Stop".to_string(),
            SidebarIcon::Stop,
            move || backend.cancel_run(&cancel_id),
        )
        .into_any(),
        RunStatus::Stopping => Label::new("Stopping…")
            .style(|s| s.font_size(theme::TEXT_SECTION).color(theme::FG_SUBTLE))
            .into_any(),
        RunStatus::Completed => Label::derived(move || {
            let secs = run.duration_ms.unwrap_or(0) as f64 / 1000.0;
            let cost = run
                .cost_usd
                .map(|c| format!(" \u{00b7} ${c:.2}"))
                .unwrap_or_default();
            let effort = perch_agent::Effort::parse(&run.effort)
                .map(|e| e.label().to_string())
                .unwrap_or_else(|| run.effort.clone());
            // The answer's byline names the model; the run says whether
            // it worked in the worktree, how hard it thought, how long it
            // took and what it cost.
            let work = match run.mode {
                perch_github::RunMode::Work => "Work \u{00b7} ",
                perch_github::RunMode::Ask => "",
            };
            format!("{work}{effort} \u{00b7} {secs:.0}s{cost}")
        })
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .color(theme::FG_SUBTLE)
                .selectable(false)
        })
        .into_any(),
        RunStatus::Stopped | RunStatus::Failed | RunStatus::Interrupted => {
            let error = run.error.clone().unwrap_or_else(|| "Failed".to_string());
            let retry_id = run.id.clone();
            Stack::horizontal((
                Label::derived(move || error.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::CLOSED)
                        .text_wrap()
                        .min_width(0.0)
                        .flex_grow(1.0_f32)
                }),
                text_button(
                    || "Retry".to_string(),
                    SidebarIcon::Refresh,
                    move || {
                        backend.retry_run(&retry_id);
                    },
                ),
            ))
            .style(|s| s.items_center().width_full().gap(8.0))
            .into_any()
        }
    }
}

/// The step lines, folded to "Read 4 files, ran 2 commands" until the
/// line is clicked, and open while the run goes.
fn steps_view(state: ReviewState, run: RunRecord) -> impl IntoView {
    let running = run.status.is_active();
    let run_id = run.id.clone();
    let expanded = move || running || state.expanded_runs.with(|e| e.contains(&run_id));
    let chevron_id = run.id.clone();
    let expanded_chevron = move || running || state.expanded_runs.with(|e| e.contains(&chevron_id));
    let toggle_id = run.id.clone();
    let steps = run.steps.clone();
    let summary = steps_summary(&steps);

    Stack::vertical((
        Container::new(
            Stack::horizontal((
                // The glyph for the way it points, not a turned one: vger
                // draws an SVG square to the screen, and a turn would only
                // move it.
                crate::ui::widgets::icon_in(
                    move || {
                        let icon = if expanded_chevron() {
                            SidebarIcon::ChevronDown
                        } else {
                            SidebarIcon::ChevronRight
                        };
                        crate::icons::sidebar_icon(icon, theme::FG_SUBTLE)
                    },
                    || theme::FG_SUBTLE,
                )
                .style(|s| s.size(10.0, 10.0).margin_right(5.0)),
                Label::derived(move || {
                    if running && steps.is_empty() {
                        "Working…".to_string()
                    } else {
                        summary.clone()
                    }
                })
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .selectable(false)
                }),
            ))
            .style(|s| s.items_center()),
        )
        .on_event_stop(el::Click, move |_, _| {
            state.expanded_runs.update(|e| {
                if !e.remove(&toggle_id) {
                    e.insert(toggle_id.clone());
                }
            })
        })
        .style(|s| clickable(s).width_full()),
        dyn_container(expanded, move |open| {
            if !open {
                return Empty::new().into_any();
            }
            let steps = run.steps.clone();
            dyn_stack(
                move || steps.clone().into_iter().enumerate(),
                |(i, _)| *i,
                |(_, step)| {
                    Label::derived(move || step.summary.clone()).style(move |s| {
                        s.font_size(theme::TEXT_BADGE)
                            .font_family(theme::CODE_FONT_STACK.to_string())
                            .color(if step.ok {
                                theme::FG_MUTED
                            } else {
                                theme::CLOSED
                            })
                            .text_ellipsis()
                            .width_full()
                    })
                },
            )
            .style(|s| s.flex_col().width_full().gap(2.0).padding_left(15.0))
            .into_any()
        }),
    ))
    .style(|s| s.flex_col().width_full().gap(6.0))
}

/// "Read 3 files, ran 1 command", from the step lines.
pub fn steps_summary(steps: &[perch_github::RunStep]) -> String {
    let count = |prefix: &str| {
        steps
            .iter()
            .filter(|s| s.summary.starts_with(prefix))
            .count()
    };
    let mut parts = Vec::new();
    let reads = count("Read ");
    let runs = count("Ran ");
    let searches = count("Searched ") + count("Listed ");
    let denied = steps.iter().filter(|s| !s.ok).count();
    if reads > 0 {
        parts.push(format!(
            "read {reads} {}",
            if reads == 1 { "file" } else { "files" }
        ));
    }
    if runs > 0 {
        parts.push(format!(
            "ran {runs} {}",
            if runs == 1 { "command" } else { "commands" }
        ));
    }
    if searches > 0 {
        parts.push(format!(
            "{searches} {}",
            if searches == 1 { "search" } else { "searches" }
        ));
    }
    if denied > 0 {
        parts.push(format!("{denied} not allowed"));
    }
    if parts.is_empty() {
        return if steps.is_empty() {
            "No steps".to_string()
        } else {
            format!("{} steps", steps.len())
        };
    }
    let mut text = parts.join(", ");
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    text
}

/// The Follow up box at the foot of a thread, where GitHub has Reply: a
/// click opens the composer in its place.
pub fn reply_line(
    app: AppState,
    state: ReviewState,
    thread: CommentThread,
    face: ThreadFace,
) -> impl IntoView {
    let running = thread.running_run().is_some();
    if running {
        return Empty::new().into_any();
    }
    let id = thread.id.clone();
    let anchor = thread.anchor.clone();
    Label::derived(|| "Follow up…".to_string())
        .on_event_stop(el::Click, move |_, _| {
            let already = state.composer.with_untracked(|c| {
                c.as_ref()
                    .is_some_and(|c| c.thread_id.as_deref() == Some(id.as_str()))
            });
            if !already {
                state.open_composer(
                    Some(id.clone()),
                    anchor.clone(),
                    String::new(),
                    defaults(app),
                );
            }
        })
        .style(move |s| {
            let (height, size) = match face {
                ThreadFace::Card => (theme::THREAD_CONTROL, theme::TEXT_BODY),
                ThreadFace::Inline => (theme::THREAD_CONTROL - 4.0, theme::TEXT_META),
            };
            s.font_size(size)
                .color(theme::FG_SUBTLE)
                .width_full()
                .height(height)
                .padding_horiz(12.0)
                .items_center()
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .selectable(false)
                .hover(|s| s.border_color(theme::FG_SUBTLE).cursor(CursorStyle::Text))
        })
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_run_s_request_has_a_card_whose_buttons_answer_it() {
        use floem::headless::{HeadlessHarness, TestRoot};

        let state = ReviewState::new();
        state.approvals.set(vec![crate::backend::ApprovalView {
            run_id: "run-1".into(),
            id: "a1".into(),
            item_id: "i".into(),
            thread_id: "t".into(),
            title: "Run outside the sandbox".into(),
            command: Some("git commit -m x".into()),
            detail: Some("The sandbox blocked .git/index.lock".into()),
            always: "Always let Work commit here".into(),
        }]);
        let run = perch_github::RunRecord {
            id: "run-1".into(),
            status: perch_github::RunStatus::Running,
            mode: perch_github::RunMode::Work,
            ..Default::default()
        };
        let (backend, mut commands) = Backend::recording();
        let root = TestRoot::new();
        let view = run_block(state, Rc::new(backend), run);
        let mut harness = HeadlessHarness::new_with_size(root, view, 700.0, 400.0);
        harness.rebuild();
        harness.paint();
        let find = |harness: &HeadlessHarness, name: &str| {
            fn walk(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
                if id.debug_name().split_whitespace().any(|part| part == name) {
                    return Some(id);
                }
                id.children().into_iter().find_map(|child| walk(child, name))
            }
            walk(harness.root_id(), name)
        };
        assert!(find(&harness, "approval-card").is_some());
        assert!(find(&harness, "approval-command").is_some());
        let always = find(&harness, "approval-always").unwrap().get_visual_rect_no_clip().center();
        harness.click(always.x, always.y);
        harness.rebuild();
        harness.paint();
        let mut answered = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let crate::backend::Command::AnswerApproval { run_id, approval_id, answer } = command {
                answered.push((run_id, approval_id, answer));
            }
        }
        assert_eq!(
            answered,
            [("run-1".to_string(), "a1".to_string(), crate::backend::ApprovalAnswer::Always)]
        );
        assert!(state.approvals.with(Vec::is_empty));
        assert!(find(&harness, "approval-card").is_none(), "answered, the card goes");
    }
    use crate::backend::Command;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::{Code, Key, KeyState, KeyboardEvent, NamedKey};
    use perch_github::RunStep;
    use std::sync::Arc;

    fn step(summary: &str, ok: bool) -> RunStep {
        RunStep {
            summary: summary.into(),
            ok,
        }
    }

    #[test]
    fn steps_are_summarised_by_kind() {
        let steps = vec![
            step("Read a.rs", true),
            step("Read b.rs", true),
            step("Ran git log", true),
            step("Asked to edit a.rs: not allowed", false),
        ];
        assert_eq!(
            steps_summary(&steps),
            "Read 2 files, ran 1 command, 1 not allowed"
        );
        assert_eq!(steps_summary(&[]), "No steps");
        assert_eq!(steps_summary(&[step("Read a.rs", true)]), "Read 1 file");
    }

    #[test]
    fn aliases_are_capitalised_and_ids_left_alone() {
        assert_eq!(model_label("sonnet"), "Sonnet");
        assert_eq!(model_label("fable"), "Fable");
        assert_eq!(model_label("claude-sonnet-5"), "claude-sonnet-5");
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

    /// Enter breaks the line and Cmd-Enter sends. The keys go through the
    /// box rather than to the handler: floem's own keymap decides what a
    /// key is a command for, and Cmd-Enter is not Enter with a modifier
    /// to it.
    #[test]
    fn enter_breaks_the_line_and_cmd_enter_sends() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item =
            perch_github::Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-1".into();
        state.item.set(Some(Arc::new(item)));
        state.open_composer(
            None,
            None,
            "why?".into(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box), "the box took the keyboard");

        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert_eq!(state.composer_draft.get(), "why?\n");
        assert!(commands.try_recv().is_err(), "Enter sent the question");

        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::Ask { question, .. }) => assert_eq!(question, "why?"),
            other => panic!("Cmd-Enter did not send: {other:?}"),
        }
        assert!(state.composer.get().is_none(), "sending closes the composer");
    }

    /// The first view whose debug name contains `text`.
    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    /// Every view whose debug name contains `text`, in tree order.
    fn find_all(id: floem::ViewId, text: &str) -> Vec<floem::ViewId> {
        let mut found = Vec::new();
        if id.debug_name().contains(text) {
            found.push(id);
        }
        for c in id.children() {
            found.extend(find_all(c, text));
        }
        found
    }

    /// One of GitHub's threads with a comment by rohan and, after it, the
    /// reader's question and the agent's answer, which are Perch's alone.
    fn github_thread_with_private_messages() -> CommentThread {
        use perch_github::{Anchor, Side};
        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        let message = |id: &str, origin: ThreadOrigin, author: Author, body: &str| CommentMessage {
            id: id.into(),
            thread_id: "gh-31".into(),
            origin,
            author,
            body: body.into(),
            created_at: at,
            run: None,
            github_id: (origin == ThreadOrigin::GitHub).then_some(31),
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Default::default(),
        };
        CommentThread {
            id: "gh-31".into(),
            worktree_id: "item-1".into(),
            origin: ThreadOrigin::GitHub,
            github_id: Some(31),
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: 7,
                line: 7,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: at,
            messages: vec![
                message("gh-31", ThreadOrigin::GitHub, Author::GitHubUser("rohan".into()), "epoch"),
                message("q1", ThreadOrigin::Local, Author::User, "why?"),
                message(
                    "a1",
                    ThreadOrigin::Local,
                    Author::Agent { backend: "claude-code".into(), model: "sonnet".into() },
                    "because",
                ),
            ],
        }
    }

    /// The delete under a thread of GitHub's takes Perch's messages and
    /// leaves the thread: the confirmation says so, and Delete asks the
    /// backend for that and not for the thread.
    #[test]
    fn deleting_under_a_github_thread_takes_perchs_messages_and_not_the_thread() {
        let root = TestRoot::new();
        let state = ReviewState::new();
        let thread = github_thread_with_private_messages();
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = delete_confirmation(state, Rc::new(backend), thread.id.clone(), thread.is_github_bound());
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Delete Perch's messages?").is_none(), "nothing asked yet");

        state.confirm_thread.set(Some("gh-31".into()));
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Delete Perch's messages?").is_some());
        assert!(
            find(root_id, "The questions and answers asked here go. Nothing on GitHub is touched.").is_some()
        );
        let delete = find(root_id, "Label: \"Delete\"").expect("a Delete button");
        let c = delete.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        match commands.try_recv() {
            Ok(Command::DeletePrivateMessages { thread_id }) => assert_eq!(thread_id, "gh-31"),
            other => panic!("Delete did not ask for Perch's messages: {other:?}"),
        }
        assert_eq!(state.confirm_thread.get(), None);
    }

    /// The menu is offered on a thread of GitHub's only while Perch has
    /// put something under it; with nothing of Perch's there, there is
    /// nothing it could delete.
    #[test]
    fn a_github_thread_has_a_menu_only_while_perch_has_messages_under_it() {
        let root = TestRoot::new();
        let state = ReviewState::new();
        let mut thread = github_thread_with_private_messages();
        let with = thread_menu(state, &thread, theme::THREAD_MENU);
        thread.messages.truncate(1);
        let without = thread_menu(state, &thread, theme::THREAD_MENU);
        let view = floem::views::Stack::vertical((
            with.debug_name("menu-with"),
            without.debug_name("menu-without"),
        ));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let with = find(harness.root_id(), "menu-with").unwrap();
        let without = find(harness.root_id(), "menu-without").unwrap();
        assert_eq!(harness.get_layout_rect(with).height(), theme::THREAD_MENU);
        assert_eq!(harness.get_layout_rect(without).height(), 0.0);
    }

    fn key(key: Key, code: Code, modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    /// Option-Left and Option-Right move the caret a word at a time, as in
    /// any native box, and the box keeps the keyboard through them. Alt
    /// with an arrow is also the key floem walks the focus with, and a
    /// button beside the box used to take it, with the next keystroke.
    #[test]
    fn option_arrows_move_the_caret_by_a_word_and_keep_the_keyboard() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item =
            perch_github::Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-1".into();
        state.item.set(Some(Arc::new(item)));
        state.open_composer(
            None,
            None,
            "why not now".into(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        let composer = state.composer.get().unwrap();
        let (backend, _commands) = crate::backend::Backend::recording();
        let composer = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        // Somewhere for the focus to go on either side, as the window has.
        let button = |label: &'static str| {
            floem::views::Button::new(label).style(|s| s.keyboard_navigable().width(80.0))
        };
        let view =
            Stack::horizontal((button("left"), composer, button("right"))).style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box), "the box took the keyboard");

        let alt = |code: Code, name: NamedKey| key(Key::Named(name), code, Modifiers::ALT);
        let typed = |ch: &str, code: Code| key(Key::Character(ch.into()), code, Modifiers::empty());

        // The draft is typed under, so the caret starts at its end.
        harness.dispatch_event(alt(Code::ArrowLeft, NamedKey::ArrowLeft));
        harness.dispatch_event(typed("X", Code::KeyX));
        harness.rebuild();
        assert_eq!(
            state.composer_draft.get(),
            "why not Xnow",
            "back to the start of the word"
        );
        assert!(
            harness.is_focused(text_box),
            "and the box kept the keyboard"
        );

        harness.dispatch_event(alt(Code::ArrowRight, NamedKey::ArrowRight));
        harness.dispatch_event(typed("Y", Code::KeyY));
        harness.rebuild();
        assert_eq!(
            state.composer_draft.get(),
            "why not XnowY",
            "forward to the end of the word"
        );
        assert!(harness.is_focused(text_box));

        // Three words back is the front, and one more has nowhere to go.
        for _ in 0..4 {
            harness.dispatch_event(alt(Code::ArrowLeft, NamedKey::ArrowLeft));
        }
        harness.dispatch_event(typed("A", Code::KeyA));
        harness.rebuild();
        assert_eq!(state.composer_draft.get(), "Awhy not XnowY");
        assert!(
            harness.is_focused(text_box),
            "the box keeps the keyboard at its front too"
        );
    }

    /// `@` puts up the item's people and the worktree's files under the
    /// caret. Enter takes the pick, a press on a row takes that row, and
    /// Escape puts the list away and leaves the composer open.
    #[test]
    fn an_at_offers_people_and_files_and_puts_the_pick_in_the_box() {
        let root = TestRoot::new();
        let app = AppState::new();
        app.window_width.set(800.0);
        app.window_height.set(400.0);
        let state = ReviewState::new();
        let mut item =
            perch_github::Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-1".into();
        state.item.set(Some(Arc::new(item)));
        let comment = |id: u64, user: &str| perch_github::IssueComment {
            id,
            node_id: String::new(),
            user: user.into(),
            user_avatar_url: String::new(),
            body: "hi".into(),
            created_at: "2026-09-20T09:00:00Z".parse().unwrap(),
            updated_at: None,
            html_url: String::new(),
            reactions: Default::default(),
        };
        state
            .issue_comments
            .set(Arc::new(vec![comment(1, "cadonna"), comment(2, "agavra")]));
        let entry = |path: &str| perch_git::TreeEntry {
            path: path.into(),
            is_dir: false,
            depth: path.matches('/').count(),
        };
        state
            .tree
            .set(Arc::new(vec![entry("src/lib.rs"), entry("src/client.rs")]));
        state.open_composer(
            None,
            None,
            String::new(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let backend = Rc::new(backend);
        let composer = composer_view(app, state, backend.clone(), composer, ThreadFace::Card);
        let view = Stack::vertical((
            composer,
            crate::review::mention::popup(app, state),
        ))
        .style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));

        let typed = |ch: &str| {
            let code = match ch {
                "@" => Code::Digit2,
                _ => Code::KeyA,
            };
            key(Key::Character(ch.into()), code, Modifiers::empty())
        };
        let named = |name: NamedKey, code: Code| key(Key::Named(name), code, Modifiers::empty());
        let shown = || {
            state.mentions.shown.with(|s| {
                s.as_ref().map(|s| {
                    s.suggestions
                        .iter()
                        .map(|s| match s {
                            crate::review::mention::Suggestion::Person(p) => {
                                format!("@{}", p.person.login)
                            }
                            crate::review::mention::Suggestion::File(f) => f.path.clone(),
                        })
                        .collect::<Vec<_>>()
                })
            })
        };

        for ch in ["h", "i", " "] {
            harness.dispatch_event(typed(ch));
        }
        harness.rebuild();
        assert_eq!(shown(), None, "no list without an @");
        harness.dispatch_event(typed("@"));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            shown().unwrap(),
            ["@cadonna", "@agavra"],
            "the item's people; nothing is open or changed, so no files yet"
        );
        // Under the @, inside the window.
        let list = find_mention_row(&harness, "mentions").get_visual_rect_no_clip();
        let text = text_box.get_visual_rect_no_clip();
        assert!(list.y0 > text.y0 && list.x0 > text.x0 + 10.0, "{list:?} {text:?}");
        assert!(list.x1 <= 800.0 && list.y1 <= 400.0, "{list:?}");
        assert!(commands.try_recv().is_err(), "nothing typed is not asked of GitHub");

        // Letters ask GitHub once typing rests, about the last of them;
        // meanwhile the list has who matches here, then the files.
        harness.dispatch_event(typed("c"));
        harness.dispatch_event(typed("a"));
        harness.rebuild();
        assert!(commands.try_recv().is_err(), "nothing is asked while typing");
        let ask = |query: &str| crate::review::mention::Ask {
            item_id: "item-1".into(),
            repo: "acme/repo".into(),
            query: query.into(),
        };
        // The timer for `c` finds typing moved on, and asks nothing.
        state.mentions.ask_if_waiting(&backend, &ask("c"));
        assert!(commands.try_recv().is_err(), "a query typed through is not asked");
        harness.dispatch_event(named(NamedKey::Backspace, Code::Backspace));
        harness.rebuild();
        state.mentions.ask_if_waiting(&backend, &ask("c"));
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::MentionableUsers { item_id, repo_full_name, query })
                if item_id == "item-1" && repo_full_name == "acme/repo" && query == "c"
        ));
        assert_eq!(shown().unwrap(), ["@cadonna", "src/client.rs", "src/lib.rs"]);

        // GitHub's answer joins the list after the item's own.
        state.mentions.install(
            "c".into(),
            vec![
                perch_github::MentionableUser {
                    login: "calavera".into(),
                    name: Some("David Calavera".into()),
                    avatar_url: String::new(),
                },
                perch_github::MentionableUser {
                    login: "cadonna".into(),
                    name: Some("Bruno Cadonna".into()),
                    avatar_url: String::new(),
                },
            ],
        );
        harness.rebuild();
        assert_eq!(
            shown().unwrap(),
            ["@cadonna", "@calavera", "src/client.rs", "src/lib.rs"]
        );

        // Down to calavera, and Enter puts them in the box, where the
        // list goes away.
        harness.dispatch_event(named(NamedKey::ArrowDown, Code::ArrowDown));
        assert_eq!(state.mentions.selected.get(), 1);
        harness.dispatch_event(named(NamedKey::Enter, Code::Enter));
        harness.rebuild();
        assert_eq!(state.composer_draft.get(), "hi @calavera ");
        assert_eq!(shown(), None);

        // A path goes to an agent as `@path`; a press on its row takes it.
        for ch in ["@", "l", "i", "b"] {
            harness.dispatch_event(typed(ch));
        }
        harness.rebuild();
        harness.paint();
        assert_eq!(shown().unwrap(), ["src/lib.rs"]);
        let row = find_mention_row(&harness, "mention-row:src/lib.rs");
        let at = harness.get_layout_rect(row);
        assert!(at.width() > 0.0, "the row is drawn");
        let centre = row.get_visual_rect_no_clip().center();
        harness.click(centre.x, centre.y);
        harness.rebuild();
        assert_eq!(state.composer_draft.get(), "hi @calavera @src/lib.rs ");
        assert_eq!(shown(), None);
        assert!(harness.is_focused(text_box), "the box has the keyboard back");

        // Escape puts the list away, not the composer, and it stays away
        // for this @ as more is typed.
        harness.dispatch_event(typed("@"));
        harness.rebuild();
        assert!(shown().is_some());
        harness.dispatch_event(named(NamedKey::Escape, Code::Escape));
        harness.rebuild();
        assert_eq!(shown(), None);
        assert!(state.composer.get().is_some(), "the composer is still open");
        harness.dispatch_event(typed("a"));
        harness.rebuild();
        assert_eq!(shown(), None);
    }

    fn find_mention_row(harness: &HeadlessHarness, name: &str) -> floem::ViewId {
        fn walk(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
            if id.debug_name().contains(name) {
                return Some(id);
            }
            id.children().into_iter().find_map(|c| walk(c, name))
        }
        walk(harness.root_id(), name).expect("the row is in the tree")
    }

    #[test]
    fn a_fresh_composer_starts_on_a_model_that_is_offered() {
        use perch_agent::{Backend, Effort};
        let mut agent = perch_github::AgentConfig {
            model: "sonnet".into(),
            effort: "ultra".into(),
            ..Default::default()
        };
        assert_eq!(
            default_choice(&agent),
            (Backend::ClaudeCode, "sonnet".to_string(), Effort::Ultra)
        );

        // Claude Code is not installed: its aliases are gone, and Codex is.
        agent.models.clear();
        agent.codex.models = vec!["gpt-5.5".into(), "gpt-6-astra".into()];
        agent.codex.efforts.insert(
            "gpt-5.5".into(),
            ["low", "xhigh"].map(String::from).to_vec(),
        );
        assert_eq!(
            default_choice(&agent),
            (Backend::Codex, "gpt-5.5".to_string(), Effort::XHigh)
        );

        // Nothing at all: config's choice stands, and the run will say so.
        agent.codex.models.clear();
        assert_eq!(
            default_choice(&agent),
            (Backend::ClaudeCode, "sonnet".to_string(), Effort::Ultra)
        );
    }

    #[test]
    fn picking_a_model_without_the_effort_drops_to_the_highest_it_takes() {
        use perch_agent::Effort;
        let mut agent = perch_github::AgentConfig::default();
        agent.codex.efforts.insert(
            "gpt-5.5".into(),
            ["low", "medium", "high", "xhigh"]
                .map(String::from)
                .to_vec(),
        );
        agent
            .codex
            .efforts
            .insert("odd".into(), vec!["medium".into()]);
        assert_eq!(
            nearest_effort(&agent, "gpt-5.5", Effort::Ultra),
            Effort::XHigh
        );
        assert_eq!(
            nearest_effort(&agent, "gpt-5.5", Effort::High),
            Effort::High,
            "a level it takes is kept"
        );
        assert_eq!(
            nearest_effort(&agent, "odd", Effort::Low),
            Effort::Medium,
            "even upward, to the one it has"
        );
        assert_eq!(
            nearest_effort(&agent, "sonnet", Effort::Ultra),
            Effort::Ultra,
            "no catalog, no doubt"
        );
    }

    #[test]
    fn the_picker_lists_claude_then_codex_and_each_model_knows_its_backend() {
        use perch_agent::Backend;
        let agent = perch_github::AgentConfig {
            models: vec!["opus".into(), "sonnet".into()],
            codex: perch_github::CodexConfig {
                binary: "codex".into(),
                models: vec!["gpt-5.6-sol".into()],
                ..Default::default()
            },
            ..perch_github::AgentConfig::default()
        };
        assert_eq!(
            model_groups(&agent),
            vec![
                vec![
                    (Backend::ClaudeCode, "opus".to_string()),
                    (Backend::ClaudeCode, "sonnet".to_string())
                ],
                vec![(Backend::Codex, "gpt-5.6-sol".to_string())],
            ]
        );

        let claude_only = perch_github::AgentConfig {
            models: vec!["sonnet".into()],
            ..perch_github::AgentConfig::default()
        };
        assert_eq!(
            model_groups(&claude_only),
            vec![vec![(Backend::ClaudeCode, "sonnet".to_string())]],
            "no separator without Codex"
        );

        let shared = perch_github::AgentConfig {
            models: vec!["shared".into()],
            codex: perch_github::CodexConfig {
                binary: "codex".into(),
                models: vec!["shared".into()],
                ..Default::default()
            },
            ..perch_github::AgentConfig::default()
        };
        assert_eq!(
            model_groups(&shared)[1],
            vec![(Backend::ClaudeCode, "shared".to_string())],
            "config decides, not the section"
        );
    }

    fn sonnet_medium() -> AgentChoice {
        (
            perch_agent::Backend::ClaudeCode,
            "sonnet".into(),
            perch_agent::Effort::Medium,
        )
    }

    /// The reader's question under the pull request, as `a` opens it.
    fn state_with_item() -> ReviewState {
        let state = ReviewState::new();
        let mut item =
            perch_github::Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-1".into();
        state.item.set(Some(Arc::new(item)));
        state
    }

    /// `a` on a selection opens the composer on the lines, on an agent.
    /// Picking GitHub turns the send into Add to review, takes the effort
    /// picker away, and sending asks the backend to keep a draft on those
    /// lines with no thread; the face remembers, so the next composer on
    /// lines opens on GitHub. On lines of a branch, or of a pull request
    /// no longer open, GitHub is not offered and the composer opens on an
    /// agent whatever the face remembers: a draft there could never post.
    #[test]
    fn a_composer_on_lines_sent_to_github_keeps_a_draft_with_the_anchor_and_no_thread() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let anchor = perch_github::Anchor {
            path: "src/x.rs".into(),
            start_line: 7,
            line: 8,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        state.open_composer(None, Some(anchor.clone()), "please".into(), sonnet_medium());
        let composer = state.composer.get().unwrap();
        assert_eq!(composer.destination, Destination::agent(sonnet_medium()));
        assert!(state.github_offered(&composer));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Label: \"Send\"").is_some());
        assert!(find(root_id, "Label: \"Medium\"").is_some(), "the effort picker");

        // As the picker's GitHub entry does.
        state.composer.update(|c| {
            if let Some(c) = c {
                c.destination = Destination::GitHub;
            }
        });
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Label: \"Add to review\"").is_some());
        assert!(find(root_id, "Label: \"GitHub\"").is_some());
        assert!(find(root_id, "Label: \"Medium\"").is_none(), "no effort for GitHub");
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box), "the flip kept the keyboard");

        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::SaveDraft {
                item_id,
                thread_id,
                anchor: on,
                body,
                editing,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(thread_id, None);
                assert_eq!(on, Some(anchor.clone()));
                assert_eq!(body, "please");
                assert_eq!(editing, None);
            }
            other => panic!("Cmd-Enter did not keep a draft: {other:?}"),
        }
        assert!(state.composer.get().is_none(), "sending closes the composer");
        assert_eq!(
            state.last_destination[ThreadFace::Card.index()].get(),
            Some(Destination::GitHub)
        );
        assert_eq!(state.agent_last.get(), None, "no agent was sent to");

        state.open_composer(None, Some(anchor.clone()), String::new(), sonnet_medium());
        assert_eq!(state.composer.get().unwrap().destination, Destination::GitHub);
        state.open_composer(None, None, String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::agent(sonnet_medium()),
            "about the item as a whole, an agent"
        );

        // A branch: no review to add to.
        state.pull_request.set(None);
        state.open_composer(None, Some(anchor.clone()), String::new(), sonnet_medium());
        let composer = state.composer.get().unwrap();
        assert!(!state.github_offered(&composer), "a branch takes no draft");
        assert_eq!(composer.destination, Destination::agent(sonnet_medium()));

        // A pull request that was closed: the same.
        let mut closed = pull();
        closed.state = "closed".into();
        closed.closed_at = Some("2026-09-20T10:00:00Z".parse().unwrap());
        state.pull_request.set(Some(Arc::new(closed)));
        state.open_composer(None, Some(anchor), String::new(), sonnet_medium());
        let composer = state.composer.get().unwrap();
        assert!(!state.github_offered(&composer), "a closed pull request takes no draft");
        assert_eq!(composer.destination, Destination::agent(sonnet_medium()));
        state.open_composer(Some("gh-31".into()), None, String::new(), sonnet_medium());
        assert!(
            !state.github_offered(&state.composer.get().unwrap()),
            "nor a reply on its thread"
        );
    }

    /// A draft under GitHub's thread: the reader's byline with the Draft
    /// mark, edit and delete under the pointer and no Ask about this;
    /// delete asks the backend straight away, edit opens the composer in
    /// its row with the text, bound for GitHub.
    #[test]
    fn a_draft_draws_with_the_draft_mark_and_edit_and_delete_under_the_pointer() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let mut thread = github_thread_with_private_messages();
        thread.messages.truncate(1);
        let mut draft = thread.messages[0].clone();
        draft.id = "d1".into();
        draft.author = Author::User;
        draft.github_id = None;
        draft.body = "I will fix it".into();
        thread.messages.push(draft);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = messages_view(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let pill = find(root_id, "draft-pill").expect("the Draft mark");
        assert!(harness.get_layout_rect(pill).width() > 0.0);
        let edit = find(root_id, "edit-draft").expect("edit");
        let delete = find(root_id, "delete-draft").expect("delete");
        assert_eq!(harness.get_layout_rect(edit).width(), 0.0, "hidden until hovered");

        // Under the pointer, the actions show.
        let body = find(root_id, "draft-pill").unwrap().get_visual_rect_no_clip();
        harness.pointer_move(body.center().x, body.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(harness.get_layout_rect(edit).width(), 24.0);
        assert_eq!(harness.get_layout_rect(delete).width(), 24.0);

        let c = delete.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        match commands.try_recv() {
            Ok(Command::DeleteDraft { item_id, message_id }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(message_id, "d1");
            }
            other => panic!("delete did not ask for the draft: {other:?}"),
        }

        let c = edit.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("edit opened the composer");
        assert_eq!(composer.editing.as_deref(), Some("d1"));
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(composer.anchor, thread.anchor);
        assert_eq!(state.composer_draft.get(), "I will fix it");
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "draft-pill").is_none(), "the composer took the draft's row");
        assert!(find(root_id, "Label: \"Add to review\"").is_some());

        // Escape, with the text still in the box, gives the row back: the
        // words are safe in the cache.
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));
        harness.dispatch_event(key(Key::Named(NamedKey::Escape), Code::Escape, Modifiers::empty()));
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none(), "Escape closed the edit");
        assert!(find(harness.root_id(), "draft-pill").is_some(), "the draft is back");

        // Edited and sent: the backend is asked to rewrite that draft, in
        // its thread, and the composer closes. Nothing is remembered for
        // the face; an edit is GitHub's by nature, not by choice.
        let root_id = harness.root_id();
        let pill = find(root_id, "draft-pill").unwrap().get_visual_rect_no_clip().center();
        harness.pointer_move(pill.x, pill.y);
        harness.rebuild();
        harness.paint();
        let edit = find(root_id, "edit-draft").expect("edit, on the draft's row again");
        let c = edit.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        harness.rebuild();
        harness.paint();
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));
        harness.dispatch_event(key(Key::Character("a".into()), Code::KeyA, Modifiers::META));
        harness.dispatch_event(key(Key::Named(NamedKey::Backspace), Code::Backspace, Modifiers::empty()));
        for ch in "I fixed it".chars() {
            harness.dispatch_event(key(Key::Character(ch.to_string().into()), Code::KeyA, Modifiers::empty()));
        }
        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::SaveDraft { item_id, thread_id, anchor, body, editing }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(thread_id.as_deref(), Some("gh-31"));
                assert_eq!(anchor, thread.anchor);
                assert_eq!(body, "I fixed it");
                assert_eq!(editing.as_deref(), Some("d1"));
            }
            other => panic!("Cmd-Enter did not rewrite the draft: {other:?}"),
        }
        assert!(state.composer.get().is_none(), "sending closes the edit");
    }

    /// A comment of the reader's that posted: their byline with the time,
    /// the link and the reactions, and no Draft mark, edit or delete,
    /// whether it opened its thread or answered someone's; the thread it
    /// opened has no byline of its own over it.
    #[test]
    fn a_posted_comment_of_the_readers_keeps_its_byline_link_and_reactions() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut theirs = github_thread_with_private_messages();
        let mut posted = theirs.messages[0].clone();
        posted.id = "d1".into();
        posted.author = Author::User;
        posted.github_id = Some(88);
        posted.node_id = Some("PRRC_88".into());
        posted.html_url = Some("https://github.com/acme/repo/pull/42#discussion_r88".into());
        posted.body = "I will fix it".into();
        theirs.messages.push(posted.clone());
        let mut ours = theirs.clone();
        ours.id = "t1".into();
        ours.github_id = Some(88);
        ours.messages = vec![CommentMessage {
            thread_id: "t1".into(),
            ..posted
        }];
        state.comment_threads.set(Arc::new(vec![theirs.clone(), ours.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let backend = Rc::new(backend);
        let view = floem::views::Stack::vertical((
            messages_view(app, state, backend.clone(), theirs.id.clone(), ThreadFace::Card)
                .debug_name("theirs"),
            cards::thread_card(app, state, backend, Rc::new(|_| {}), ours)
                .debug_name("ours"),
        ))
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 900.0);
        harness.rebuild();
        harness.paint();
        for name in ["theirs", "ours"] {
            let card = find(harness.root_id(), name).unwrap();
            assert!(find(card, "posted-when").is_some(), "{name}: the comment's time");
            assert!(find(card, "github-link").is_some(), "{name}: its link");
            assert!(find(card, "reactions").is_some(), "{name}: its reactions");
            assert!(find(card, "draft-pill").is_none(), "{name}: not a draft any more");
            assert!(find(card, "edit-draft").is_none(), "{name}: not edited here");
            assert!(find(card, "delete-draft").is_none(), "{name}: not deleted here");
            assert!(find(card, "Label: \"You\"").is_some(), "{name}: the reader's");
        }
        assert!(
            find(find(harness.root_id(), "theirs").unwrap(), "reply-github").is_some(),
            "rohan's comment offers Reply"
        );
        assert!(
            find(find(harness.root_id(), "ours").unwrap(), "You asked").is_none(),
            "the thread the reader opened on GitHub has no byline over its comment"
        );
    }

    /// Reply on a GitHub user's comment opens the composer under the
    /// thread bound for GitHub, whatever the thread ends in: here the
    /// agent's answer, under which Follow up would open on the agent.
    /// Only while the pull request is open; on a closed one it is absent.
    #[test]
    fn reply_on_a_github_comment_goes_to_github_under_the_agents_answer() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = github_thread_with_private_messages();
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = messages_view(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let reply = find(harness.root_id(), "reply-github").expect("Reply on rohan's comment");
        assert_eq!(harness.get_layout_rect(reply).width(), 0.0, "hidden until hovered");
        let at = find(harness.root_id(), "Label: \"rohan\"").unwrap().get_visual_rect_no_clip().center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert!(harness.get_layout_rect(reply).width() > 0.0);
        let c = reply.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("Reply opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(composer.anchor, thread.anchor);
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.editing, None);
        assert!(state.composer_draft.get().is_empty());
        state.close_composer();

        // Follow up under the same thread is the agent's, as before.
        state.open_composer(Some("gh-31".into()), thread.anchor.clone(), String::new(), sonnet_medium());
        assert!(!state.composer.get().unwrap().destination.is_github());
        state.close_composer();

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "reply-github").is_none(), "nowhere for a draft to go");
    }

    /// A follow-up follows the thread's last message: under rohan's
    /// comment it is bound for GitHub, under the agent's answer for that
    /// agent at its run's effort. Ask about this on rohan's comment goes
    /// to an agent whatever the thread ends in.
    #[test]
    fn a_follow_up_follows_the_last_message_and_ask_about_this_goes_to_an_agent() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = github_thread_with_private_messages();
        state.comment_threads.set(Arc::new(vec![thread.clone()]));

        // Under the agent's answer, which ends the thread: Opus at the
        // run's high, neither of which the defaults would give.
        let mut answered = thread.clone();
        let answer = answered.messages.last_mut().unwrap();
        answer.author = Author::Agent { backend: "claude-code".into(), model: "opus".into() };
        answer.run = Some(perch_github::RunRecord {
            id: "r1".into(),
            backend: "claude-code".into(),
            model: "opus".into(),
            effort: "high".into(),
            status: perch_github::RunStatus::Completed,
            ..Default::default()
        });
        state.comment_threads.set(Arc::new(vec![answered.clone()]));
        state.open_composer(Some(answered.id.clone()), answered.anchor.clone(), String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::Agent {
                backend: perch_agent::Backend::ClaudeCode,
                model: "opus".into(),
                effort: perch_agent::Effort::High,
            }
        );
        state.close_composer();

        // Under rohan's comment alone.
        let mut theirs = thread.clone();
        theirs.messages.truncate(1);
        state.comment_threads.set(Arc::new(vec![theirs.clone()]));
        state.open_composer(Some(theirs.id.clone()), theirs.anchor.clone(), String::new(), sonnet_medium());
        assert_eq!(state.composer.get().unwrap().destination, Destination::GitHub);
        state.close_composer();

        // Ask about this on rohan's comment.
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = message_view(
            app,
            state,
            Rc::new(backend),
            theirs.clone(),
            theirs.messages[0].clone(),
            ThreadFace::Card,
            Lane::Flush,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let ask = find(harness.root_id(), "Label: \"Ask about this\"").expect("Ask about this");
        let at = harness.get_layout_rect(harness.root_id()).center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        let c = ask.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("Ask about this opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(composer.destination, Destination::agent(sonnet_medium()));
        assert_eq!(composer.editing, None);
    }

    /// An agent's finished answer offers Post to GitHub and Copy at its
    /// top right, under the pointer only. Post opens the follow-up
    /// composer on the thread, bound for GitHub, with the answer's
    /// markdown as the text and nothing marked as its source; Copy puts
    /// the markdown on the clipboard.
    #[test]
    fn an_agents_answer_offers_post_to_github_and_copy_under_the_pointer() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut thread = github_thread_with_private_messages();
        let answer = thread.messages.last_mut().unwrap();
        answer.run = Some(perch_github::RunRecord {
            id: "r1".into(),
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "medium".into(),
            status: perch_github::RunStatus::Completed,
            ..Default::default()
        });
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = message_view(
            app,
            state,
            Rc::new(backend),
            thread.clone(),
            thread.messages[2].clone(),
            ThreadFace::Card,
            Lane::PrivateContinue,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let post = find(root_id, "post-to-github").expect("Post to GitHub");
        let copy = find(root_id, "copy-message").expect("Copy");
        assert_eq!(harness.get_layout_rect(post).width(), 0.0, "hidden until hovered");
        assert_eq!(harness.get_layout_rect(copy).width(), 0.0, "hidden until hovered");

        let at = harness.get_layout_rect(root_id).center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(harness.get_layout_rect(post).width(), 24.0);
        assert_eq!(harness.get_layout_rect(copy).width(), 24.0);
        // Over the message's top right corner, not in a row of its own.
        let corner = post.get_visual_rect_no_clip();
        assert!(corner.y0 < 20.0, "at the top: {corner:?}");
        assert!(corner.x1 > 600.0 - 60.0, "at the right: {corner:?}");

        let _ = crate::ui::widgets::copied();
        let c = copy.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert_eq!(crate::ui::widgets::copied(), vec!["because".to_string()]);

        let c = post.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("Post to GitHub opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.anchor, thread.anchor);
        assert_eq!(composer.editing, None, "a new draft, not an edit");
        assert_eq!(state.composer_draft.get(), "because");
    }

    /// A finding's thread is Perch's own, on the lines the reviewer
    /// pointed at, and its message carries the finding's text with no
    /// run. It offers Post to GitHub as an answer does, and keeps its
    /// text clear of the corner the actions come to, so a pointer resting
    /// on the finding to read it never puts a glyph on a word.
    #[test]
    fn a_findings_message_offers_post_to_github_beside_its_text() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = finding_thread(
            "The lock is taken twice on the retry path, once in the caller and \
             once in the helper it calls, so a second attempt deadlocks the \
             writer; the retry has to drop it before it calls back in.",
        );
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = message_view(
            app,
            state,
            Rc::new(backend),
            thread.clone(),
            thread.messages[0].clone(),
            ThreadFace::Card,
            Lane::Flush,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let post = find(root_id, "post-to-github").expect("a finding offers it");
        let at = harness.get_layout_rect(root_id).center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        let actions = find(root_id, "agent-actions").unwrap().get_visual_rect_no_clip();
        let prose = find(root_id, "RichText").expect("the finding's text").get_visual_rect_no_clip();
        assert!(prose.height() > 40.0, "the text wraps: {prose:?}");
        assert!(
            actions.intersect(prose).is_zero_area(),
            "the actions sit beside the text, not on it: {actions:?} over {prose:?}"
        );
        let c = post.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(composer.thread_id.as_deref(), Some("finding-1"));
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.anchor, thread.anchor);
        assert_eq!(state.composer_draft.get(), thread.messages[0].body);
    }

    /// Where a message from a thread can go on a pull request. Lines or
    /// a place on GitHub, either alone, take a draft, which needs the
    /// pull request open to ride in a review. Neither takes an issue
    /// comment instead, which a closed pull request still accepts, so
    /// what closing takes away is the draft and not the comment.
    #[test]
    fn a_thread_with_lines_or_a_place_on_github_takes_a_draft_and_one_with_neither_comments() {
        let state = state_with_pull_request();
        let mut lines = github_thread_with_private_messages();
        lines.origin = ThreadOrigin::Local;
        lines.github_id = None;
        assert!(state.github_offered_on(&lines), "lines alone");
        let mut place = github_thread_with_private_messages();
        place.anchor = None;
        assert!(state.github_offered_on(&place), "a place on GitHub alone");
        let mut neither = lines.clone();
        neither.anchor = None;
        assert!(state.github_offered_on(&neither), "neither, so an issue comment");

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        assert!(!state.github_offered_on(&lines), "no review to take the draft");
        assert!(!state.github_offered_on(&place), "nor a reply for one");
        assert!(state.github_offered_on(&neither), "a closed pull request still takes a comment");

        state.pull_request.set(None);
        assert!(!state.github_offered_on(&neither), "an issue has none of it yet");
    }

    /// A finding's thread as the reviewer leaves it: Perch's own, on the
    /// lines the finding is about, with the finding's text as an agent's
    /// message that no run produced.
    fn finding_thread(text: &str) -> CommentThread {
        use perch_github::{Anchor, Side};
        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        CommentThread {
            id: "finding-1".into(),
            worktree_id: "item-1".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: 7,
                line: 9,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: at,
            messages: vec![CommentMessage {
                id: "f1".into(),
                thread_id: "finding-1".into(),
                origin: ThreadOrigin::Local,
                author: Author::Agent { backend: "claude-code".into(), model: "sonnet".into() },
                body: text.into(),
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

    /// An answer asked at the foot of a pull request sits in a thread of
    /// Perch's own with no lines and no place on GitHub, where a draft
    /// could not go. Post to GitHub is offered there all the same, since
    /// the composer it opens posts an issue comment: the same send the
    /// foot has, which is what the reader would otherwise paste into.
    /// With no pull request to comment on it goes, and Copy stays.
    #[test]
    fn post_to_github_is_offered_on_a_thread_of_perchs_own_and_posts_an_issue_comment() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut ours = github_thread_with_private_messages();
        ours.id = "ours".into();
        ours.origin = ThreadOrigin::Local;
        ours.anchor = None;
        ours.github_id = None;
        for m in &mut ours.messages {
            m.thread_id = "ours".into();
            m.github_id = None;
        }
        state.comment_threads.set(Arc::new(vec![ours.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = message_view(
            app,
            state,
            Rc::new(backend),
            ours.clone(),
            ours.messages[2].clone(),
            ThreadFace::Card,
            Lane::Flush,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let post = find(root_id, "post-to-github").expect("nowhere to reply, so it comments");
        assert!(find(root_id, "copy-message").is_some(), "Copy stays");

        let at = harness.get_layout_rect(root_id).center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        let c = post.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("Post to GitHub opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("ours"));
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.anchor, None);
        assert_eq!(state.composer_draft.get(), "because");
        assert!(state.posts_issue_comment(&composer), "an issue comment, not a draft");

        // An issue has no pull request to comment on.
        state.pull_request.set(None);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "post-to-github").is_none(), "nowhere at all");
    }

    /// Post to GitHub is offered only on an open pull request, and the
    /// pull request lands after the threads, so the action follows it:
    /// absent until it arrives, gone again when it closes.
    #[test]
    fn post_to_github_follows_the_pull_request() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let thread = github_thread_with_private_messages();
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = message_view(
            app,
            state,
            Rc::new(backend),
            thread.clone(),
            thread.messages[2].clone(),
            ThreadFace::Card,
            Lane::Flush,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "post-to-github").is_none(), "no pull request yet");

        state.pull_request.set(Some(Arc::new(pull())));
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "post-to-github").is_some(), "the pull request landed");

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "post-to-github").is_none(), "closed takes no review");
    }

    /// The draft the reader leaves in `thread`, bound for GitHub.
    fn draft_under(thread: &CommentThread) -> CommentMessage {
        let mut draft = thread.messages[0].clone();
        draft.id = "d1".into();
        draft.origin = ThreadOrigin::GitHub;
        draft.author = Author::User;
        draft.github_id = None;
        draft.body = "I will fix it".into();
        draft
    }

    /// Under rohan's comment the question and the answer are Perch only,
    /// the answer continuing the question's run; the draft after them is
    /// flush. A thread that is wholly Perch's marks nothing, and a draft
    /// between two private messages starts a second run.
    #[test]
    fn lanes_follow_runs_of_private_messages_in_a_thread_with_a_github_message() {
        let mut thread = github_thread_with_private_messages();
        thread.messages.push(draft_under(&thread));
        assert_eq!(
            Lane::of(&thread),
            [Lane::Flush, Lane::PrivateStart, Lane::PrivateContinue, Lane::Flush]
        );

        let mut split = thread.clone();
        split.messages.swap(2, 3);
        assert_eq!(
            Lane::of(&split),
            [Lane::Flush, Lane::PrivateStart, Lane::Flush, Lane::PrivateStart]
        );

        let mut private = thread.clone();
        private.messages.remove(3);
        private.messages.remove(0);
        assert!(!private.has_github_message());
        assert_eq!(Lane::of(&private), [Lane::Flush, Lane::Flush]);
    }

    /// Under rohan's comment, the reader's question and the agent's answer
    /// each say Perch only in their bylines; rohan's comment and the
    /// draft say nothing of the kind. Every message is laid out as GitHub
    /// lays out a comment, its text under its byline: on Conversation
    /// each is indented past its avatar, with the line down the avatars
    /// between them, and in the code the first sits flush under its
    /// avatar and the replies are indented under the line.
    #[test]
    fn private_messages_under_a_github_comment_are_labelled_perch_only_in_their_bylines() {
        for face in [ThreadFace::Card, ThreadFace::Inline] {
            let root = TestRoot::new();
            let app = AppState::new();
            let state = state_with_item();
            let mut thread = github_thread_with_private_messages();
            thread.messages.push(draft_under(&thread));
            state.comment_threads.set(Arc::new(vec![thread.clone()]));
            let (backend, _commands) = crate::backend::Backend::recording();
            let view = messages_view(app, state, Rc::new(backend), thread.id.clone(), face)
                .style(|s| s.width(600.0));
            let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
            harness.rebuild();
            harness.paint();
            let root_id = harness.root_id();

            assert_eq!(find_all(root_id, "comment-byline").len(), 4, "{face:?}: a byline per message");
            let stack = find(root_id, "thread-messages").expect("the messages");
            let rows = stack.children();
            assert_eq!(rows.len(), 4, "{face:?}: a row per message");
            let labelled: Vec<bool> = rows.iter().map(|r| find(*r, "perch-only").is_some()).collect();
            assert_eq!(labelled, [false, true, true, false], "{face:?}: the question and the answer");
            for label in find_all(root_id, "perch-only") {
                assert!(harness.get_layout_rect(label).width() > 0.0, "{face:?}: the label is drawn");
            }
            assert!(find(rows[3], "draft-pill").is_some(), "{face:?}: the draft is last");
            assert!(find(rows[0], "Label: \"Ask about this\"").is_some(), "{face:?}: rohan's comment");

            // Every byline starts at one place, past the avatar; the text
            // under it starts there too, but for the first message in the
            // code, which is flush with its avatar.
            let left = stack.get_visual_rect_no_clip().x0;
            let inset = match face {
                ThreadFace::Card => theme::THREAD_INSET,
                ThreadFace::Inline => theme::INLINE_THREAD_INSET,
            };
            let avatar = match face {
                ThreadFace::Card => theme::THREAD_AVATAR,
                ThreadFace::Inline => theme::INLINE_THREAD_AVATAR,
            };
            let past_avatar = inset + avatar + theme::THREAD_AVATAR_GAP;
            for (i, row) in rows.iter().enumerate() {
                let byline = find(*row, "comment-byline").unwrap().get_visual_rect_no_clip();
                assert_eq!(byline.x0 - left, past_avatar, "{face:?}: row {i}'s byline");
                let text = find(*row, "LinkedText")
                    .or_else(|| find(*row, "RichText"))
                    .expect("the message's text")
                    .get_visual_rect_no_clip();
                let flush = face == ThreadFace::Inline && i == 0;
                let expected = if flush { inset } else { past_avatar };
                assert_eq!(text.x0 - left, expected, "{face:?}: row {i}'s text");
                // The line down the avatars runs beside every message
                // indented under it.
                assert_eq!(find(*row, "comment-rail").is_some(), !flush, "{face:?}: row {i}'s rail");
            }
        }
    }

    /// A thread that is wholly Perch's, as every thread was before drafts,
    /// labels nothing Perch only: the label would say nothing there.
    #[test]
    fn a_wholly_private_thread_labels_nothing_perch_only() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let mut thread = github_thread_with_private_messages();
        thread.messages.remove(0);
        thread.origin = ThreadOrigin::Local;
        thread.github_id = None;
        thread.id = "t1".into();
        for m in &mut thread.messages {
            m.thread_id = "t1".into();
        }
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = messages_view(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "perch-only").is_none());
        assert_eq!(find_all(root_id, "comment-byline").len(), 2, "the question and the answer have bylines");
        assert!(find(root_id, "agent-avatar").is_some(), "the answer is under the agent's mark");

        // A draft added under it labels what was there.
        let mut with_draft = thread.clone();
        let mut draft = draft_under(&with_draft);
        draft.thread_id = "t1".into();
        with_draft.messages.push(draft);
        state.comment_threads.set(Arc::new(vec![with_draft]));
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert_eq!(find_all(root_id, "perch-only").len(), 2, "the question and the answer are labelled");
        assert!(find(root_id, "draft-pill").is_some());
    }

    fn pull() -> perch_github::PullRequest {
        perch_github::PullRequest {
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

    /// The item as a pull request, whose conversation takes a comment.
    fn state_with_pull_request() -> ReviewState {
        let state = state_with_item();
        state.pull_request.set(Some(Arc::new(pull())));
        state
    }

    fn escape() -> floem::event::Event {
        key(Key::Named(NamedKey::Escape), Code::Escape, Modifiers::empty())
    }

    /// Whether a view is laid out at all: a hidden one has no size.
    fn shown(harness: &HeadlessHarness, text: &str) -> bool {
        find(harness.root_id(), text).is_some_and(|id| harness.get_layout_rect(id).height() > 0.0)
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let id = find(harness.root_id(), name).unwrap_or_else(|| panic!("no view {name}"));
        let center = id.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        harness.rebuild();
        harness.paint();
    }

    /// rohan's thread alone, `resolved` or not, which GitHub lets the
    /// reader resolve.
    fn resolvable_thread(resolved: bool) -> CommentThread {
        let mut thread = github_thread_with_private_messages();
        thread.messages.truncate(1);
        thread.resolved = resolved;
        thread.node_id = Some("PRRT_31".into());
        thread.can_resolve = true;
        thread
    }

    /// Resolve at the foot of the card in the code asks GitHub and shows
    /// the thread resolved before it answers: folded, as a thread
    /// resolved on GitHub arrives.
    #[test]
    fn resolve_at_the_foot_of_a_github_thread_asks_github_and_folds_the_card() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = resolvable_thread(false);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = thread_card(app, state, Rc::new(backend), thread).style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-resolve"));
        assert!(find(harness.root_id(), "thread-unresolve").is_none());

        click(&mut harness, "thread-resolve");
        match commands.try_recv() {
            Ok(Command::ResolveThread { item_id, thread_id, resolve }) => {
                assert_eq!((item_id.as_str(), thread_id.as_str(), resolve), ("item-1", "gh-31", true));
            }
            other => panic!("Resolve asked nothing: {other:?}"),
        }
        assert!(state.is_resolved("gh-31"));
        assert!(state.is_collapsed(&CardId::Thread("gh-31".into())));
        assert!(!shown(&harness, "thread-resolve"), "folded away with the card");
    }

    /// A resolved thread offers Unresolve. The thread shows the click at
    /// once, and a second click waits for GitHub; a refusal puts the
    /// thread back as the threads have it.
    #[test]
    fn a_resolved_thread_offers_unresolve_and_takes_one_click_until_github_answers() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = resolvable_thread(true);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = follow_up(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-unresolve"));

        click(&mut harness, "thread-unresolve");
        assert!(
            matches!(commands.try_recv(), Ok(Command::ResolveThread { resolve: false, .. })),
            "Unresolve asks to unresolve"
        );
        assert!(shown(&harness, "thread-resolve"), "shown unresolved before GitHub answers");
        assert!(!shown(&harness, "thread-unresolve"));
        click(&mut harness, "thread-resolve");
        assert!(commands.try_recv().is_err(), "the first ask is unanswered");

        state.settle_resolution("gh-31");
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-unresolve"), "refused, it is as GitHub has it");
        assert!(!shown(&harness, "thread-resolve"));
    }

    /// Only where GitHub lets the reader, and not while the composer has
    /// the foot. On Conversation, Resolve conversation is in the foot
    /// under the grey reply bar, at the thread's left, where github.com
    /// puts it.
    #[test]
    fn resolve_is_absent_where_github_does_not_let_the_reader_and_while_composing() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut thread = resolvable_thread(false);
        thread.can_resolve = false;
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = follow_up(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "thread-resolve"));
        assert!(shown(&harness, "Follow up"), "the line is there all the same");

        thread.can_resolve = true;
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-resolve"));
        assert!(find(harness.root_id(), "Label: \"Resolve conversation\"").is_some(), "GitHub's words");
        let line = find(harness.root_id(), "Follow up").unwrap().get_visual_rect_no_clip();
        let button = find(harness.root_id(), "thread-resolve").unwrap().get_visual_rect_no_clip();
        assert!(button.y0 > line.y1, "under the Follow up box: {line:?} {button:?}");
        assert_eq!(button.x0, theme::THREAD_INSET, "at the thread's left, in its inset: {button:?}");
        // The box sits past the reader's face, in the bar's inset.
        let bar = find(harness.root_id(), "reply-bar").unwrap().get_visual_rect_no_clip();
        assert_eq!(bar.width(), 600.0, "the bar is the thread's width");
        assert_eq!(
            line.x0,
            theme::THREAD_INSET + theme::THREAD_AVATAR + theme::THREAD_AVATAR_GAP,
            "the box is past the face"
        );
        assert_eq!(line.x1, 600.0 - theme::THREAD_INSET, "and reaches the bar's inset");

        state.open_composer(Some(thread.id.clone()), thread.anchor.clone(), String::new(), sonnet_medium());
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "thread-resolve"));
        let composer = find(harness.root_id(), "composer-box").expect("the composer is under the thread");
        assert_eq!(
            harness.get_layout_rect(composer).width(),
            600.0 - 2.0 * theme::THREAD_INSET,
            "and has the bar to itself, the face gone"
        );
        state.close_composer();
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-resolve"));
    }

    /// A thread of Perch's own, the reader's question and an agent's
    /// answer, resolves too, in Perch. A finding's has Dismiss and Reopen
    /// instead, and no Resolve.
    #[test]
    fn a_thread_of_perchs_own_resolves_in_perch_and_a_findings_does_not() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = finding_thread("because");
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = follow_up(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-resolve"), "the reader's own, whatever GitHub allows");

        click(&mut harness, "thread-resolve");
        assert!(
            matches!(
                commands.try_recv(),
                Ok(Command::ResolveThread { thread_id, resolve: true, .. }) if thread_id == "finding-1"
            ),
            "Resolve asks the backend, which keeps it in Perch"
        );
        assert!(state.is_resolved("finding-1"));
        assert!(shown(&harness, "thread-unresolve"));
        state.settle_resolution("finding-1");

        // Once the thread is a finding's, the finding's controls say what
        // becomes of it.
        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        app.update_review_finding(
            "item-1".into(),
            crate::review::model::ReviewFindingSummary {
                finding: perch_github::Finding {
                    id: "f-1".into(),
                    item_id: "item-1".into(),
                    stable_key: "lock".into(),
                    thread_id: "finding-1".into(),
                    created_group_id: None,
                    disposition: perch_github::FindingDisposition::Open,
                    verification: perch_github::FindingVerification::Unverified,
                    dismissal_reason: None,
                    possible_duplicate_of: None,
                    duplicate_of: None,
                    last_verified_snapshot_id: None,
                    last_verified_head_sha: None,
                    last_checked_snapshot_id: None,
                    last_checked_head_sha: None,
                    created_at: at,
                    updated_at: at,
                },
                severity: perch_github::FindingSeverity::High,
                attributions: Vec::new(),
                seen_in_reviews: Vec::new(),
                backend: None,
                model: None,
                original_review_number: None,
            },
        );
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "thread-resolve"));
        assert!(!shown(&harness, "thread-unresolve"));
    }

    /// The words of every label under `id`, in tree order.
    fn texts(id: floem::ViewId) -> Vec<String> {
        let mut out = Vec::new();
        let name = id.debug_name();
        if let Some(start) = name.find("Label: \"") {
            let rest = &name[start + "Label: \"".len()..];
            if let Some(end) = rest.rfind('"') {
                out.push(rest[..end].to_string());
            }
        }
        for c in id.children() {
            out.extend(texts(c));
        }
        out
    }

    /// On Conversation a thread folds as github.com's does. Its chevron
    /// folds it to its header and opens it again; Resolve conversation
    /// folds it at once, before GitHub answers, with Show resolved at the
    /// header's end; Show resolved opens it, with Hide resolved there and
    /// Unresolve conversation in its foot; and none of it opens or
    /// collapses the thread's card in the code.
    #[test]
    fn resolve_folds_a_conversation_thread_to_its_header_and_show_resolved_opens_it() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = resolvable_thread(false);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = cards::thread_card(app, state, Rc::new(backend), Rc::new(|_| {}), thread.clone())
            .style(|s| s.width(700.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 800.0);
        harness.rebuild();
        harness.paint();
        let comment = "name label - Label: \"rohan\"";
        assert!(shown(&harness, comment), "open, the comment shows");
        assert!(shown(&harness, "thread-path"), "the header names the file");
        assert!(!shown(&harness, "resolved-toggle"), "nothing to show or hide on a thread not resolved");

        // The chevron folds it to its header, and opens it again.
        click(&mut harness, "thread-chevron");
        assert!(state.is_folded("gh-31"));
        assert!(find(harness.root_id(), comment).is_none(), "folded to its header");
        assert!(shown(&harness, "thread-path"));
        click(&mut harness, "thread-chevron");
        assert!(shown(&harness, comment));

        // Resolve conversation folds it before GitHub answers.
        assert!(shown(&harness, "Label: \"Resolve conversation\""));
        click(&mut harness, "thread-resolve");
        // The card asked for its file's lines as it was built; past that,
        // Resolve asked GitHub.
        let resolve = std::iter::from_fn(|| commands.try_recv().ok())
            .find(|c| matches!(c, Command::ResolveThread { .. }));
        match resolve {
            Some(Command::ResolveThread { thread_id, resolve: true, .. }) => assert_eq!(thread_id, "gh-31"),
            other => panic!("Resolve asked nothing: {other:?}"),
        }
        assert!(find(harness.root_id(), comment).is_none(), "resolved, folded to its header");
        assert!(shown(&harness, "Label: \"Show resolved\""));

        // GitHub answers, and the thread stays folded.
        let mut resolved = thread.clone();
        resolved.resolved = true;
        state.comment_threads.set(Arc::new(vec![resolved]));
        state.settle_resolution("gh-31");
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), comment).is_none());

        // Show resolved opens it: Hide resolved in the header, Unresolve
        // conversation in the foot. The card in the code stays collapsed.
        click(&mut harness, "resolved-toggle");
        assert!(shown(&harness, comment));
        assert!(shown(&harness, "Label: \"Hide resolved\""));
        assert!(shown(&harness, "Label: \"Unresolve conversation\""));
        assert!(state.is_collapsed(&CardId::Thread("gh-31".into())));

        // Hide resolved folds it again.
        click(&mut harness, "resolved-toggle");
        assert!(find(harness.root_id(), comment).is_none());
        assert!(shown(&harness, "Label: \"Show resolved\""));
    }

    /// In the code a thread is headed as Files changed heads one: GitHub's
    /// "Comment on line R7", and one of the reader's own a question on its
    /// lines, marked Local. Resolved, a thread is its one row with Show
    /// resolved, which opens it with Hide resolved in its header and
    /// Unresolve comment in its foot, and leaves Conversation folded.
    #[test]
    fn a_card_in_the_code_is_headed_by_its_lines_and_a_resolved_one_offers_show_resolved() {
        use perch_github::{Anchor, Side};
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let theirs = resolvable_thread(true);
        let mut ours = theirs.clone();
        ours.id = "t1".into();
        ours.origin = ThreadOrigin::Local;
        ours.github_id = None;
        ours.resolved = false;
        ours.anchor = Some(Anchor {
            path: "src/x.rs".into(),
            start_line: 5,
            line: 7,
            side: Side::Right,
            commit_sha: "h".into(),
        });
        ours.messages = vec![CommentMessage {
            id: "q1".into(),
            thread_id: "t1".into(),
            origin: ThreadOrigin::Local,
            author: Author::User,
            github_id: None,
            body: "why?".into(),
            ..theirs.messages[0].clone()
        }];
        state.comment_threads.set(Arc::new(vec![theirs.clone(), ours.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let backend = Rc::new(backend);
        let view = floem::views::Stack::vertical((
            thread_card(app, state, backend.clone(), theirs).debug_name("theirs"),
            thread_card(app, state, backend, ours).debug_name("ours"),
        ))
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 900.0);
        harness.rebuild();
        harness.paint();

        let ours = find(harness.root_id(), "ours").unwrap();
        let heading = find(ours, "lines-heading").expect("the reader's thread is open");
        assert_eq!(texts(heading).join(" "), "Question on lines R5 to R7");
        assert!(find(ours, "Label: \"Local\"").is_some(), "marked as the reader's own");

        let theirs = find(harness.root_id(), "theirs").unwrap();
        assert!(find(theirs, "lines-heading").is_none(), "resolved, so collapsed to its row");
        let toggle = find(theirs, "resolved-toggle").expect("Show resolved on the row");
        assert!(harness.get_layout_rect(toggle).width() > 0.0);
        assert_eq!(texts(toggle), ["Show resolved"]);
        let c = toggle.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        harness.rebuild();
        harness.paint();
        assert!(!state.is_collapsed(&CardId::Thread("gh-31".into())));
        let theirs = find(harness.root_id(), "theirs").unwrap();
        let heading = find(theirs, "lines-heading").expect("open, with its header");
        assert_eq!(texts(heading).join(" "), "Comment on line R7");
        assert_eq!(texts(find(theirs, "resolved-toggle").unwrap()), ["Hide resolved"]);
        assert!(find(theirs, "Label: \"Unresolve comment\"").is_some());
        assert!(state.is_folded("gh-31"), "opening it in the code leaves Conversation folded");
    }

    /// A comment by whoever opened the pull request says Author in its
    /// byline, as GitHub marks them; anyone else's does not.
    #[test]
    fn a_comment_by_the_pull_requests_author_is_marked_author() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut thread = resolvable_thread(false);
        let mut reply = thread.messages[0].clone();
        reply.id = "gh-32".into();
        reply.author = Author::GitHubUser("octocat".into());
        reply.github_id = Some(32);
        reply.body = "fixed".into();
        thread.messages.push(reply);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = messages_view(app, state, Rc::new(backend), thread.id.clone(), ThreadFace::Card)
            .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let rows = find(harness.root_id(), "thread-messages").unwrap().children();
        let marked: Vec<bool> = rows
            .iter()
            .map(|row| {
                find(*row, "author-label").is_some_and(|l| harness.get_layout_rect(l).width() > 0.0)
            })
            .collect();
        assert_eq!(marked, [false, true], "octocat opened the pull request; rohan did not");
    }

    /// At the foot of a pull request's conversation GitHub is offered
    /// and posts an issue comment: under a thread that is on GitHub it is
    /// a reply for the review, under one of Perch's own with no lines it
    /// is an issue comment too, and on an issue it is not offered.
    #[test]
    fn github_is_offered_at_the_foot_of_a_pull_request_and_posts_at_once() {
        let state = state_with_pull_request();
        let mut theirs = github_thread_with_private_messages();
        theirs.id = "theirs".into();
        let mut ours = github_thread_with_private_messages();
        ours.id = "ours".into();
        ours.anchor = None;
        ours.github_id = None;
        ours.origin = ThreadOrigin::Local;
        state.comment_threads.set(Arc::new(vec![theirs, ours]));

        state.open_composer(None, None, String::new(), sonnet_medium());
        let foot = state.composer.get().unwrap();
        assert_eq!(foot.destination, Destination::agent(sonnet_medium()), "an agent first");
        assert!(state.github_offered(&foot));
        assert!(state.posts_issue_comment(&foot));

        state.open_composer(Some("theirs".into()), None, String::new(), sonnet_medium());
        let reply = state.composer.get().unwrap();
        assert!(state.github_offered(&reply));
        assert!(!state.posts_issue_comment(&reply), "a reply on GitHub's thread is a draft");

        state.open_composer(Some("ours".into()), None, String::new(), sonnet_medium());
        let follow_up = state.composer.get().unwrap();
        assert!(state.github_offered(&follow_up));
        assert!(state.posts_issue_comment(&follow_up), "nowhere on GitHub to reply");

        state.pull_request.set(None);
        state.open_composer(None, None, String::new(), sonnet_medium());
        let issue = state.composer.get().unwrap();
        assert!(!state.github_offered(&issue), "an issue takes no comment yet");
    }

    /// With GitHub picked at the foot the button reads Comment. The first
    /// send asks, naming the pull request, and Escape takes the question
    /// back with the text intact; the next send, Enter alone, posts. The
    /// box is still while the comment goes, a refusal gives the text back
    /// with the reason under it, and a comment that posted closes the
    /// composer. The next composer at the foot is an agent's again.
    #[test]
    fn an_issue_comment_asks_once_posts_on_enter_and_keeps_the_text_when_refused() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        state.open_composer(None, None, "Looks good".into(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Label: \"Comment\"").is_some());
        assert!(find(root_id, "Label: \"Medium\"").is_none(), "no effort for GitHub");
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));

        // The first send asks.
        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        harness.paint();
        assert!(commands.try_recv().is_err(), "nothing posted before the yes");
        assert!(state.composer_confirming.get());
        let root_id = harness.root_id();
        assert!(find(root_id, "Post to github.com/acme/repo/pull/42?").is_some());
        assert!(find(root_id, "composer-post").is_some());
        assert!(find(root_id, "Label: \"Comment\"").is_none(), "the send gave way to the ask");

        // Escape takes it back, text and all.
        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(!state.composer_confirming.get());
        assert!(state.composer.get().is_some(), "the composer stays");
        assert_eq!(state.composer_draft.get(), "Looks good");
        assert!(find(harness.root_id(), "Label: \"Comment\"").is_some());

        // Enter alone is the yes.
        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        assert!(state.composer_confirming.get());
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        harness.paint();
        let token = match commands.try_recv() {
            Ok(Command::PostIssueComment {
                item_id,
                token,
                body,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(body, "Looks good");
                token
            }
            other => panic!("Enter did not post: {other:?}"),
        };
        assert_eq!(state.composer_draft.get(), "Looks good", "Enter did not break the line");
        assert!(!state.composer_confirming.get());
        let posting = state.posting.get().expect("the comment is in flight");
        assert_eq!(posting.token, token);
        assert_eq!(posting.body, "Looks good");
        assert!(state.composer.get().is_some(), "the composer stays open while posting");
        let root_id = harness.root_id();
        assert!(find(root_id, "composer-posting").is_some());
        assert!(find(root_id, "Label: \"Comment\"").is_none(), "nothing to click while posting");
        assert_eq!(
            state.last_destination[ThreadFace::Card.index()].get(),
            Some(Destination::GitHub)
        );

        // Nothing sends twice, and the keys do nothing to the text.
        harness.dispatch_event(enter(Modifiers::META));
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert!(commands.try_recv().is_err());
        assert_eq!(state.composer_draft.get(), "Looks good");
        // Nor can the box be put away from under the comment.
        assert!(!shown(&harness, "composer-close"), "no closing while posting");

        // An answer to some other send is not this one's.
        state.settle_issue_comment(token + 100, Err("stale".into()));
        assert!(state.posting.get().is_some());
        assert!(state.composer_error.get().is_none());

        // Refused: the text is back with the reason under it.
        state.settle_issue_comment(token, Err("GitHub could not post the comment: 403".into()));
        harness.rebuild();
        harness.paint();
        assert!(state.posting.get().is_none());
        assert!(state.composer.get().is_some(), "the composer stays with its text");
        assert_eq!(state.composer_draft.get(), "Looks good");
        let root_id = harness.root_id();
        assert!(find(root_id, "composer-error").is_some());
        assert!(find(root_id, "GitHub could not post the comment: 403").is_some());
        assert!(find(root_id, "Label: \"Comment\"").is_some(), "the send is back");
        assert!(shown(&harness, "composer-close"), "and so is the close");

        // Clicking Comment asks, clicking Post posts, and this time it lands.
        click(&mut harness, "composer-send");
        assert!(state.composer_confirming.get());
        assert!(commands.try_recv().is_err());
        click(&mut harness, "composer-post");
        let token = match commands.try_recv() {
            Ok(Command::PostIssueComment { token, body, .. }) => {
                assert_eq!(body, "Looks good");
                token
            }
            other => panic!("Post did not post: {other:?}"),
        };
        assert!(state.composer_error.get().is_none(), "a new try clears the old reason");
        state.settle_issue_comment(token, Ok(()));
        assert!(state.posting.get().is_none());
        assert!(state.composer.get().is_none(), "posted, the composer closes");
        assert_eq!(state.composer_draft.get(), "");

        // The foot still opens on an agent; GitHub is a click away.
        state.open_composer(None, None, String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::agent(sonnet_medium())
        );
    }

    /// Cancel under the ask gives the footer back, and picking a
    /// destination while asking drops the ask, since it was about the
    /// old one.
    #[test]
    fn cancel_and_a_new_destination_take_the_ask_back() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        state.open_composer(None, None, "Looks good".into(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();

        click(&mut harness, "composer-send");
        assert!(state.composer_confirming.get());
        click(&mut harness, "composer-cancel-post");
        assert!(!state.composer_confirming.get());
        assert_eq!(state.composer_draft.get(), "Looks good");
        assert!(find(harness.root_id(), "Label: \"Comment\"").is_some());

        click(&mut harness, "composer-send");
        assert!(state.composer_confirming.get());
        state.set_destination(Destination::agent(sonnet_medium()));
        harness.rebuild();
        harness.paint();
        assert!(!state.composer_confirming.get());
        assert!(find(harness.root_id(), "Label: \"Send\"").is_some());
        assert!(commands.try_recv().is_err(), "nothing was posted");
    }

    /// A composer opened over another, as Ask about this and Reply do,
    /// starts without the ask or the refusal that were about the old
    /// one: an Enter in the new box is a line, not a yes.
    #[test]
    fn a_composer_opened_over_another_starts_without_its_ask_or_refusal() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        state.open_composer(None, None, "Looks good".into(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        state.composer_confirming.set(true);
        state.composer_error.set(Some("GitHub could not post the comment: 403".into()));

        state.open_composer_with(
            None,
            None,
            "> quoted".into(),
            Destination::agent(sonnet_medium()),
            None,
        );
        assert!(!state.composer_confirming.get());
        assert!(state.composer_error.get().is_none());

        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Label: \"Send\"").is_some());
        assert!(find(root_id, "composer-post").is_none());
        assert!(find(root_id, "composer-error").is_none());
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert!(commands.try_recv().is_err(), "Enter was a line");
        assert_eq!(state.composer_draft.get(), "> quoted\n");
    }

    /// An issue comment in flight belongs to the composer that sent it.
    /// One opened since is not still, and the answer, come late, does not
    /// close it or put a refusal under it; the flight is over either way.
    #[test]
    fn an_answer_to_a_composer_put_away_settles_nothing_but_the_flight() {
        let state = state_with_pull_request();
        state.open_composer(None, None, "Looks good".into(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let sent = state.composer.get().unwrap().opening;
        state.start_posting(3, "Looks good".into());
        assert!(state.posting_from(sent));

        state.open_composer(None, None, "Another".into(), sonnet_medium());
        let next = state.composer.get().unwrap().opening;
        assert!(!state.posting_from(next), "nothing of this composer's is in flight");
        assert!(state.posting.get().is_some());

        state.settle_issue_comment(3, Err("GitHub could not post the comment: 403".into()));
        assert!(state.posting.get().is_none());
        assert!(state.composer_error.get().is_none(), "the refusal was not this composer's");
        assert_eq!(state.composer_draft.get(), "Another");

        state.start_posting(4, "Another".into());
        state.open_composer(None, None, "A third".into(), sonnet_medium());
        state.settle_issue_comment(4, Ok(()));
        assert!(state.posting.get().is_none());
        assert!(state.composer.get().is_some(), "the third composer is not the one that posted");
        assert_eq!(state.composer_draft.get(), "A third");
    }

    /// With the ask up, emptying the box takes the ask down: there is no
    /// comment left to post.
    #[test]
    fn emptying_the_box_takes_the_ask_back() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        state.open_composer(None, None, "x".into(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let composer = state.composer.get().unwrap();
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(enter(Modifiers::META));
        harness.rebuild();
        assert!(state.composer_confirming.get());
        harness.dispatch_event(key(
            Key::Named(NamedKey::Backspace),
            Code::Backspace,
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.composer_draft.get(), "");
        assert!(!state.composer_confirming.get());
        assert!(find(harness.root_id(), "Label: \"Comment\"").is_some());
    }

    /// The lines wrap inside the box's padding, and the box's border takes
    /// the accent while the lines have the keyboard. floem lays the lines
    /// out in a layer the size of the editor's padding box, so padding on
    /// the editor itself, as the box once had, left the layer 10px into
    /// the padding on each side and the last word of a line over the
    /// border; and the keyboard is the inner editor view's, so a `focus`
    /// style on anything around it never applied.
    #[test]
    fn the_lines_wrap_inside_the_box_and_the_border_follows_the_keyboard() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item =
            perch_github::Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-1".into();
        state.item.set(Some(Arc::new(item)));
        state.open_composer(
            None,
            None,
            "why?".into(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        let composer = state.composer.get().unwrap();
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Inline);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();

        fn find(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
            if id.debug_name().starts_with(name) {
                return Some(id);
            }
            id.children().into_iter().find_map(|c| find(c, name))
        }
        let text_box = find(harness.root_id(), "editor-box").expect("the box");
        let lines = find(text_box, "Editor View").expect("the lines in it");
        let room = text_box.get_content_rect_local();
        let outer = text_box.get_visual_rect();
        assert!(room.x0 > 0.0, "the box has padding");
        assert_eq!(
            lines.get_visual_rect(),
            floem::kurbo::Rect::new(
                outer.x0 + room.x0,
                outer.y0 + room.y0,
                outer.x0 + room.x1,
                outer.y0 + room.y1,
            ),
            "the lines fill the room inside the padding, and no more"
        );

        assert!(harness.is_focused(lines), "the lines took the keyboard");
        harness.recompute_styles();
        assert_eq!(
            harness
                .get_computed_style(text_box)
                .get(floem::style::BorderLeftColor),
            Some(theme::ACCENT.into()),
            "the border follows the keyboard into the box"
        );
    }

    /// The box opens five lines tall, and the grip along the inside of
    /// its bottom edge drags it taller and shorter, between the two
    /// limits. What the drag sets stays set.
    #[test]
    fn the_grip_drags_the_box_taller_and_shorter() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        state.open_composer(None, None, String::new(), sonnet_medium());
        let composer = state.composer.get().unwrap();
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Inline);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 900.0);
        harness.rebuild();
        harness.paint();

        let text_box = find(harness.root_id(), "editor-box").expect("the box");
        let height = move || text_box.get_size().expect("the box is laid out").height;
        assert_eq!(height(), theme::COMPOSER_HEIGHT);
        assert_eq!(state.composer_height.get(), None, "undragged");

        // Grab the grip and take it down sixty pixels: the box follows,
        // and keeps the height when the pointer lets go.
        let grip = find(harness.root_id(), "composer resize handle").expect("the grip");
        let (x, y) = {
            let rect = grip.get_visual_rect_no_clip();
            (rect.center().x, rect.center().y)
        };
        harness.pointer_down(x, y);
        harness.pointer_move(x, y + 60.0);
        assert!(state.composer_resizing.get());
        assert_eq!(state.composer_height.get(), Some(theme::COMPOSER_HEIGHT + 60.0));
        harness.rebuild();
        harness.paint();
        assert_eq!(height(), theme::COMPOSER_HEIGHT + 60.0);

        // The height is the pointer's distance from the grab, not the
        // last move's, so a drag back up shortens it and cannot drift.
        harness.pointer_move(x, y - 20.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(height(), theme::COMPOSER_HEIGHT - 20.0);

        // Neither limit is passed.
        harness.pointer_move(x, y - 400.0);
        assert_eq!(state.composer_height.get(), Some(theme::COMPOSER_MIN_HEIGHT));
        harness.pointer_move(x, y + 900.0);
        assert_eq!(state.composer_height.get(), Some(theme::COMPOSER_MAX_HEIGHT));

        harness.pointer_move(x, y + 100.0);
        harness.pointer_up(x, y + 100.0);
        assert!(!state.composer_resizing.get(), "the drag is over");
        assert_eq!(
            state.composer_height.get(),
            Some(theme::COMPOSER_HEIGHT + 100.0),
            "the box stays where it was left"
        );
    }

    /// How far a box's lines are scrolled, and how far they can be: the
    /// lines run past the box, and the part of them that shows is the box.
    fn scrolled(lines: floem::ViewId) -> (f64, f64) {
        let shown = lines.get_visual_rect();
        let whole = lines.get_visual_rect_no_clip();
        (shown.y0 - whole.y0, whole.height() - shown.height())
    }

    /// Types `text` into the focused box a key at a time, and hands back
    /// how far the box's lines were scrolled after each key.
    fn type_into(harness: &mut HeadlessHarness, lines: floem::ViewId, text: &str) -> Vec<f64> {
        text.chars()
            .map(|ch| {
                let code = if ch == ' ' { Code::Space } else { Code::KeyX };
                harness.dispatch_event(key(
                    Key::Character(ch.to_string()),
                    code,
                    Modifiers::empty(),
                ));
                harness.paint();
                scrolled(lines).0
            })
            .collect()
    }

    /// A question longer than its box opens scrolled to the caret at its
    /// end, and stays there as it is typed into, following the caret onto
    /// a new row. floem forgot the rows of the wrapped line with every key
    /// and put the caret on the line's first one, so the box jumped to the
    /// top and back on alternate keys.
    #[test]
    fn a_long_question_stays_scrolled_to_the_caret_as_it_is_typed() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let question = (1..=80)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        state.open_composer(None, None, question, sonnet_medium());
        let composer = state.composer.get().unwrap();
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Inline);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 900.0);
        harness.rebuild();
        harness.paint();

        let text_box = find(harness.root_id(), "editor-box").expect("the box");
        let lines = find(text_box, "Editor View").expect("the lines in it");
        let (at, end) = scrolled(lines);
        assert!(end > 0.0, "the question is longer than its box");
        assert_eq!(at, end, "the box opens on the caret");

        let offsets = type_into(&mut harness, lines, " and then some more");
        let (_, end_now) = scrolled(lines);
        assert!(end_now > end, "the words went onto a new row");
        assert!(
            offsets.iter().all(|&at| at >= end),
            "the box stayed down with the caret as it was typed into: {offsets:?}"
        );
        assert_eq!(
            scrolled(lines).0,
            end_now,
            "and followed it onto the new row"
        );
    }

    /// A question of several paragraphs opens scrolled to its end, and a
    /// word typed into its middle leaves the box where it is. The rows
    /// of the paragraphs above the view counted as one each until they
    /// were laid out again, so the box opened short of the caret, and
    /// every key moved it.
    #[test]
    fn a_question_of_paragraphs_opens_on_its_end_and_holds_still_mid_way() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let question = (1..=6)
            .map(|p| {
                (1..=25)
                    .map(|i| format!("w{p}x{i}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n");
        state.open_composer(None, None, question, sonnet_medium());
        let composer = state.composer.get().unwrap();
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Inline);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 900.0);
        harness.rebuild();
        harness.paint();

        let text_box = find(harness.root_id(), "editor-box").expect("the box");
        let lines = find(text_box, "Editor View").expect("the lines in it");
        let (at, end) = scrolled(lines);
        assert_eq!(at, end, "the box opens on the caret at the end");

        // Up into the middle of the question, well above where the box
        // opened.
        for _ in 0..14 {
            harness.dispatch_event(key(
                Key::Named(NamedKey::ArrowUp),
                Code::ArrowUp,
                Modifiers::empty(),
            ));
        }
        harness.paint();
        let (mid_way, _) = scrolled(lines);
        assert!(mid_way < end - 100.0, "the caret went up the question");

        let offsets = type_into(&mut harness, lines, "abc");
        assert_eq!(offsets, vec![mid_way; 3], "the box held still");
    }
}
