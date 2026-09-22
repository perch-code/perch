//! Thread views: questions anchored to lines, and the agent's answers.
//!
//! A thread is a comment thread, not a chat: short questions, short
//! answers, tied to lines. Anchored threads are cards in the code, and the
//! complete item history is on Conversation. A composer sits in the code
//! under selected lines or on Conversation for an item-level question; a
//! follow-up's composer sits under its thread.
//!
//! The message, the run's steps and footer, the follow-up line and the
//! composer are one set of views here, taking a [`ThreadFace`] for the
//! sizes that differ, so Conversation and inline cards cannot drift apart.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{
    dyn_container, dyn_stack, text_editor, Container, Decorators, Empty, Label, ScrollExt, Stack,
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
    clickable, editor_box, icon_button, name_label, sidebar_glyph, text_button, Name,
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

    /// Room between a thread's messages.
    fn message_gap(self) -> f64 {
        match self {
            ThreadFace::Inline => theme::THREAD_GAP,
            ThreadFace::Card => 20.0,
        }
    }
}

/// Where a message sits across the thread's width. In a thread that has
/// something on GitHub, a message that does not, the reader's question to
/// an agent and the agent's answer, is drawn in the private lane: inset,
/// with a hairline down its left, so the reader never wonders what a
/// colleague can see. The first of each run of them carries the lane's
/// label. Everything else, and every message of a wholly private thread,
/// sits flush: the lane is a difference, not a decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    Flush,
    PrivateStart,
    PrivateContinue,
}

impl Lane {
    /// The lane of each message of `thread`, in order.
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
    let placeholder = if composer.thread_id.is_some() {
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
    let doc_editor = editor.editor().clone();
    let read_text = Rc::new(move || doc_editor.doc().text().to_string());
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
            state.composer_focused.set(true)
        })
        .on_event_cont(el::FocusLost, move |_, _| state.composer_focused.set(false))
        // Escape closes an empty composer, or one editing a draft, whose
        // words are still in the cache and come back as the message row.
        // In the capture phase, because the editor stops every key once
        // it has focus and this has to run first.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
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
    // One editing a draft inside an inline card sits among the messages,
    // in the card's own scroll, so it scrolls that to itself too.
    let inner = editor.editor().editor_view_id;
    let editing = composer.editing.is_some();
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
            } else if editing {
                id.scroll_to(None);
            }
            if state.composer_autofocus.get_untracked() {
                id.request_focus();
            }
        })
        .on_cleanup(move || {
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
        editor_box(editor, 72.0, face == ThreadFace::Inline),
        composer_error_line(state),
        composer_footer(app, state, send, posting_here),
    ))
    .debug_name("composer-box")
    .style(move |s| {
        let base = s.flex_col().padding(12.0).border(1.0).border_radius(14.0);
        match face {
            ThreadFace::Card => base
                .width_full()
                .background(theme::CANVAS_SUBTLE)
                .border_color(theme::BORDER_MUTED),
            // A card in the code is placed by its edges, and its width
            // follows from them.
            ThreadFace::Inline => base
                .background(theme::CANVAS)
                .border_color(theme::ACCENT_RING),
        }
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
fn model_groups(agent: &perch_github::AgentConfig) -> Vec<Vec<(perch_agent::Backend, String)>> {
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

/// A thread inline in the code, led by a chevron that collapses it to one
/// row, with no anchor label since it is already under those lines. It
/// holds the messages, the run under an agent's answer with its steps on
/// one line, and the follow-up line. A gutter marker click flashes its
/// border. One of the reader's own is grey inside a dashed border; one of
/// GitHub's is white inside a solid one, so neither can be taken for the
/// other.
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
        // Collapsed, it is its one row exactly. The layer sets its width
        // from the edges it is placed against.
        let base = s.apply_if(collapsed.get(), |s| s.height(theme::CODE_LINE_HEIGHT as f64));
        let base = match origin {
            ThreadOrigin::Local => base
                .background(theme::CANVAS_SUBTLE)
                .border(floem::kurbo::Stroke::new(1.0).with_dashes(0.0, theme::THREAD_DASH))
                .border_radius(theme::RADIUS_LG)
                .border_color(theme::BORDER),
            ThreadOrigin::GitHub => base
                .background(theme::CANVAS)
                .border(1.0)
                .border_radius(theme::RADIUS)
                .border_color(theme::BORDER_MUTED),
        };
        if flashing.get() {
            base.border_color(theme::ACCENT)
        } else {
            base
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

/// A thread's card open: its first row, the delete confirmation when one
/// is asked for, the messages and the follow-up. In the code the messages
/// scroll inside the card once it is as tall as a card may be, so a long
/// thread cannot push the code off the screen.
fn open_inline_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
) -> impl IntoView {
    let menu = thread_menu(state, &thread, theme::THREAD_MENU).into_any();
    let first_row = chevron_row(
        state,
        CardId::Thread(thread.id.clone()),
        thread.outdated,
        menu,
    );
    let messages = messages_view(
        app,
        state,
        backend.clone(),
        thread.id.clone(),
        ThreadFace::Inline,
    )
    .scroll()
    .style(card_scroll);
    Stack::vertical((
        first_row.style(|s| s.flex_shrink(0.0_f32)),
        crate::review::cards::finding_controls(app, state, backend.clone(), thread.id.clone())
            .style(|s| s.flex_shrink(0.0_f32)),
        delete_confirmation(state, backend.clone(), thread.id.clone(), thread.is_github_bound())
            .style(|s| s.flex_shrink(0.0_f32)),
        messages,
        follow_up(app, state, backend, thread.id.clone(), ThreadFace::Inline)
            .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .padding(theme::THREAD_PADDING)
            .gap(theme::THREAD_GAP)
            .max_height(inline_card_max_height())
    })
}

/// How tall a card in the code may be: its rows at the cap, less the
/// inset inside its first and last rows and its border.
pub(crate) fn inline_card_max_height() -> f64 {
    crate::review::doc::CARD_MAX_ROWS as f64 * theme::CODE_LINE_HEIGHT as f64
        - 2.0 * theme::CARD_INSET
        - 2.0
}

/// The scroll a card's messages sit in, for the cap. floem keeps the
/// bar's width whether or not anything overflows, off the right of the
/// content for the vertical bar and under it for the horizontal one. The
/// messages take the card's whole width and never scroll sideways, so the
/// scroll reaches `CARD_SCROLLBAR` into the card's right padding, where
/// the bar draws at the cap, and keeps nothing under the messages.
pub(crate) fn card_scroll(s: floem::style::Style) -> floem::style::Style {
    s.flex_shrink(1.0_f32)
        .min_height(0.0)
        .overflow_x(floem::taffy::Overflow::Hidden)
        .scrollbar_width(theme::CARD_SCROLLBAR)
        .margin_right(-theme::CARD_SCROLLBAR)
}

/// The first row of a card in the code, where its Conversation card has an
/// anchor label: the chevron that collapses it, Outdated, and at the
/// end whatever the card puts there, the `...` menu on a thread of the
/// reader's own. The row is as tall as that menu whether or not it holds
/// one, so the chevron sits at one height on every card.
pub(crate) fn chevron_row(
    state: ReviewState,
    card_id: CardId,
    outdated: bool,
    trailing: floem::AnyView,
) -> impl IntoView {
    Stack::horizontal((
        chevron(state, card_id, true),
        outdated_pill(outdated),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        trailing,
    ))
    .style(|s| s.items_center().width_full().height(theme::THREAD_MENU))
}

/// A thread collapsed to its one row: who spoke first, the first line of
/// what they said, how many messages follow, Outdated, and the `...`
/// menu, smaller than the open card's so it fits the row.
fn collapsed_row(app: AppState, state: ReviewState, thread: CommentThread) -> impl IntoView {
    let first = thread.messages.first();
    let author = first.map(|m| m.author.clone()).unwrap_or(Author::User);
    let line = first
        .map(|m| crate::review::timeline::first_line(&m.body))
        .unwrap_or_default();
    // Set off from the pill before it as the pills are from what is
    // before them: the pill's fill and the menu's hover fill are one
    // colour, and would touch.
    let menu = thread_menu(state, &thread, theme::COLLAPSED_CARD_MENU)
        .style(|s| s.margin_left(theme::COLLAPSED_CARD_PILL_GAP))
        .into_any();
    collapsed_card_row(
        state,
        CardId::Thread(thread.id),
        move || author_name(app, &author),
        line,
        thread.messages.len().saturating_sub(1),
        thread.outdated,
        menu,
    )
}

/// A card in the code collapsed to its one row: the chevron, `author`,
/// the first `line` of the first message, how many `more` messages
/// follow, Outdated, and at the end whatever the card puts there. The
/// author is read each time it is drawn, since who the reader is can
/// land after the card is built.
pub(crate) fn collapsed_card_row(
    state: ReviewState,
    card_id: CardId,
    author: impl Fn() -> Name + 'static,
    line: String,
    more: usize,
    outdated: bool,
    trailing: floem::AnyView,
) -> impl IntoView {
    Stack::horizontal((
        chevron(state, card_id, false),
        name_label(author, |s| {
            s.font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
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
            move || crate::icons::sidebar_icon(SidebarIcon::ChevronRight, color()),
            color,
        )
        .style(move |s| {
            let base = s.size(theme::CHEVRON_GLYPH, theme::CHEVRON_GLYPH);
            if open {
                base.rotate(floem::style::Angle::QUARTER_TURN)
            } else {
                base
            }
        }),
    )
    .on_event_stop(el::Click, move |_, _| state.toggle_card(&card_id))
    .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
    .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
    .style(|s| {
        clickable(s)
            .size(theme::CHEVRON_SLOT, theme::CHEVRON_SLOT)
            .items_center()
            .justify_center()
            .border_radius(3.0)
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

/// The `...` at the end of a thread's first row, and the menu under it,
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
            "Delete Perch Messages\u{2026}"
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
/// a kept row is not rebuilt.
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
                        t.messages
                            .iter()
                            .zip(Lane::of(t))
                            .map(|(m, lane)| {
                                let opening = editing
                                    .as_ref()
                                    .filter(|(id, _)| *id == m.id)
                                    .map(|(_, opening)| *opening);
                                (m.clone(), opening, lane)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
        },
        // The key follows what the row shows: an edit on GitHub changes
        // the body, a draft posted gains its id, a draft being edited is
        // the composer, and a draft added to a thread that was wholly
        // private puts the rest of it in the lane.
        |(m, editing, lane)| {
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
            )
        },
        move |(m, editing, lane)| {
            let thread = state
                .comment_threads
                .with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
            match (thread, editing) {
                (Some(_), Some(_)) => match state.composer.get_untracked() {
                    Some(c) => composer_view(app, state, backend.clone(), c, face)
                        .style(|s| s.width_full())
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
    .style(move |s| s.flex_col().width_full().gap(face.message_gap()))
}

/// The follow-up composer under a thread when one is open for it, else the
/// line that opens one.
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
    // Keyed on whether a run is going as well as on the composer: the line
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
    dyn_container(
        move || (composer.get(), running.get()),
        move |(composer, _)| match composer {
            // A draft being edited has the composer in its own row, up
            // among the messages; nothing goes here.
            Some((_, _, Some(_))) => Empty::new().into_any(),
            // Under a thread the composer is a child of this container,
            // not a card placed by its edges, and takes the width here.
            Some(_) => match state.composer.get_untracked() {
                Some(c) => composer_view(app, state, backend.clone(), c, face)
                    .style(|s| s.width_full())
                    .into_any(),
                None => Empty::new().into_any(),
            },
            None => {
                let thread = state
                    .comment_threads
                    .with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
                match thread {
                    Some(thread) => reply_line(app, state, thread, face).into_any(),
                    None => Empty::new().into_any(),
                }
            }
        },
    )
    .style(|s| s.width_full())
}

/// One message: the text as markdown, and under an answer the run that
/// produced it, or under a question the run that is answering it. A
/// comment from GitHub carries its own byline, with Ask about this, the
/// reaction picker and its link on hover, and the reactions on it under.
/// A message in the private lane is drawn there, whole, with the lane's
/// label above the first of a run.
pub fn message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    message: CommentMessage,
    face: ThreadFace,
    lane: Lane,
) -> impl IntoView {
    let message = bare_message_view(app, state, backend, thread, message, face);
    match lane {
        Lane::Flush => message,
        Lane::PrivateStart => private_lane(
            Stack::vertical((perch_only_label(), message))
                .style(|s| s.flex_col().width_full().gap(6.0)),
        ),
        Lane::PrivateContinue => private_lane(message),
    }
}

/// The lane's inset and hairline around a private message, or the label
/// and the message together.
fn private_lane(inner: impl IntoView + 'static) -> floem::AnyView {
    Container::new(inner)
        .debug_name("private-lane")
        .style(|s| {
            s.width_full()
                .padding_left(theme::PRIVATE_LANE_INSET)
                .border_left(1.0)
                .border_color(theme::BORDER_MUTED)
        })
        .into_any()
}

/// `PERCH ONLY` at the top of a run of private messages. Upper case rather
/// than small caps, which floem's text cannot set.
fn perch_only_label() -> impl IntoView {
    Label::derived(|| "PERCH ONLY".to_string())
        .debug_name("perch-only")
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_MUTED)
                .selectable(false)
        })
}

/// A message on its own, before any lane: a comment from GitHub with its
/// byline, a draft with the Draft mark, or the reader's question and the
/// agent's answer with their runs.
fn bare_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    message: CommentMessage,
    face: ThreadFace,
) -> floem::AnyView {
    if let (ThreadOrigin::GitHub, Author::GitHubUser(login)) = (message.origin, &message.author) {
        return github_message_view(app, state, backend, &thread, &message, login.clone())
            .into_any();
    }
    if message.is_draft() {
        return draft_message_view(app, state, backend, &thread, &message).into_any();
    }
    let is_agent = matches!(message.author, Author::Agent { .. });
    let body = message.body.clone();

    // Questions and answers are both markdown; the reader writes it too.
    // A finished run sits on the answer, and reads in the order it went:
    // what the agent did, then what it said, then what it cost. While it
    // runs, or when it failed, the run sits under the question whole.
    let finished = message
        .run
        .as_ref()
        .is_some_and(|r| r.status == RunStatus::Completed);
    let message = match message.run.clone() {
        Some(run) if is_agent && finished => Stack::vertical((
            run_surface(steps_view(state, run.clone())),
            crate::review::markdown::render(&body),
            run_footer(app, state, backend, thread, message.body.clone(), run, face),
        ))
        .style(|s| s.flex_col().width_full().gap(8.0))
        .into_any(),
        Some(run) => Stack::vertical((
            crate::review::markdown::render(&body),
            run_block(app, state, backend, thread, message.body.clone(), run, face),
        ))
        .style(|s| s.flex_col().width_full().gap(8.0))
        .into_any(),
        None => crate::review::markdown::render(&body).into_any(),
    };

    if face == ThreadFace::Card && !is_agent {
        Container::new(message)
            .style(|s| {
                s.width_full()
                    .padding(14.0)
                    .background(theme::CANVAS_SUBTLE)
                    .border_radius(12.0)
            })
            .into_any()
    } else {
        message
    }
}

/// A comment from GitHub in a thread: a small byline, then the body, then
/// the reactions on it. A reply, any comment after the thread's first,
/// sits flush under the one it answers with a rule down its left. Ask
/// about this opens the composer under the thread itself, so the answer
/// lands beside the comment it is about; the thread's history carries
/// the comment to the agent, and nothing is quoted.
fn github_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    message: &CommentMessage,
    login: String,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let reply = thread.messages.first().is_some_and(|m| m.id != message.id);
    let who = cards::Byline {
        login: login.clone(),
        avatar_url: cards::avatar_url_of(state, login.clone()),
        avatar_size: theme::NESTED_AVATAR,
        glyph: None,
        verb: String::new(),
        when: Some(message.created_at),
    };
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
    let react = message.node_id.as_deref().and_then(cards::reactable);
    let url = message.html_url.clone().unwrap_or_default();
    let reactions = cards::reactions_under(state, backend.clone(), &react);
    cards::track_hover(
        Stack::vertical((
            cards::byline(
                app,
                who,
                cards::actions(state, backend, hovered, Some(ask), react, url),
            ),
            crate::review::markdown::render(&message.body),
            reactions,
        ))
        .style(move |s| {
            let base = s.flex_col().width_full().gap(6.0);
            if reply {
                base.padding_left(12.0)
                    .border_left(theme::REPLY_RULE)
                    .border_color(theme::BORDER_MUTED)
            } else {
                base
            }
        }),
        hovered,
    )
}

/// The reader's message on its way to GitHub: their own byline with the
/// Draft mark after the name and no time, since it has not gone
/// anywhere yet; edit and delete on hover, and the body. Flush with the
/// comments it answers, whether it is the thread's first or a reply,
/// since it is theirs and not a rule under someone else's. Edit opens
/// the composer in this row with the text; delete asks nothing, a draft
/// being Perch's alone.
fn draft_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    message: &CommentMessage,
) -> impl IntoView {
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
        hover_icon(hovered, SidebarIcon::Pencil, "edit-draft", edit),
        hover_icon(hovered, SidebarIcon::Trash, "delete-draft", delete),
    ))
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any();
    cards::track_hover(
        Stack::vertical((
            draft_byline(app, actions),
            crate::review::markdown::render(&message.body),
        ))
        .style(|s| s.flex_col().width_full().gap(6.0)),
        hovered,
    )
}

/// The reader's face and name, as the thread byline draws them, with
/// `Draft` after the name and whatever the caller puts at the right.
fn draft_byline(app: AppState, trailing: floem::AnyView) -> impl IntoView {
    let viewer = move || app.viewer.get().filter(|(l, _)| !l.is_empty());
    let seed = viewer().map(|(l, _)| l).unwrap_or_else(|| "you".into());
    Stack::horizontal((
        crate::ui::widgets::avatar(
            app,
            move || viewer().map(|(_, a)| a).unwrap_or_default(),
            theme::NESTED_AVATAR,
            seed,
        )
        .style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        name_label(
            move || match viewer() {
                Some((login, _)) => Name::Login(login),
                None => Name::Plain("You".into()),
            },
            |s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
            },
        ),
        draft_pill(),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        trailing,
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// The Draft mark after a draft's author, in GitHub's colour: this is
/// going there.
pub fn draft_pill() -> impl IntoView {
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
        })
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
pub fn run_block(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    question: String,
    run: RunRecord,
    face: ThreadFace,
) -> impl IntoView {
    let partial = run
        .final_result
        .clone()
        .filter(|text| run.status != RunStatus::Completed && !text.trim().is_empty());
    let response = match partial {
        Some(text) => Container::new(crate::review::markdown::render(&text))
            .style(|s| {
                s.width_full()
                    .padding_top(7.0)
                    .border_top(1.0)
                    .border_color(theme::BORDER_MUTED)
            })
            .into_any(),
        None => Empty::new().into_any(),
    };
    run_surface(
        Stack::vertical((
            steps_view(state, run.clone()),
            response,
            run_footer(app, state, backend, thread, question, run, face),
        ))
        .style(|s| s.flex_col().width_full().gap(6.0)),
    )
}

/// The box a run's lines sit in. Tool activity is an inset neutral surface,
/// separate from the prose without becoming a second heavy card.
fn run_surface(inner: impl IntoView + 'static) -> impl IntoView {
    Container::new(inner).style(|s| {
        s.width_full()
            .padding(10.0)
            .border_radius(12.0)
            .background(theme::CANVAS_SUBTLE)
    })
}

/// The footer under a run: Stop while it goes, cost and time once it is
/// done, the error and Retry when it is not.
fn run_footer(
    _app: AppState,
    _state: ReviewState,
    backend: Rc<Backend>,
    _thread: CommentThread,
    _question: String,
    run: RunRecord,
    face: ThreadFace,
) -> floem::AnyView {
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
            // A Conversation card's byline already names the model and the
            // effort; the card in the code has no byline, so its footer says.
            match face {
                ThreadFace::Inline => {
                    format!(
                        "{} \u{00b7} {effort} \u{00b7} {secs:.0}s{cost}",
                        model_label(&run.model)
                    )
                }
                ThreadFace::Card => format!("{secs:.0}s{cost}"),
            }
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
                sidebar_glyph(SidebarIcon::ChevronRight, theme::FG_SUBTLE).style(move |s| {
                    let base = s.size(10.0, 10.0).margin_right(5.0);
                    if expanded_chevron() {
                        base.rotate(floem::style::Angle::QUARTER_TURN)
                    } else {
                        base
                    }
                }),
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

/// The one-line invitation at the bottom of a thread.
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
            let base = clickable(s)
                .font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .width_full()
                .padding_vert(4.0)
                .hover(|s| s.color(theme::ACCENT));
            // On a Conversation card the line stands where the composer's
            // box will be, and takes the box's shape. In the code it is a
            // bare line, one row of the card's estimate.
            match face {
                ThreadFace::Inline => base,
                ThreadFace::Card => base
                    .padding_horiz(12.0)
                    .height(theme::ASK_BOX_HEIGHT - 12.0)
                    .items_center()
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER_MUTED)
                    .border_radius(theme::RADIUS)
                    .hover(|s| s.color(theme::ACCENT).border_color(theme::BORDER)),
            }
        })
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
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
    /// lines opens on GitHub.
    #[test]
    fn a_composer_on_lines_sent_to_github_keeps_a_draft_with_the_anchor_and_no_thread() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
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

        state.open_composer(None, Some(anchor), String::new(), sonnet_medium());
        assert_eq!(state.composer.get().unwrap().destination, Destination::GitHub);
        state.open_composer(None, None, String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::agent(sonnet_medium()),
            "about the item as a whole, an agent"
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
    }

    /// A follow-up follows the thread's last message: under rohan's
    /// comment it is bound for GitHub, under the agent's answer for that
    /// agent at its run's effort. Ask about this on rohan's comment goes
    /// to an agent whatever the thread ends in.
    #[test]
    fn a_follow_up_follows_the_last_message_and_ask_about_this_goes_to_an_agent() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
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

    /// Under rohan's comment the question and the answer are in the lane,
    /// the answer continuing the question's run; the draft after them is
    /// flush. A thread that is wholly Perch's has no lane, and a draft
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
    /// are drawn in the lane, inset with the rule, and `PERCH ONLY` sits
    /// once above the question; rohan's comment and the draft are flush.
    /// The same on both faces.
    #[test]
    fn private_messages_under_a_github_comment_are_drawn_in_the_lane_with_one_label() {
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

            let lanes = find_all(root_id, "private-lane");
            assert_eq!(lanes.len(), 2, "{face:?}: the question and the answer");
            let labels = find_all(root_id, "perch-only");
            assert_eq!(labels.len(), 1, "{face:?}: one label for the run");
            assert!(find(lanes[0], "perch-only").is_some(), "{face:?}: the label is in the first lane");
            assert!(find(lanes[1], "perch-only").is_none(), "{face:?}: and not the second");
            assert!(harness.get_layout_rect(labels[0]).width() > 0.0, "{face:?}: the label is drawn");

            // The rows are siblings in the stack, so their rects share a
            // frame: a lane's content sits inset past the rule, and every
            // other row's starts at the thread's edge.
            let stack = lanes[0].parent().expect("the lane is a row of the stack");
            let rows = stack.children();
            assert_eq!(rows.len(), 4, "{face:?}: a row per message");
            for row in &rows {
                let content = harness.get_content_rect(*row);
                assert!(content.width() > 0.0, "{face:?}: the row is laid out");
                let expected = if lanes.contains(row) { theme::PRIVATE_LANE_INSET + 1.0 } else { 0.0 };
                assert_eq!(content.x0, expected, "{face:?}: the row's inset");
            }

            // The draft and rohan's comment are not in any lane.
            let pill = find(root_id, "draft-pill").expect("the draft");
            assert!(lanes.iter().all(|l| find(*l, "draft-pill").is_none()), "{face:?}: the draft is flush");
            assert!(harness.get_layout_rect(pill).width() > 0.0);
            assert!(find(root_id, "Label: \"Ask about this\"").is_some(), "rohan's comment");
            assert!(lanes.iter().all(|l| find(*l, "Label: \"Ask about this\"").is_none()), "{face:?}: rohan is flush");
        }
    }

    /// A thread that is wholly Perch's, as every thread was before drafts,
    /// draws no lane and no label: the inset would say nothing there.
    #[test]
    fn a_wholly_private_thread_has_no_lane() {
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
        assert!(find(root_id, "private-lane").is_none());
        assert!(find(root_id, "perch-only").is_none());

        // A draft added under it puts what was there in the lane.
        let mut with_draft = thread.clone();
        let mut draft = draft_under(&with_draft);
        draft.thread_id = "t1".into();
        with_draft.messages.push(draft);
        state.comment_threads.set(Arc::new(vec![with_draft]));
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert_eq!(find_all(root_id, "private-lane").len(), 2, "the question and the answer moved into the lane");
        assert_eq!(find_all(root_id, "perch-only").len(), 1);
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
}
