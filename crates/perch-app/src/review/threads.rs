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
use floem::style::CursorStyle;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use perch_github::{
    Author, CommentMessage, CommentThread, IssueCloseReason, RunRecord, RunStatus, ThreadOrigin,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards;
use crate::review::doc::CardId;
use crate::review::model::{AgentChoice, Composer, Destination, ReviewState};
use crate::review::timeline::count_of;
use crate::state::AppState;
use crate::theme;
use crate::ui::menu::MenuExt;
use crate::ui::widgets::{
    badge, clickable, confirmation, editor_box, icon_button, name_label, prose_editor,
    sidebar_glyph, text_button, Name,
};

/// A model as the screen shows it, without its maker: "Sonnet" for the
/// alias `sonnet`, "Opus 5.5" for `claude-opus-5-5`, "Sol 6.1" for
/// `gpt-6.1-sol` and "GPT 5.5" for `gpt-5.5`. The date on the end of an
/// id such as `claude-haiku-4-5-20251001` goes; the version says it.
pub fn model_label(model: &str) -> String {
    // Claude Code can report the context window on the end: `[1m]`.
    let model = model.split('[').next().unwrap_or_default().trim();
    let mut words = Vec::new();
    let mut version = Vec::new();
    let mut gpt = false;
    for part in model.split('-').filter(|part| !part.is_empty()) {
        if part.eq_ignore_ascii_case("claude") || part.eq_ignore_ascii_case("codex") {
            // The maker, which `model_name` puts first.
        } else if part.eq_ignore_ascii_case("gpt") {
            gpt = true;
        } else if part.len() >= 6 && part.chars().all(|c| c.is_ascii_digit()) {
            // A release date.
        } else if part.starts_with(|c: char| c.is_ascii_digit()) {
            version.push(part);
        } else if part.chars().all(|c| c.is_ascii_alphabetic()) {
            let mut chars = part.chars();
            let first = chars.next().map(|c| c.to_ascii_uppercase());
            words.push(
                first
                    .into_iter()
                    .chain(chars.map(|c| c.to_ascii_lowercase()))
                    .collect(),
            );
        } else {
            words.push(part.to_string());
        }
    }
    if words.is_empty() && gpt {
        words.push("GPT".to_string());
    }
    if words.is_empty() && version.is_empty() {
        return model.to_string();
    }
    if !version.is_empty() {
        words.push(version.join("."));
    }
    words.join(" ")
}

/// A model with its maker first: "Claude Opus 5.5", "Codex Sol 6.1". The
/// maker is the backend's, or the id's when the backend is not known.
pub fn model_name(backend: &str, model: &str) -> String {
    let maker = match perch_agent::Backend::parse(backend) {
        Some(perch_agent::Backend::ClaudeCode) => Some("Claude"),
        Some(perch_agent::Backend::Codex) => Some("Codex"),
        None if backend.eq_ignore_ascii_case("claude") => Some("Claude"),
        None => {
            let model = model.trim().to_ascii_lowercase();
            if model.starts_with("claude") {
                Some("Claude")
            } else if model.starts_with("gpt") {
                Some("Codex")
            } else {
                None
            }
        }
    };
    let label = model_label(model);
    match maker {
        Some(maker) if label.is_empty() => maker.to_string(),
        Some(maker) => format!("{maker} {label}"),
        None => label,
    }
}

/// Who ran, as a byline names them: "Claude Opus 5.5 (high)",
/// "Codex Sol 6.1 (xhigh)".
pub fn agent_name(backend: &str, model: &str, effort: &str) -> String {
    let name = model_name(backend, model);
    match effort.trim() {
        "" => name,
        effort => format!("{name} ({})", effort.to_ascii_lowercase()),
    }
}

/// A run's [`agent_name`]: the model and effort it ran with, else, until
/// the CLI says, the ones it asked for.
pub fn run_agent_name(run: &RunRecord) -> String {
    let or_requested = |used: &str, requested: &str| {
        if used.trim().is_empty() {
            requested
        } else {
            used
        }
        .to_string()
    };
    agent_name(
        &run.backend,
        &or_requested(&run.model, &run.requested_model),
        &or_requested(&run.effort, &run.requested_effort),
    )
}

/// Whether the command a box's keymap resolved a key to is Cmd-Enter
/// (Ctrl-Enter off macOS), which sends. floem binds Cmd-Enter to
/// `NewLineBelow`, and Ctrl-Enter on Linux and Windows to `NewLineAbove`,
/// commands of their own, not to Enter's `InsertNewLine` with the
/// modifier held; a box for a question has no use for a line opened by
/// the caret, so any of them with the command key held is Send.
pub fn cmd_enter(cmd: &floem::views::editor::text_document::PreCommand<'_>) -> bool {
    use floem::views::editor::command::Command;
    use floem::views::editor::core::command::EditCommand;

    let opens_a_line = match cmd.cmd {
        Command::Edit(EditCommand::InsertNewLine | EditCommand::NewLineBelow) => true,
        Command::Edit(EditCommand::NewLineAbove) => !cfg!(target_os = "macos"),
        _ => false,
    };
    opens_a_line && cmd.mods.contains(crate::ui::OS_MOD)
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

/// Where a message is in its thread: the first, over which the line down
/// the avatars starts, and the last, under which it stops.
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
    // A walkthrough place's Follow up is a new thread, typed as a reply.
    let follow_up = composer.thread_id.is_some() || composer.step.is_some();
    // Under a thread whose agent is at work, what is typed goes to that
    // run, which reads it after its current step. The box is built again
    // when the run ends, as a follow-up's. An edit is never a steer: it
    // saves the message it is in place of.
    let edit_kind = edit_kind(state, &composer);
    let save_label = edit_kind.clone().and_then(EditKind::save_label);
    let editing = composer.editing.is_some();
    let steering_run = composer
        .thread_id
        .as_ref()
        .filter(|_| !editing)
        .and_then(|id| {
            state.comment_threads.with_untracked(|threads| {
                threads
                    .iter()
                    .find(|t| &t.id == id)
                    .and_then(|t| t.running_run().map(|run| run.id.clone()))
            })
        });
    let steering = steering_run.is_some();
    let editing_placeholder = edit_kind.as_ref().and_then(EditKind::placeholder);
    let placeholder = if let Some(placeholder) = editing_placeholder {
        placeholder.to_string()
    } else if steering {
        "Steer…".to_string()
    } else if follow_up {
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
    // Markdown as it is typed: bold, italics, code and links take their
    // styles in the box, the way they will in the thread.
    let editor = prose_editor(draft.clone(), placeholder.clone(), Some(state.next_token()));
    let doc_editor = editor.editor().clone();
    let read_text = Rc::new(move || doc_editor.doc().text().to_string());
    // `@` offers people and files. A file goes to an agent as `@path`, as
    // Claude Code and Codex take one, and to GitHub as a code span, where
    // `@` would mention someone.
    let mentioner =
        crate::review::mention::attach(app, state, backend.clone(), editor.editor(), move |path| {
            let github = state
                .composer
                .with_untracked(|c| c.as_ref().is_some_and(|c| c.destination.is_github()));
            if github {
                format!("`{path}` ")
            } else {
                format!("@{path} ")
            }
        });
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
            // A close needs nothing said, nor does a description, which
            // may be emptied; everything else does.
            let closing =
                state.composer_confirming.get_untracked() && state.composer_closing.get_untracked();
            let description = edit_kind.as_ref().is_some_and(EditKind::edits_description);
            if question.trim().is_empty() && !closing && !description {
                return;
            }
            // A run that has ended since takes it as a follow-up.
            if let Some(run_id) = &steering_run {
                if !question.trim().is_empty() {
                    backend.steer_run(run_id, question.trim());
                    state.composer_sent();
                }
                return;
            }
            let Some(composer) = state.composer.get() else {
                return;
            };
            // A walkthrough place's Follow up starts its thread with the
            // note it asks about.
            let question = match &composer.step {
                Some(step) => {
                    crate::review::walkthrough::follow_up_preface(state, step) + question.trim()
                }
                None => question,
            };
            let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
                return;
            };
            // A reviewer's words, edited, stay where they are, Perch's. A
            // comment on GitHub shows its new words at once and goes back
            // to GitHub with them, keeping the whitespace it opens with,
            // an indented code block's.
            match (&edit_kind, &composer.editing) {
                (Some(EditKind::Reviewer), Some(id)) => {
                    backend.edit_reviewer_message(&item_id, id, question.trim());
                    state.composer_sent();
                    return;
                }
                (Some(EditKind::Posted { node_id, place }), _) => {
                    let body = question.trim_end();
                    state.rewrite_github_comment(node_id, body);
                    backend.update_github_comment(&item_id, node_id, *place, body);
                    state.composer_sent();
                    return;
                }
                _ => {}
            }
            // An issue comment posts at once, so it is asked about first:
            // the footer names the pull request or issue, and the next
            // send goes.
            // A close was asked about when its button was clicked.
            if composer.destination.is_github() && state.posts_issue_comment(&composer) {
                if !state.composer_confirming.get_untracked() {
                    state.composer_closing.set(false);
                    state.composer_confirming.set(true);
                    return;
                }
                let body = question.trim().to_string();
                if closing {
                    let token = if state.pull_request.with_untracked(Option::is_some) {
                        backend.close_pull_request(&item_id, &body)
                    } else {
                        let reason = state.composer_close_reason.get_untracked();
                        backend.close_issue(&item_id, &body, reason)
                    };
                    state.last_destination[face.index()].set(Some(composer.destination.clone()));
                    state.start_closing(token, body);
                    return;
                }
                let token = backend.post_issue_comment(&item_id, &body);
                state.last_destination[face.index()].set(Some(composer.destination.clone()));
                state.start_posting(token, body);
                return;
            }
            // A reply under GitHub's thread posts at once too, as
            // github.com's Reply does, unasked: it lands where it was
            // written.
            if composer.destination.is_github() && state.posts_reply(&composer) {
                if let Some(thread_id) = &composer.thread_id {
                    let body = question.trim().to_string();
                    let token = backend.reply_to_thread(&item_id, thread_id, &body);
                    state.last_destination[face.index()].set(Some(composer.destination.clone()));
                    state.start_posting(token, body);
                }
                return;
            }
            // On a worktree GitHub has no pull request for, a suggestion
            // is the reader's own edit: it goes into the file at once,
            // as Commit suggestion puts it there on github.com.
            if composer.anchor.is_some() && state.pull_request.with_untracked(Option::is_none) {
                if let Some(lines) = crate::review::suggestion::only_suggestion(&question) {
                    match Effect::untrack(|| composer_original(state)) {
                        Some((anchor, original)) => {
                            let token =
                                backend.apply_suggestion(&item_id, &anchor.path, original, lines);
                            state.start_applying(token, question);
                        }
                        None => state.composer_error.set(Some(
                            "Perch could not read the lines this suggestion replaces".into(),
                        )),
                    }
                    return;
                }
            }
            // Work on an issue is Start work: a branch of the reader's own
            // from the default branch, whose composer waits with what was
            // written here.
            if composer.mode == perch_github::RunMode::Work
                && Effect::untrack(|| start_work_offered(state))
            {
                let mut instructions = question.trim().to_string();
                if let Some(anchor) = &composer.anchor {
                    instructions = format!("About `{}`:\n\n{instructions}", anchor.label());
                }
                state.composer_sent();
                crate::review::start_work::start_work_with(
                    app,
                    &backend,
                    &item_id,
                    Some(instructions),
                );
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
                    state.agent_last.set(Some((*agent, model.clone(), *effort)));
                }
                // A new thread's lines picked under a scope other than
                // Branch are numbered by its diff, which GitHub does not
                // have: they are moved to GitHub's first, and the box
                // waits for that.
                Destination::GitHub
                    if composer.editing.is_none()
                        && composer.picked.is_some()
                        && composer.anchor.is_some() =>
                {
                    let (Some(anchor), Some(picked)) =
                        (composer.anchor.clone(), composer.picked.clone())
                    else {
                        return;
                    };
                    let body = question.trim().to_string();
                    let token = backend.save_draft_in_scope(&item_id, anchor, &body, picked);
                    state.last_destination[face.index()].set(Some(composer.destination.clone()));
                    state.start_posting(token, body);
                    return;
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
    // A composer on lines has Suggest, on its thread's lines where it is
    // a follow-up, since those are the ones GitHub's hunk has.
    let tabs = crate::review::markdown_toolbar::Tabs::new(editor.editor(), posting_here)
        .suggesting(move || composer_original(state).map(|(_, original)| original));
    let key_tabs = tabs.clone();
    let editor = editor
        // Cmd-Enter sends, before the editor can take it for a line. While
        // the footer is asking whether to post, Enter alone is the yes;
        // while the comment goes, the box is read-only and neither sends.
        .pre_command(move |cmd| {
            if posting_here_now() {
                return CommandExecuted::No;
            }
            if cmd_enter(&cmd) || (plain_enter(&cmd) && state.composer_confirming.get_untracked()) {
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
                if empty
                    && state.composer_confirming.get_untracked()
                    && !state.composer_closing.get_untracked()
                {
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
                if key_tabs.key(key) {
                    return floem::event::EventPropagation::Stop;
                }
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    // Escape takes the question back first, with the
                    // text intact; while posting there is nothing to do.
                    if posting_here_now() {
                        return floem::event::EventPropagation::Stop;
                    }
                    if state.close_menu.with_untracked(Option::is_some) {
                        crate::review::close_menu::back(state);
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
        })
        // The box draws its placeholder itself, in no view of its own. The
        // name is lower-cased and dashed, so a search for words as the
        // screen shows them does not find it.
        .debug_name(format!(
            "composer-placeholder:{}",
            placeholder.to_lowercase().replace(' ', "-")
        ));
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
    // `a` can find it and scrolls the page to the whole of itself, Send
    // included: it sits at the end of the conversation, or under a thread,
    // and may have opened off screen or only its top on it.
    let inner = editor.editor().editor_view_id;
    let whole = std::rc::Rc::new(std::cell::Cell::new(None::<floem::ViewId>));
    let whole_set = whole.clone();
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
                whole.get().unwrap_or(id).scroll_to(None);
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

    let whole_box = Stack::vertical((
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Thread, theme::accent)
                .style(|s| s.size(13.0, 13.0).margin_right(6.0)),
            Label::derived(move || anchor_label.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::fg_muted())
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
                theme::fg_subtle,
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
        tabs.view(
            state,
            resizable_editor_box(Grip::composer(state), editor, face == ThreadFace::Inline),
            theme::COMPOSER_MAX_HEIGHT,
        ),
        composer_error_line(state),
        composer_footer(app, state, send, posting_here, steering, save_label),
        resize_cursor_sheet(Grip::composer(state)),
    ))
    .debug_name("composer-box")
    .style(move |s| {
        let base = s.flex_col().padding(12.0).border(1.0).border_radius(14.0);
        let base = match face {
            ThreadFace::Card => base
                .width_full()
                .background(theme::canvas_subtle())
                .border_color(theme::border_muted()),
            // A card in the code is placed by its edges, and its width
            // follows from them.
            ThreadFace::Inline => base
                .background(theme::canvas())
                .border_color(theme::accent_ring()),
        };
        // Under a thread it is the thread's reply form, as GitHub's is:
        // white, in the thread's rule, with a box's corners.
        base.apply_if(follow_up, |s| {
            s.background(theme::canvas())
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
        })
    });
    whole_set.set(Some(floem::View::id(&whole_box)));
    whole_box
}

/// A box the reader drags taller or shorter by the grip along the
/// inside of its bottom edge: where the dragged height is kept, and the
/// heights the box keeps to.
///
/// Until it is dragged the box opens at `opens_at` and grows with what
/// is typed as far as `grows_to`, as GitHub's does. A drag fixes it at a
/// height of the reader's, between `shortest` and `tallest`, which it
/// keeps whatever is typed into it, and which the next such box opens at.
#[derive(Clone, Copy)]
pub(crate) struct Grip {
    pub height: RwSignal<Option<f64>>,
    /// Whether the box's bottom edge is being dragged, for the sheet
    /// that keeps the cursor up.
    pub resizing: RwSignal<bool>,
    pub opens_at: f64,
    pub grows_to: f64,
    pub shortest: f64,
    pub tallest: f64,
}

impl Grip {
    /// The composer's box, five lines tall until dragged.
    fn composer(state: ReviewState) -> Self {
        Self {
            height: state.composer_height,
            resizing: state.composer_resizing,
            opens_at: theme::COMPOSER_HEIGHT,
            grows_to: theme::COMPOSER_GROWN_HEIGHT,
            shortest: theme::COMPOSER_MIN_HEIGHT,
            tallest: theme::COMPOSER_MAX_HEIGHT,
        }
    }
}

/// An editor in its box, with the grip along the inside of its bottom
/// edge that drags it taller or shorter.
pub(crate) fn resizable_editor_box(
    grip: Grip,
    editor: impl IntoView + 'static,
    bordered: bool,
) -> impl IntoView {
    let boxed = editor_box(
        editor,
        move || match grip.height.get() {
            Some(height) => (height, height),
            None => (grip.opens_at, grip.grows_to),
        },
        bordered,
    );
    let box_id = floem::View::id(&boxed);
    Stack::vertical((boxed, resize_handle(grip, box_id)))
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
fn resize_handle(grip: Grip, box_id: floem::ViewId) -> impl IntoView {
    use floem::event::DragConfig;
    use floem::style::Position;

    let grab = RwSignal::new(grip.opens_at);
    let hovered = RwSignal::new(false);

    // The grip is drawn by a child of the band and takes no pointer
    // events of its own: capture goes to whatever the pointer went down
    // on, and a drag from the middle of the band must be the band's like
    // one from either end.
    Container::new(Empty::new().style(move |s| {
        s.width(theme::COMPOSER_GRIP_WIDTH)
            .height(theme::COMPOSER_GRIP_MARK)
            .border_radius(theme::COMPOSER_GRIP_MARK / 2.0)
            .pointer_events_none()
            .background(match (grip.resizing.get(), hovered.get()) {
                (true, _) => theme::accent(),
                (false, true) => theme::fg_subtle(),
                (false, false) => theme::border(),
            })
    }))
    .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
    .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
    .on_event_stop(el::PointerDown, move |cx, e: &PointerButtonEvent| {
        grab.set(
            grip.height
                .get_untracked()
                .or_else(|| box_id.get_size().map(|size| size.height))
                .unwrap_or(grip.opens_at),
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
        grip.resizing.set(true);
    })
    .on_event_stop(el::DragMove, move |_, e: &floem::event::DragMoveEvent| {
        let dy = e.current_state.logical_point().y - e.start_state.logical_point().y;
        let height = (grab.get_untracked() + dy).clamp(grip.shortest, grip.tallest);
        grip.height.set(Some(height));
    })
    .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
        grip.resizing.set(false);
    })
    .on_event_stop(
        el::DragCancel,
        move |_, _: &floem::event::DragCancelEvent| {
            grip.resizing.set(false);
        },
    )
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
pub(crate) fn resize_cursor_sheet(grip: Grip) -> impl IntoView {
    use floem::style::Position;

    Empty::new()
        .debug_name("composer resize cursor sheet")
        .style(move |s| {
            s.position(Position::Absolute)
                .inset_left(0.0)
                .inset_right(0.0)
                .inset_top(-grip.tallest)
                .inset_bottom(-grip.tallest)
                .z_index(100)
                .cursor(CursorStyle::RowResize)
                .apply_if(!grip.resizing.get(), |s| s.hide())
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
                        .color(theme::closed())
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
/// github.com/owner/repo/pull/12?` (or `.../issues/12`) with Post and
/// Cancel, and after Close
/// with comment it asks whether to close; while this composer's comment
/// or close goes, it says so and offers nothing. A reviewer's message
/// or a comment on GitHub being edited has nowhere else to go, so it has
/// `save` alone.
fn composer_footer(
    app: AppState,
    state: ReviewState,
    send: Rc<dyn Fn()>,
    posting: impl Fn() -> bool + Copy + 'static,
    steering: bool,
    save: Option<&'static str>,
) -> impl IntoView {
    if let Some(save) = save {
        return Stack::horizontal((
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            send_button(move || save.to_string(), send).debug_name("composer-send"),
        ))
        .debug_name("composer-edit-footer")
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0))
        .into_any();
    }
    // A steer goes to the agent at work, whatever the pickers say, so it
    // has none: only when the agent reads it.
    if steering {
        return Stack::horizontal((
            Label::derived(|| "The agent reads this after its current step".to_string()).style(
                |s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_subtle())
                        .text_ellipsis()
                        .min_width(0.0)
                        .selectable(false)
                },
            ),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            send_button(|| "Send".to_string(), send).debug_name("composer-send"),
        ))
        .debug_name("composer-steer-footer")
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0))
        .into_any();
    }
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
            crate::review::close_menu::split_button(app, state),
            send_button(move || send_label(state), send.clone()).debug_name("composer-send"),
        ))
        // In a narrow column the buttons that close go under the pickers,
        // at the right, rather than past the card's edge.
        .style(|s| {
            s.items_center()
                .width_full()
                .gap(6.0)
                .flex_wrap(floem::taffy::FlexWrap::Wrap)
                .justify_end()
                .margin_top(8.0)
        })
        .into_any(),
        FooterMode::Confirming => Stack::horizontal((
            Label::derived(move || {
                let pull = state.pull_request.with(|pull| {
                    pull.as_ref().map(|pull| {
                        format!("github.com/{}/pull/{}", pull.repo_full_name, pull.number)
                    })
                });
                let issue = move || {
                    state.issue.with(|issue| {
                        issue.as_ref().map(|issue| {
                            format!(
                                "github.com/{}/issues/{}",
                                issue.repo_full_name, issue.number
                            )
                        })
                    })
                };
                let target = pull.or_else(issue).unwrap_or_else(|| "GitHub".to_string());
                // An issue says why it closes, but for completed, the
                // close GitHub takes a plain one for.
                let why = if state.issue.with(Option::is_some) {
                    match state.composer_close_reason.get() {
                        IssueCloseReason::Completed => String::new(),
                        IssueCloseReason::NotPlanned => " as not planned".to_string(),
                        IssueCloseReason::Duplicate(n) => format!(" as a duplicate of #{n}"),
                    }
                } else {
                    String::new()
                };
                if !state.composer_closing.get() {
                    format!("Post to {target}?")
                } else if state.composer_draft.with(|d| d.trim().is_empty()) {
                    format!("Close {target}{why}?")
                } else {
                    format!("Comment on and close {target}{why}?")
                }
            })
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            })
            .debug_name("composer-confirm"),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            text_button(
                || "Cancel".to_string(),
                SidebarIcon::Close,
                move || state.composer_confirming.set(false),
            )
            .debug_name("composer-cancel-post"),
            send_button(
                move || {
                    if state.composer_closing.get() {
                        "Close".to_string()
                    } else {
                        "Post".to_string()
                    }
                },
                send.clone(),
            )
            .debug_name("composer-post"),
        ))
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0))
        .into_any(),
        FooterMode::Posting => Stack::horizontal((
            Label::derived(move || {
                let (closing, applying) = state.posting.with(|p| {
                    p.as_ref()
                        .map_or((false, false), |p| (p.closing, p.applying))
                });
                if closing {
                    "Closing\u{2026}".to_string()
                } else if applying {
                    "Applying\u{2026}".to_string()
                } else {
                    "Posting\u{2026}".to_string()
                }
            })
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .selectable(false)
            })
            .debug_name("composer-posting"),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
        ))
        .style(|s| s.items_center().width_full().height(30.0).margin_top(8.0))
        .into_any(),
    })
    .into_any()
}

/// Whether the open composer offers to close its pull request or issue.
/// Tracked.
pub(crate) fn close_offered(app: AppState, state: ReviewState) -> bool {
    pull_request_close_offered(app, state) || issue_close_offered(app, state)
}

/// Whether the open composer's text goes to the conversation as an issue
/// comment while GitHub is reachable, as a close from it needs. Tracked.
fn closes_from_composer(state: ReviewState) -> bool {
    let to_conversation = state.composer.with(|c| {
        c.as_ref()
            .is_some_and(|c| c.destination.is_github() && state.posts_issue_comment(c))
    });
    to_conversation && !state.offline.get()
}

/// Whether the open composer offers to close the issue: the issue is
/// open and GitHub says the reader may close it, or, until it says, the
/// reader wrote it. Tracked.
fn issue_close_offered(app: AppState, state: ReviewState) -> bool {
    if !closes_from_composer(state) {
        return false;
    }
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    let closable = state.issue_closable.get();
    state.issue.with(|issue| {
        issue.as_ref().is_some_and(|issue| {
            issue.is_open()
                && closable.unwrap_or(viewer.as_deref() == Some(issue.author_login.as_str()))
        })
    })
}

/// Whether the open composer offers to close the pull request: it goes
/// to the conversation, the pull request is open, and the reader wrote
/// it or may write to its repository. Tracked.
pub(crate) fn pull_request_close_offered(app: AppState, state: ReviewState) -> bool {
    use perch_github::RepositoryPermission;

    if !closes_from_composer(state) {
        return false;
    }
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    let (open, author) = state.pull_request.with(|pull| match pull {
        Some(pull) => (
            pull.is_open() && !pull.merged,
            viewer.as_deref() == Some(pull.author_login.as_str()),
        ),
        None => (false, false),
    });
    let may_write = state.pull_request_overview.with(|overview| {
        overview.as_ref().is_some_and(|overview| {
            matches!(
                overview.viewer_permission,
                Some(
                    RepositoryPermission::Admin
                        | RepositoryPermission::Maintain
                        | RepositoryPermission::Write
                        | RepositoryPermission::Triage
                )
            )
        })
    });
    open && (author || may_write)
}

/// What the send does, on its button: `Send` to an agent, `Add to
/// review` for a draft, `Comment` for an issue comment that posts at
/// once. Tracked over the composer.
/// Whether the open item takes Work runs: a worktree of the reader's own,
/// or a pull request they opened, on disk.
fn work_offered(app: AppState, state: ReviewState) -> bool {
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    state.item.with(|item| {
        item.as_ref()
            .is_some_and(|item| crate::items::work_allowed(item, viewer.as_deref()))
    })
}

/// Whether the composer's Work is Start work: a new question to an agent
/// on an issue there is a branch to cut for. Tracked.
fn start_work_offered(state: ReviewState) -> bool {
    let issue = state.item.with(|item| {
        item.as_ref().is_some_and(|item| {
            matches!(item.subject, Some(perch_github::ItemSubject::Issue(_)))
                && crate::review::start_work::can_start_work(item)
        })
    });
    issue
        && state.composer.with(|c| {
            c.as_ref().is_some_and(|c| {
                !c.destination.is_github() && c.thread_id.is_none() && c.editing.is_none()
            })
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
/// the text is bound for an agent. On an issue, Work is Start work, for a
/// new question. Absent anywhere else, where every message is a question.
fn mode_switch(app: AppState, state: ReviewState) -> impl IntoView {
    use perch_github::RunMode;

    let shown = move || {
        (work_offered(app, state)
            && state
                .composer
                .with(|c| c.as_ref().is_some_and(|c| !c.destination.is_github())))
            || start_work_offered(state)
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
                    .color(if on { theme::fg() } else { theme::fg_muted() })
                    .apply_if(on, |s| {
                        s.background(theme::canvas())
                            .font_weight(theme::weight_emphasis())
                    })
                    .apply_if(!on, |s| s.hover(|s| s.color(theme::fg())))
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
                        .background(theme::canvas_subtle())
                        .border(1.0)
                        .border_color(theme::border())
                        .border_radius(8.0)
                })
                .into_any()
        } else {
            Empty::new().into_any()
        }
    })
}

/// What a composer with `editing` set is editing.
#[derive(Debug, Clone, PartialEq)]
enum EditKind {
    /// A draft of the reader's, still on its way to GitHub.
    Draft,
    /// What a reviewer said in a finding's thread, Perch's alone.
    Reviewer,
    /// A comment on GitHub, by its node id.
    Posted {
        node_id: String,
        place: perch_github::CommentPlace,
    },
}

impl EditKind {
    /// The footer's one button for an edit that has nowhere else to go; a
    /// draft's footer is a draft's as ever.
    fn save_label(self) -> Option<&'static str> {
        match self {
            EditKind::Draft => None,
            EditKind::Reviewer => Some("Save"),
            EditKind::Posted { .. } => Some("Update comment"),
        }
    }

    /// What the box says when emptied, for an edit of something on
    /// GitHub: what goes in it, not a question. An edit of the reader's
    /// draft or a reviewer's words says what the composer would.
    fn placeholder(&self) -> Option<&'static str> {
        match self {
            EditKind::Posted { .. } if self.edits_description() => Some("Add a description…"),
            EditKind::Posted {
                place: perch_github::CommentPlace::Review,
                ..
            } => Some("Leave a review…"),
            EditKind::Posted { .. } => Some("Leave a comment…"),
            EditKind::Draft | EditKind::Reviewer => None,
        }
    }

    /// Whether this edits a pull request's or an issue's description,
    /// which may be empty, as it may when opened. A review's summary may
    /// not: GitHub wants one on a review that asks for changes, and a
    /// review with none has no Edit here to give it words again.
    fn edits_description(&self) -> bool {
        use perch_github::CommentPlace;
        matches!(
            self,
            EditKind::Posted {
                place: CommentPlace::PullRequestBody | CommentPlace::IssueBody,
                ..
            }
        )
    }
}

/// What `composer` edits: a message in a thread by its id, or what a card
/// shows of GitHub's by its node id, a comment in the conversation, the
/// description or a review's summary. `None` when it edits nothing.
fn edit_kind(state: ReviewState, composer: &Composer) -> Option<EditKind> {
    let id = composer.editing.as_deref()?;
    let message = state.comment_threads.with_untracked(|threads| {
        threads
            .iter()
            .flat_map(|t| &t.messages)
            .find(|m| m.id == id)
            .cloned()
    });
    match message {
        Some(m) if matches!(m.author, Author::Agent { .. }) => Some(EditKind::Reviewer),
        Some(m) if m.from_github() => {
            let node_id = m.node_id.filter(|n| !n.is_empty())?;
            Some(EditKind::Posted {
                node_id,
                place: perch_github::CommentPlace::Line,
            })
        }
        Some(_) => Some(EditKind::Draft),
        None => state
            .github_comment_place(id)
            .map(|place| EditKind::Posted {
                node_id: id.to_string(),
                place,
            }),
    }
}

fn send_label(state: ReviewState) -> String {
    state.composer.with(|c| match c.as_ref() {
        Some(c) if c.destination.is_github() => {
            if state.posts_issue_comment(c) {
                "Comment".to_string()
            } else if state.posts_reply(c) {
                "Reply".to_string()
            } else {
                "Add to review".to_string()
            }
        }
        Some(c) if c.mode == perch_github::RunMode::Work && start_work_offered(state) => {
            "Start work".to_string()
        }
        _ => "Send".to_string(),
    })
}

/// The one high-contrast action in the composer, matching Codex's dark
/// primary controls while the model and effort remain quiet selectors.
/// `Send` to an agent; `Add to review` when the text is bound for GitHub;
/// `Reply` under GitHub's thread; `Comment`, then `Post`, for an issue
/// comment.
fn send_button(label: impl Fn() -> String + 'static, on_click: Rc<dyn Fn()>) -> impl IntoView {
    Container::new(
        Stack::horizontal((
            Label::derived(label).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::canvas())
                    .margin_right(6.0)
                    .selectable(false)
            }),
            sidebar_glyph(SidebarIcon::ChevronRight, theme::canvas).style(|s| s.size(13.0, 13.0)),
        ))
        .style(|s| s.items_center()),
    )
    .on_event_stop(el::Click, move |_, _| on_click())
    .style(|s| {
        clickable(s)
            .height(30.0)
            .items_center()
            .padding_horiz(12.0)
            .background(theme::fg())
            .border_radius(9.0)
            .hover(|s| s.background(theme::action_hover()))
    })
}

/// What the item is, for a placeholder: "pull request", "issue" or
/// "worktree".
pub fn subject_word(state: ReviewState) -> &'static str {
    if state.pull_request.with(Option::is_some) {
        "pull request"
    } else if state.issue.with(Option::is_some) {
        "issue"
    } else {
        "worktree"
    }
}

/// Where the text goes, from what config offers: Claude Code's models,
/// then Codex's under a line when there are any, then GitHub under
/// another. A model carries its backend, so picking one sets both, and
/// keeps the effort where the model takes it. GitHub can be picked where
/// a draft has somewhere to land, a composer on lines or under a thread
/// that is on GitHub, and where the text posts at once as an issue
/// comment: the foot of a pull request's or an issue's conversation, or
/// under a thread with neither lines nor a place on GitHub. A draft being
/// edited stays a draft: the models are greyed out, since sending would
/// replace its body, not ask.
fn destination_picker(app: AppState, state: ReviewState) -> impl IntoView {
    use crate::ui::menu::Menu;

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
                    item.checked(checked)
                        .enabled(models_offered)
                        .action(move || {
                            let agent = app.agent.get_untracked();
                            // The effort stays where it was, or where it was
                            // last for an agent when coming back from GitHub.
                            let effort = state
                                .composer
                                .with_untracked(|c| {
                                    c.as_ref().and_then(|c| c.destination.agent_choice())
                                })
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
        menu.separator()
            .item("GitHub", move |item| {
                item.checked(composer.destination.is_github())
                    .enabled(github_offered)
                    .action(move || state.set_destination(Destination::GitHub))
            })
            .return_focus(state.composer_box.get_untracked())
    })
    .debug_name("composer-destination")
}

/// The open composer's anchor and the lines under it, as a suggestion
/// replaces them: on its thread's lines where it is a follow-up, since
/// those are the ones GitHub's hunk has. Tracked.
fn composer_original(
    state: ReviewState,
) -> Option<(perch_github::Anchor, crate::review::suggestion::Original)> {
    let (thread_id, anchor) = state
        .composer
        .with(|c| c.as_ref().map(|c| (c.thread_id.clone(), c.anchor.clone())))?;
    let anchor = anchor?;
    let thread = thread_id.and_then(|id| {
        state
            .comment_threads
            .with(|t| t.iter().find(|t| t.id == id).cloned())
    });
    let original = match thread {
        Some(thread) => crate::review::suggestion::original_of(state, &thread),
        None => crate::review::suggestion::original(state, &anchor, false, None),
    }?;
    Some((anchor, original))
}

/// The picker's face: `Claude Opus`, `Codex Sol 6.1`, or `GitHub`. The
/// effort has a picker of its own beside it.
pub fn destination_label(destination: &Destination) -> String {
    match destination {
        Destination::Agent { backend, model, .. } => model_name(backend.name(), model),
        Destination::GitHub => "GitHub".to_string(),
    }
}

/// The picker's sections: one per backend that has models, Claude Code
/// first. Each entry names the backend the model runs on, which is
/// config's call, not the section's, so a model listed under both stays
/// with Claude Code wherever it appears.
pub(crate) fn model_groups(
    agent: &perch_github::AgentConfig,
) -> Vec<Vec<(perch_agent::Backend, String)>> {
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
    use crate::ui::menu::Menu;

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
        menu.return_focus(state.composer_box.get_untracked())
    })
    .debug_name("composer-effort")
}

/// A small pop-up button, like the toolbar's, sized for the composer.
fn menu_button(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> crate::ui::menu::Menu + 'static,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle)
            .style(|s| s.size(10.0, 10.0).pointer_events_none()),
    ))
    .dropdown(menu)
    .style(|s| {
        s.items_center()
            .height(26.0)
            .padding_horiz(8.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .selectable(false)
            .hover(|s| {
                s.background(theme::canvas_subtle())
                    .cursor(CursorStyle::Pointer)
            })
    })
}

/// The Outdated mark beside an anchor whose lines have moved.
pub fn outdated_pill(outdated: bool) -> impl IntoView {
    if !outdated {
        return Empty::new().into_any();
    }
    badge(
        || "Outdated".to_string(),
        theme::notice_fg,
        theme::notice_bg,
        Some(theme::notice_border),
    )
    .style(|s| s.margin_left(8.0))
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
    let resolved = {
        let id = thread.id.clone();
        Memo::new(move |_| state.is_resolved(&id))
    };
    let open_thread = thread.clone();
    let open_backend = backend.clone();
    let card = dyn_container(
        move || (collapsed.get(), resolved.get()),
        move |(collapsed, resolved)| {
            if collapsed && resolved {
                resolved_row(app, state, open_thread.clone()).into_any()
            } else if collapsed {
                collapsed_row(app, state, open_thread.clone()).into_any()
            } else {
                open_inline_card(app, state, open_backend.clone(), open_thread.clone()).into_any()
            }
        },
    )
    .style(move |s| {
        // Collapsed, it is exactly as tall as its rows less the inset it
        // sits inside them. The layer sets its width from the edges it is
        // placed against.
        let folded = collapsed.get();
        let base = s
            .apply_if(folded, |s| {
                s.height(
                    crate::review::doc::COLLAPSED_ROWS as f64 * theme::code_line_height() as f64
                        - 2.0 * theme::COLLAPSED_CARD_INSET,
                )
            })
            .border(1.0)
            .border_radius(theme::RADIUS);
        // White and framed is GitHub's; grey and framed a step quieter
        // is Perch's own, as on Conversation.
        let (background, border) = match origin {
            ThreadOrigin::Local => (theme::perch_bg(), theme::border_muted()),
            ThreadOrigin::GitHub => (theme::canvas(), theme::border()),
        };
        base.background(background).border_color(if flashing.get() {
            theme::accent()
        } else {
            border
        })
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
    let menu = thread_menu(state, &thread, theme::THREAD_MENU, || {}).into_any();
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
        crate::review::run_view::running_mark(state, thread.id.clone()),
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
            .border_color(theme::border())
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
        s.font_size(theme::TEXT_META)
            .color(theme::fg_muted())
            .selectable(false)
            .flex_shrink(0.0_f32)
    };
    let strong = |s: floem::style::Style| {
        s.font_size(theme::TEXT_META)
            .font_weight(theme::weight_strong())
            .color(theme::fg())
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
        badge(
            || "Perch only".to_string(),
            theme::fg_muted,
            theme::canvas,
            None,
        )
        .style(|s| s.margin_left(8.0)),
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
        ThreadFace::Inline => (theme::COLLAPSED_CARD_MENU, theme::TEXT_META),
    };
    Stack::horizontal((
        crate::ui::widgets::icon_in(
            move || {
                let icon = if folded() {
                    SidebarIcon::Unfold
                } else {
                    SidebarIcon::Fold
                };
                crate::icons::sidebar_icon(icon, theme::fg_muted())
            },
            theme::fg_muted,
        )
        .style(|s| s.size(14.0, 14.0).margin_right(4.0)),
        Label::derived(move || {
            if folded() {
                "Show resolved".to_string()
            } else {
                "Hide resolved".to_string()
            }
        })
        .style(move |s| s.font_size(size).color(theme::fg()).selectable(false)),
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
            .hover(|s| s.background(theme::badge_bg()))
            .apply_if(!resolved.get(), |s| s.hide())
    })
}

/// A resolved thread collapsed to its one row, as github.com folds one:
/// the chevron, the lines it is on, Outdated, Local on one of the
/// reader's own, and Show resolved and the `...` menu at the end. It is
/// the open card's header with the chevron pointing right, so nothing
/// moves when Show resolved opens it.
fn resolved_row(app: AppState, state: ReviewState, thread: CommentThread) -> impl IntoView {
    let card_id = CardId::Thread(thread.id.clone());
    let toggle_id = card_id.clone();
    let open_id = card_id.clone();
    Stack::horizontal((
        chevron(state, card_id, false),
        lines_heading(app, state, &thread),
        outdated_pill(thread.outdated),
        local_chip(!thread.is_github_bound()),
        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(12.0)),
        resolved_toggle(
            state,
            thread.id.clone(),
            || true,
            ThreadFace::Inline,
            move || state.toggle_card(&toggle_id),
        ),
        thread_menu(state, &thread, theme::COLLAPSED_CARD_MENU, move || {
            state.expand_card(&open_id)
        })
        .style(|s| s.margin_left(theme::COLLAPSED_CARD_PILL_GAP)),
    ))
    .debug_name("resolved-row")
    .style(|s| {
        s.items_center()
            .width_full()
            .height_full()
            .padding_horiz(theme::COLLAPSED_CARD_PADDING)
    })
}

/// A thread not resolved collapsed to its one row: who spoke first, the
/// first line of what they said, how many messages follow, Outdated, and
/// the `...` menu, smaller than the open card's so it fits the row.
fn collapsed_row(app: AppState, state: ReviewState, thread: CommentThread) -> impl IntoView {
    let first = thread.messages.first();
    let author = first.map(|m| m.author.clone()).unwrap_or(Author::User);
    let run = first.and_then(|m| m.run.clone());
    let line = first
        .map(|m| crate::review::timeline::first_line(&m.body))
        .unwrap_or_default();
    let card_id = CardId::Thread(thread.id.clone());
    let open_id = card_id.clone();
    // A thread on GitHub that holds the reader's asks says how many, in
    // Perch's grey, so folding it does not hide that they are there.
    let private = if thread.has_github_message() {
        thread.messages.iter().filter(|m| m.is_private()).count()
    } else {
        0
    };
    // Set off from the pill before it as the pills are from what is
    // before them: the pill's fill and the menu's hover fill are one
    // colour, and would touch.
    let trailing = Stack::horizontal((
        match private {
            0 => Empty::new().into_any(),
            n => badge(
                move || format!("{n} Perch only"),
                theme::fg_muted,
                theme::perch_bg,
                None,
            )
            .debug_name("perch-only-count")
            .style(|s| s.margin_left(theme::COLLAPSED_CARD_PILL_GAP))
            .into_any(),
        },
        crate::review::run_view::running_mark(state, thread.id.clone()),
        thread_menu(state, &thread, theme::COLLAPSED_CARD_MENU, move || {
            state.expand_card(&open_id)
        })
        .style(|s| {
            s.margin_left(theme::COLLAPSED_CARD_PILL_GAP)
                .flex_shrink(0.0_f32)
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
    .into_any();
    let avatar = author_avatar(app, state, &author, theme::COLLAPSED_CARD_AVATAR);
    collapsed_card_row(
        state,
        card_id,
        avatar,
        move || author_name(app, &author, run.as_ref()),
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
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
                .selectable(false)
                .margin_left(6.0)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || line.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
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
/// their login once GitHub has said it, the agent by its model and the
/// effort of the `run` that wrote it. A login is a link to its page; a
/// model and "You" lead nowhere.
fn author_name(app: AppState, author: &Author, run: Option<&RunRecord>) -> Name {
    match author {
        Author::User => app
            .viewer
            .get()
            .map(|(login, _)| login)
            .filter(|l| !l.is_empty())
            .map(Name::Login)
            .unwrap_or_else(|| Name::Plain("You".into())),
        Author::Agent { backend, model } => Name::Plain(match run {
            Some(run) => run_agent_name(run),
            None => model_name(backend, model),
        }),
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
        sidebar_glyph(SidebarIcon::Agents, theme::fg_muted)
            .style(move |s| s.size(size * 0.6, size * 0.6)),
    )
    .debug_name("agent-avatar")
    .style(move |s| {
        s.size(size, size)
            .border_radius(size / 2.0)
            .background(theme::badge_bg())
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
    badge(
        move || format!("{more} more"),
        theme::fg_subtle,
        theme::badge_bg,
        None,
    )
    .style(|s| s.margin_left(theme::COLLAPSED_CARD_PILL_GAP))
    .into_any()
}

/// The chevron at a card's left: pointing right while the card is
/// collapsed and down while it is open, as a run's steps have it, in a
/// slot the size of the gutter's controls. Click toggles the card.
fn chevron(state: ReviewState, card_id: CardId, open: bool) -> impl IntoView {
    chevron_button(
        move || open,
        theme::CARD_CHEVRON_SLOT,
        theme::CARD_CHEVRON_GLYPH,
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
            theme::accent()
        } else {
            theme::fg_subtle()
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
            .border_radius(if slot > theme::CHEVRON_SLOT {
                theme::RADIUS
            } else {
                3.0
            })
            .flex_shrink(0.0_f32)
            .hover(|s| s.background(theme::badge_bg()))
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
            confirmation(
                "delete-thread",
                title.to_string(),
                copy.to_string(),
                "Delete",
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
                move || state.confirm_thread.set(None),
            )
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
/// since nothing here may touch what is on GitHub. The confirmation
/// shows inside the card, so the menu calls `open` first: a card drawn
/// as one row opens to show it.
pub fn thread_menu(
    state: ReviewState,
    thread: &CommentThread,
    size: f64,
    open: impl Fn() + 'static,
) -> impl IntoView {
    use crate::ui::menu::Menu;

    let github_bound = thread.is_github_bound();
    if github_bound && !thread.messages.iter().any(|m| m.is_private()) {
        return Empty::new().into_any();
    }
    let thread_id = thread.id.clone();
    let open = Rc::new(open);
    Container::new(
        sidebar_glyph(SidebarIcon::Ellipsis, theme::fg_subtle).style(|s| s.pointer_events_none()),
    )
    .dropdown(move || {
        let id = thread_id.clone();
        let label = if github_bound {
            "Delete Perch's Messages\u{2026}"
        } else {
            "Delete Thread\u{2026}"
        };
        // Delete asks first, under the byline, wherever the thread is
        // showing; the confirmation is the one thing that deletes.
        let open = open.clone();
        Menu::new().item(label, move |item| {
            let id = id.clone();
            let open = open.clone();
            item.icon(SidebarIcon::Trash).action(move || {
                open();
                state.confirm_thread.set(Some(id.clone()));
            })
        })
    })
    .debug_name("thread-menu")
    .style(move |s| {
        clickable(s)
            .size(size, size)
            .items_center()
            .justify_center()
            .border_radius(size / 2.0)
            .flex_shrink(0.0_f32)
            .hover(|s| s.background(theme::badge_bg()))
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
        // A run's status and error, but not its steps or words: the run
        // block reads those live, and a row built again for each would
        // throw the whole message away and draw it again.
        |(m, editing, lane, place)| {
            (
                m.id.clone(),
                m.origin.name(),
                m.github_id,
                cards::body_hash(&m.body),
                m.run.as_ref().map(|r| (r.status.name(), r.error.clone())),
                *editing,
                *lane,
                *place,
            )
        },
        move |(m, editing, lane, place)| {
            let thread = state
                .comment_threads
                .with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
            match (thread, editing) {
                // The room goes outside the box's border, not in place of
                // its own padding. The Follow up box is gone while a draft
                // is edited, so under the last one is the card's edge.
                (Some(_), Some(_)) => match state.composer.get_untracked() {
                    Some(c) => Container::new(
                        composer_view(app, state, backend.clone(), c, face)
                            .style(|s| s.width_full()),
                    )
                    .style(move |s| {
                        s.width_full()
                            .padding_horiz(face.inset())
                            .padding_top(face.comment_space())
                            .padding_bottom(if place.last {
                                face.inset()
                            } else {
                                face.comment_space()
                            })
                    })
                    .into_any(),
                    None => Empty::new().into_any(),
                },
                // A message only Perch has, in a thread on GitHub, is on
                // a grey band from one side of the box to the other, so
                // what a colleague can see is the white.
                (Some(thread), None) => Container::new(message_view(
                    app,
                    state,
                    backend.clone(),
                    thread,
                    m,
                    face,
                    lane,
                ))
                .debug_name(if lane == Lane::Flush {
                    "message-row"
                } else {
                    "private-band"
                })
                .style(move |s| {
                    s.width_full()
                        .padding_horiz(face.inset())
                        .apply_if(lane != Lane::Flush, |s| s.background(theme::perch_bg()))
                        // Each row paints its own grey, and two meeting
                        // on a fractional y leave a hairline of their
                        // blended edges. One under another reaches a
                        // pixel up over the first, which its grey covers.
                        .apply_if(lane == Lane::PrivateContinue, |s| {
                            s.margin_top(-1.0).padding_top(1.0)
                        })
                })
                .into_any(),
                (None, _) => Empty::new().into_any(),
            }
        },
    )
    .debug_name("thread-messages")
    .style(|s| s.flex_col().width_full())
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
        // A finding's band runs from one side of the box to the other,
        // so it sets its own room in from them.
        Container::new(cards::finding_controls(
            app,
            state,
            backend.clone(),
            thread.id.clone(),
            inset,
        ))
        .style(move |s| s.width_full().apply_if(!finding.get(), |s| s.hide())),
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
    // Keyed on whether a run is going, and whether it is stopping, as well
    // as on the composer: while the agent works the box steers it, when
    // it is done the box follows up, and while it stops there is none,
    // which only the thread list says.
    let running = Memo::new(move |_| {
        state.comment_threads.with(|t| {
            t.iter()
                .find(|t| t.id == running_thread)
                .and_then(|t| t.running_run())
                .map(|run| run.status == RunStatus::Stopping)
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
    // Nothing in the box's place: an agent stopping with no composer
    // open, or a draft being edited up among the comments.
    let empty = Memo::new(move |_| match composer.get() {
        Some((_, _, Some(_))) => true,
        Some(_) => false,
        None => running.get() == Some(true),
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
            // On the box's own colour, as its header is: a grey strip
            // would read as Perch's on a GitHub thread.
            s.items_center()
                .width_full()
                .padding_vert(8.0)
                .padding_horiz(theme::THREAD_INSET)
                .border_top(1.0)
                .border_color(theme::border_muted())
                .apply_if(empty.get(), |s| s.hide())
        })
        .into_any(),
        // A white message's room under its text is room over the box; a
        // grey band's is inside the grey, so under one the box takes the
        // inset it has at its sides and foot, or it meets the band's edge.
        ThreadFace::Inline => {
            let band_thread = thread_id.clone();
            let after_band = Memo::new(move |_| {
                state.comment_threads.with(|t| {
                    t.iter()
                        .find(|t| t.id == band_thread)
                        .is_some_and(|t| Lane::of(t).last().is_some_and(|l| *l != Lane::Flush))
                })
            });
            Container::new(reply)
                .debug_name("reply-bar")
                .style(move |s| {
                    s.width_full()
                        .padding_horiz(theme::INLINE_THREAD_INSET)
                        .padding_bottom(theme::INLINE_THREAD_INSET)
                        .apply_if(after_band.get(), |s| {
                            s.padding_top(theme::INLINE_THREAD_INSET)
                        })
                        .apply_if(empty.get(), |s| s.hide())
                })
                .into_any()
        }
    };
    Stack::vertical((
        bar,
        resolve_foot(state, backend, thread_id, face, offered, foot_hidden),
    ))
    .style(|s| s.flex_col().width_full())
}

/// Whether a thread's foot offers Resolve: on one of GitHub's where
/// GitHub lets the reader, a published finding's too, on any other
/// always, and never on a finding's still in Perch, which has Dismiss and
/// Reopen instead.
fn resolve_offered(app: AppState, state: ReviewState, thread_id: String) -> Memo<bool> {
    Memo::new(move |_| {
        let (resolvable, on_github) = state.comment_threads.with(|threads| {
            threads
                .iter()
                .find(|t| t.id == thread_id)
                .map_or((false, false), |t| (t.resolvable(), t.on_github()))
        });
        resolvable && (on_github || cards::finding_of(app, state, &thread_id).is_none())
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
                let Some(item_id) = state
                    .item
                    .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
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
                    s.color(theme::fg_subtle()).cursor(CursorStyle::Default)
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
                .border_color(theme::border())
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
                .color(theme::fg())
                .background(theme::canvas_subtle())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .hover(|s| s.background(theme::canvas_hover()))
        })
}

/// One message, laid out as github.com lays out a comment: a face, a
/// byline with the name, when it was said and its labels, and what was
/// said under it. A comment on GitHub, the reader's or anyone's, has
/// Reply, Ask about this, the reaction picker and its link at the end of
/// its byline under the pointer, and the reactions on it under its text. A draft has the
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
        return posted_message_view(app, state, backend, &thread, &message, face, place);
    }
    local_message_view(
        app,
        state,
        backend,
        thread,
        message,
        face,
        place,
        lane != Lane::Flush,
    )
}

/// The height of a comment's byline: the buttons at its end, which the
/// avatar beside it is centred on.
const BYLINE_ROW: f64 = 24.0;

/// The room between a comment's byline and its text.
const COMMENT_BODY_GAP: f64 = 6.0;

/// A message laid out as github.com lays out a comment: the avatar at the
/// left of its byline, and its text under the byline, indented past the
/// avatar, with a line down the avatars from each message to the next.
/// The first is laid out as the rest are, in the code too: set flush, as
/// Files changed sets the comment a thread opens with, it sat apart from
/// the replies and from a grey band under it.
pub(crate) fn comment_frame(
    face: ThreadFace,
    place: Place,
    avatar: floem::AnyView,
    byline: floem::AnyView,
    body: floem::AnyView,
) -> floem::AnyView {
    let space = face.comment_space();
    let top = if place.first { face.inset() } else { space };
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
            .apply_if(shown, |s| s.background(theme::border_muted()))
    })
}

/// A comment's byline: the `name`, then what comes `after` it, when it
/// was said and its labels, and at the end what can be done with it.
pub(crate) fn byline_row(
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
        .style(|s| {
            s.items_center()
                .width_full()
                .min_width(0.0)
                .height(BYLINE_ROW)
        })
        .into_any()
}

/// A name as a comment's byline sets it: GitHub's semibold, a step down
/// in the code.
pub(crate) fn name_style(s: floem::style::Style, face: ThreadFace) -> floem::style::Style {
    let size = match face {
        ThreadFace::Card => theme::TEXT_BODY,
        ThreadFace::Inline => theme::TEXT_META,
    };
    s.font_size(size)
        .font_weight(theme::weight_strong())
        .color(theme::fg())
        .flex_shrink(0.0_f32)
}

/// When a message was said, "3 hours ago", kept current by the clock.
fn when_label(
    app: AppState,
    at: jiff::Timestamp,
    face: ThreadFace,
    name: &'static str,
) -> impl IntoView {
    // `now` ticks each second while an agent runs; a memo only tells the
    // label when the words change.
    let ago = Memo::new(move |_| crate::timefmt::relative(at, app.now.get()));
    Label::derived(move || ago.get())
        .debug_name(name)
        .style(move |s| {
            let size = match face {
                ThreadFace::Card => theme::TEXT_BODY,
                ThreadFace::Inline => theme::TEXT_BADGE,
            };
            s.font_size(size)
                .color(theme::fg_muted())
                .margin_left(6.0)
                .text_ellipsis()
                .min_width(0.0)
        })
}

/// A label in a comment's byline, as GitHub sets Author and its kin: the
/// word in a pill with a rule round it.
fn byline_label(text: &'static str, name: &'static str) -> Container {
    badge(
        move || text.to_string(),
        theme::fg_muted,
        || floem::peniko::Color::TRANSPARENT,
        Some(theme::border),
    )
    .debug_name(name)
    .style(|s| s.margin_left(6.0))
}

/// Author, beside a comment by whoever opened the pull request or the
/// issue, as GitHub marks them. `login` is read as it is drawn.
fn author_label(state: ReviewState, login: impl Fn() -> Option<String> + 'static) -> impl IntoView {
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
pub(crate) fn perch_only_label() -> impl IntoView {
    cards::tip(
        byline_label("Perch only", "perch-only"),
        "Only Perch shows this; nothing here is on GitHub.".to_string(),
    )
}

/// After Review #N, in the byline of a message said after the past review
/// the item shows had ended. A mark rather than words in the text, so an
/// edit never carries it into the message.
fn after_review_label(state: ReviewState, at: jiff::Timestamp) -> impl IntoView {
    let number = Memo::new(move |_| {
        state.historical_review.with(|review| {
            let group = &review.as_ref()?.group;
            (at > group.ended_at?).then_some(group.number)
        })
    });
    cards::tip(
        badge(
            move || {
                number
                    .get()
                    .map(|n| format!("After Review #{n}"))
                    .unwrap_or_default()
            },
            theme::fg_muted,
            || floem::peniko::Color::TRANSPARENT,
            Some(theme::border),
        )
        .debug_name("after-review")
        .style(move |s| {
            s.margin_left(6.0)
                .apply_if(number.get().is_none(), |s| s.hide())
        }),
        "Said after this review ended; the code shown is what the review read.".to_string(),
    )
}

/// The reader's question to an agent, or an agent's answer, under the
/// reader's face and name or the agent's mark and its model, `private`
/// marking it Perch only. A finished run sits on the answer, and reads in
/// the order it went: what the agent did, then what it said, then what
/// it cost. While it runs, or when it failed, the run sits under the
/// question, in a comment of the agent's own. An answer that is done, or a finding, which has no run
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
    let settled = is_agent && !body.trim().is_empty() && (finished || message.run.is_none());
    let text = message_body(state, &thread, &body, None);
    let actions = if settled {
        // Only what a reviewer said is the reader's to edit; an answer
        // stays as its run left it.
        let editable = message.run.is_none().then(|| message.id.clone());
        agent_actions(
            app,
            state,
            thread.clone(),
            body.clone(),
            editable,
            text.id(),
            hovered,
        )
    } else {
        Empty::new().into_any()
    };
    // A run on the reader's question is the answer on its way: it takes a
    // comment of its own under the question, as the answer will.
    let (content, pending) = match message.run.clone() {
        Some(run) if is_agent && finished => (
            Stack::vertical((
                crate::review::run_view::run_fold(
                    state,
                    &run,
                    without_answer(run.clone(), &body).steps,
                ),
                text,
                run_footer(backend.clone(), run),
            ))
            .style(|s| s.flex_col().width_full().gap(8.0))
            .into_any(),
            None,
        ),
        Some(run) if is_agent => (
            Stack::vertical((text, run_block(app, state, backend.clone(), run)))
                .style(|s| s.flex_col().width_full().gap(8.0))
                .into_any(),
            None,
        ),
        Some(run) => (text, Some(run)),
        None => (text, None),
    };
    let author = message.author.clone();
    let author_run = message.run.clone();
    let mut after = vec![when_label(app, message.created_at, face, "comment-when").into_any()];
    if private {
        after.push(perch_only_label().into_any());
    }
    after.push(after_review_label(state, message.created_at).into_any());
    let avatar = author_avatar(app, state, &author, face.avatar());
    let byline = byline_row(
        name_label(
            move || author_name(app, &author, author_run.as_ref()),
            move |s| name_style(s, face),
        ),
        after,
        actions,
    );
    let Some(run) = pending else {
        return cards::track_hover(comment_frame(face, place, avatar, byline, content), hovered)
            .into_any();
    };
    let question = Place {
        first: place.first,
        last: false,
    };
    let answer = Place {
        first: false,
        last: place.last,
    };
    Stack::vertical((
        comment_frame(face, question, avatar, byline, content),
        pending_answer_view(app, state, backend, run, face, answer, private),
    ))
    .style(|s| s.flex_col().width_full())
    .into_any()
}

/// The agent's comment while its answer is on the way, or when it never
/// came: its mark and model over the run, as the answer will have them.
fn pending_answer_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    run: RunRecord,
    face: ThreadFace,
    place: Place,
    private: bool,
) -> floem::AnyView {
    let at = run.started_at.unwrap_or(run.queued_at);
    // The model the CLI reports can land after the run starts.
    let live = live_run(state, run.clone());
    let model = move || live.with(|run| Name::Plain(run_agent_name(run)));
    let mut after = vec![when_label(app, at, face, "comment-when").into_any()];
    if private {
        after.push(perch_only_label().into_any());
    }
    let byline = byline_row(
        name_label(model, move |s| name_style(s, face)),
        after,
        Empty::new().into_any(),
    );
    comment_frame(
        face,
        place,
        agent_avatar(face.avatar()).into_any(),
        byline,
        run_block(app, state, backend, run).into_any(),
    )
}

/// Quote reply, Post to GitHub, while the thread has somewhere on GitHub
/// to go, and Copy, at the end of an agent's byline, under the pointer
/// only, as a comment's are. Quote reply opens the Follow up box with
/// what is selected in `scope`, the answer's text, or else all of it,
/// quoted, bound for the agent. Post opens the composer under the
/// thread's last message, bound for GitHub, with the answer as the text:
/// a follow-up the reader edits and sends as their own, a draft where the
/// thread takes one and an issue comment where it has no reply to make.
/// Nothing remembers where it came from. A reviewer's message, `editable`,
/// also has Edit, which opens the composer in its place, for the reader
/// to trim a finding before posting it.
fn agent_actions(
    app: AppState,
    state: ReviewState,
    thread: CommentThread,
    body: String,
    editable: Option<String>,
    scope: floem::ViewId,
    hovered: RwSignal<bool>,
) -> floem::AnyView {
    let edit = editable.map(|id| {
        let (thread_id, anchor, body) = (thread.id.clone(), thread.anchor.clone(), body.clone());
        let edit: Rc<dyn Fn()> = Rc::new(move || {
            state.open_composer_with(
                Some(thread_id.clone()),
                anchor.clone(),
                body.clone(),
                Destination::GitHub,
                Some(id.clone()),
            );
        });
        cards::hover_action(hovered, SidebarIcon::Pencil, "Edit", "edit-finding", edit).into_any()
    });
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    let whole = body.clone();
    let quote: Rc<dyn Fn(Option<String>)> = Rc::new(move |selected| {
        let text = selected.as_deref().unwrap_or(&whole);
        quote_to_agent(app, state, &thread_id, anchor.clone(), text);
    });
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
    // Absent, not greyed, where the message has nowhere to go: a branch,
    // an issue thread with lines, or a closed pull request under a thread
    // whose draft would need a review. The pull request lands after the
    // threads, so this follows it rather than the message's own rebuild.
    let offered = Memo::new(move |_| state.github_offered_on(&thread));
    Stack::horizontal((
        cards::quote_action(hovered, scope, "quote-reply-agent", quote),
        dyn_container(
            move || offered.get(),
            move |offered| {
                if !offered {
                    return Empty::new().into_any();
                }
                cards::hover_action(
                    hovered,
                    SidebarIcon::PullRequest,
                    "Post to GitHub",
                    "post-to-github",
                    post.clone(),
                )
                .into_any()
            },
        ),
        cards::hover_action(hovered, SidebarIcon::Copy, "Copy", "copy-message", copy),
        edit.unwrap_or_else(|| Empty::new().into_any()),
    ))
    .debug_name("agent-actions")
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any()
}

/// Open the Follow up box under `thread_id` with `text` quoted, under
/// whatever the reader had typed there, bound for an agent: the one the
/// box was already bound for, or the last one asked. Never for GitHub;
/// that is what Post to GitHub is for.
fn quote_to_agent(
    app: AppState,
    state: ReviewState,
    thread_id: &str,
    anchor: Option<perch_github::Anchor>,
    text: &str,
) {
    let quote = crate::ask::quote_reply(text);
    let open = state.composer.with_untracked(|c| {
        c.as_ref()
            .filter(|c| c.thread_id.as_deref() == Some(thread_id) && c.editing.is_none())
            .map(|c| c.destination.clone())
    });
    let typed = open
        .is_some()
        .then(|| state.composer_draft.get_untracked())
        .filter(|d| !d.trim().is_empty());
    let draft = match typed {
        Some(typed) => format!("{}\n\n{quote}", typed.trim_end()),
        None => quote,
    };
    let destination = open.filter(|d| !d.is_github()).unwrap_or_else(|| {
        Destination::agent(
            state
                .agent_last
                .get_untracked()
                .unwrap_or_else(|| defaults(app)),
        )
    });
    state.open_composer_with(
        Some(thread_id.to_string()),
        anchor,
        draft,
        destination,
        None,
    );
}

/// Reply and Ask about this on a comment in a thread that is on GitHub,
/// the reader's or anyone's. Reply opens the composer under the thread,
/// bound for GitHub, whatever the thread's last message was, while it has
/// somewhere on GitHub to go; Ask about this opens it to an agent, so the
/// answer lands beside the comment it is about; the thread's history
/// carries the comment to the agent, and nothing is quoted.
fn reply_and_ask(
    app: AppState,
    state: ReviewState,
    thread: &CommentThread,
    hovered: RwSignal<bool>,
) -> (floem::AnyView, Rc<dyn Fn()>) {
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    // To an agent, whatever the thread's last message was: the point is
    // to ask about the comment, not to answer it.
    let ask: Rc<dyn Fn()> = Rc::new(move || {
        state.open_composer_with(
            Some(thread_id.clone()),
            anchor.clone(),
            String::new(),
            Destination::agent(
                state
                    .agent_last
                    .get_untracked()
                    .unwrap_or_else(|| defaults(app)),
            ),
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
            cards::hover_action(
                hovered,
                SidebarIcon::Reply,
                "Reply",
                "reply-github",
                reply_to.clone(),
            )
            .into_any()
        },
    )
    .into_any();
    (reply_button, ask)
}

/// A comment from GitHub in a thread, under its author's face and
/// byline, with the reactions on it under its text, and Reply, Ask about
/// this, the reaction picker and its link on hover.
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
    let (reply_button, ask) = reply_and_ask(app, state, thread, hovered);
    let node_id = message.node_id.clone().unwrap_or_default();
    let may = node_id.clone();
    let delete = cards::delete_action(state, hovered, node_id.clone(), move || {
        !may.is_empty() && state.may_delete_line_comment(&may)
    });
    let may = node_id.clone();
    let edit = cards::edit_action(
        hovered,
        move || !may.is_empty() && state.may_update_line_comment(&may),
        edit_in_row(state, thread, message),
    );
    let leading = cards::leading_actions(Some(reply_button), edit, Some(delete));
    let confirm = cards::delete_comment_confirmation(app, state, backend.clone(), node_id);
    let react = message.node_id.as_deref().and_then(cards::reactable);
    let url = message.html_url.clone().unwrap_or_default();
    let reactions = cards::reactions_under(app, state, backend.clone(), &react);
    let name = login.clone();
    let author = login.clone();
    let byline = byline_row(
        name_label(
            move || Name::Login(name.clone()),
            move |s| name_style(s, face),
        ),
        vec![
            when_label(app, message.created_at, face, "comment-when").into_any(),
            author_label(state, move || Some(author.clone())).into_any(),
            after_review_label(state, message.created_at).into_any(),
        ],
        cards::actions(
            state,
            backend.clone(),
            hovered,
            Some(leading),
            Some(ask),
            react,
            url,
        ),
    );
    let avatar = crate::ui::widgets::avatar(
        app,
        cards::avatar_url_of(state, login.clone()),
        face.avatar(),
        login,
    )
    .into_any();
    let body = Stack::vertical((
        message_body(state, thread, &message.body, Some(backend)),
        reactions,
        confirm,
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
    let edit = edit_in_row(state, thread, message);
    let delete_id = message.id.clone();
    let delete: Rc<dyn Fn()> = Rc::new(move || {
        let Some(item_id) = state
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        else {
            return;
        };
        backend.delete_draft(&item_id, &delete_id);
    });
    let actions = Stack::horizontal((
        cards::hover_action(hovered, SidebarIcon::Pencil, "Edit", "edit-draft", edit),
        cards::hover_action(
            hovered,
            SidebarIcon::Trash,
            "Delete",
            "delete-draft",
            delete,
        ),
    ))
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any();
    let byline = byline_row(
        name_label(
            move || author_name(app, &Author::User, None),
            move |s| name_style(s, face),
        ),
        vec![
            draft_pill().into_any(),
            author_label(state, move || viewer_login(app)).into_any(),
            after_review_label(state, message.created_at).into_any(),
        ],
        actions,
    );
    let body = message_body(state, thread, &message.body, None);
    let avatar = viewer_avatar(app, face.avatar()).into_any();
    cards::track_hover(comment_frame(face, place, avatar, byline, body), hovered).into_any()
}

/// A comment of the reader's that is on GitHub, under their face and
/// name with the comment's time, Reply, Ask about this, the reaction
/// picker and its link on hover, as anyone's comment has, and the
/// reactions on it under its text, Edit, which opens the composer in its
/// row, and Delete, which asks first. No Draft mark.
fn posted_message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    message: &CommentMessage,
    face: ThreadFace,
    place: Place,
) -> floem::AnyView {
    let hovered = RwSignal::new(false);
    let (reply_button, ask) = reply_and_ask(app, state, thread, hovered);
    // The reader wrote it, so GitHub lets them edit and delete it.
    let node_id = message.node_id.clone().unwrap_or_default();
    let deletable = !node_id.is_empty();
    let delete = cards::delete_action(state, hovered, node_id.clone(), move || deletable);
    let edit = cards::edit_action(
        hovered,
        move || deletable,
        edit_in_row(state, thread, message),
    );
    let leading = cards::leading_actions(Some(reply_button), edit, Some(delete));
    let confirm = cards::delete_comment_confirmation(app, state, backend.clone(), node_id);
    let react = message.node_id.as_deref().and_then(cards::reactable);
    let url = message.html_url.clone().unwrap_or_default();
    let reactions = cards::reactions_under(app, state, backend.clone(), &react);
    let byline = byline_row(
        name_label(
            move || author_name(app, &Author::User, None),
            move |s| name_style(s, face),
        ),
        vec![
            when_label(app, message.created_at, face, "posted-when").into_any(),
            author_label(state, move || viewer_login(app)).into_any(),
            after_review_label(state, message.created_at).into_any(),
        ],
        cards::actions(
            state,
            backend.clone(),
            hovered,
            Some(leading),
            Some(ask),
            react,
            url,
        ),
    );
    let body = Stack::vertical((
        message_body(state, thread, &message.body, Some(backend)),
        reactions,
        confirm,
    ))
    .style(|s| s.flex_col().width_full())
    .into_any();
    let avatar = viewer_avatar(app, face.avatar()).into_any();
    cards::track_hover(comment_frame(face, place, avatar, byline, body), hovered).into_any()
}

/// Open the composer in `message`'s row with its text, to edit it: a
/// draft, a reviewer's words, or a comment on GitHub.
fn edit_in_row(
    state: ReviewState,
    thread: &CommentThread,
    message: &CommentMessage,
) -> Rc<dyn Fn()> {
    let (thread_id, anchor) = (thread.id.clone(), thread.anchor.clone());
    let (id, body) = (message.id.clone(), message.body.clone());
    Rc::new(move || {
        state.open_composer_with(
            Some(thread_id.clone()),
            anchor.clone(),
            body.clone(),
            Destination::GitHub,
            Some(id.clone()),
        );
    })
}

/// A message's text, a suggestion in it drawn as the change it makes to
/// the thread's lines, which follow the file as it loads. A message on
/// GitHub, given `applying` with its backend, has Apply suggestion under
/// each suggestion.
fn message_body(
    state: ReviewState,
    thread: &CommentThread,
    body: &str,
    applying: Option<Rc<Backend>>,
) -> floem::AnyView {
    if thread.anchor.is_none() || !crate::review::suggestion::has_suggestion(body) {
        return crate::review::markdown::render_linked(body, state).into_any();
    }
    let (thread, body) = (thread.clone(), body.to_string());
    let original_thread = thread.clone();
    let original =
        Memo::new(move |_| crate::review::suggestion::original_of(state, &original_thread));
    dyn_container(
        move || original.get(),
        move |original| {
            let foot = applying.clone().and_then(|backend| {
                crate::review::apply_suggestion::foot(state, backend, &thread, original.clone())
            });
            crate::review::markdown::render_suggesting(&body, state, original, foot).into_any()
        },
    )
    .style(|s| s.width_full())
    .into_any()
}

/// The Draft mark after a draft's author, in GitHub's colour: this is
/// going there, and is not there yet.
pub fn draft_pill() -> impl IntoView {
    cards::tip(
        badge(
            || "Draft".to_string(),
            theme::github,
            theme::github_bg,
            None,
        )
        .debug_name("draft-pill")
        .style(|s| s.margin_left(8.0)),
        "Not on GitHub until the review is submitted.".to_string(),
    )
}

/// A run under the question while it works, or when it stopped or
/// failed. While it goes: the line saying what it is doing, with Stop,
/// then its words and calls as they come. Once it is over: the fold, and
/// the error with Retry. Either way what it asks the reader, the words of
/// a run from before notes, and what a Work run changed.
pub fn run_block(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    run: RunRecord,
) -> impl IntoView {
    use crate::review::run_view::{run_fold, run_header, timeline};

    // Its steps and words as they arrive, read from the thread list: the
    // row holding the block is kept through the run, and a step's result
    // comes after the step, with nothing else to build the block again.
    let live = live_run(state, run.clone());
    let active = Memo::new(move |_| live.with(|run| run.status.is_active()));
    // A run from before notes has only the text.
    let partial = Memo::new(move |_| {
        live.with(|run| {
            run.final_result.clone().filter(|text| {
                run.status != RunStatus::Completed
                    && !text.trim().is_empty()
                    && !run.steps.iter().any(|step| step.note)
            })
        })
    });
    let response = dyn_container(
        move || partial.get(),
        move |partial| match partial {
            Some(text) => Container::new(crate::review::markdown::render_linked(&text, state))
                .style(|s| s.width_full().margin_top(8.0))
                .into_any(),
            None => Empty::new().into_any(),
        },
    )
    .style(|s| s.width_full());
    let changes = work_changes(state, backend.clone(), &run);
    let asks = approval_cards(state, backend.clone(), run.id.clone());
    let body = dyn_container(
        move || active.get(),
        move |active| {
            if active {
                let steps = Memo::new(move |_| live.with(|run| run.steps.clone()));
                Stack::vertical((
                    run_header(app, backend.clone(), live),
                    timeline(state, move || steps.get(), || true),
                ))
                .style(|s| s.flex_col().width_full().gap(4.0))
                .into_any()
            } else {
                let run = live.get_untracked();
                Stack::vertical((
                    run_fold(state, &run, run.steps.clone()),
                    run_footer(backend.clone(), run),
                ))
                .style(|s| s.flex_col().width_full().gap(6.0))
                .into_any()
            }
        },
    )
    .style(|s| s.width_full());
    // No gap: each part below the body sets itself off while it shows,
    // since taffy gaps an empty child too.
    Stack::vertical((body, asks, response, changes))
        .debug_name("run-block")
        .style(|s| s.flex_col().width_full())
}

/// A run as the thread list has it now, or as it was handed over when
/// the list no longer has it.
fn live_run(state: ReviewState, initial: RunRecord) -> Memo<RunRecord> {
    Memo::new(move |_| {
        state
            .comment_threads
            .with(|threads| {
                threads
                    .iter()
                    .flat_map(|t| &t.messages)
                    .find_map(|m| m.run.as_ref().filter(|r| r.id == initial.id).cloned())
            })
            .unwrap_or_else(|| initial.clone())
    })
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
    // Set off from what is above only while there is a card: the run's
    // column has no gap, which taffy would leave around an empty stack.
    .style(move |s| {
        s.flex_col()
            .gap(8.0)
            .width_full()
            .apply_if(!pending.with(Vec::is_empty), |s| s.margin_top(8.0))
    })
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
            sidebar_glyph(SidebarIcon::Warning, theme::draft_yellow)
                .style(|s| s.size(14.0, 14.0).margin_right(6.0).flex_shrink(0.0_f32)),
            Label::derived(move || title.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
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
                        .color(theme::fg())
                        .text_wrap()
                        .width_full()
                        .padding(6.0)
                        .background(theme::canvas())
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
                        .color(theme::fg_muted())
                        .text_wrap()
                        .width_full()
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        Stack::horizontal((
            action_button(
                "Allow once",
                "approval-once",
                true,
                || true,
                move || once(ApprovalAnswer::Once),
            ),
            action_button(
                &view.always,
                "approval-always",
                false,
                || true,
                move || always(ApprovalAnswer::Always),
            ),
            action_button(
                "Deny",
                "approval-deny",
                false,
                || true,
                move || deny(ApprovalAnswer::Deny),
            ),
        ))
        .style(|s| {
            s.items_center()
                .gap(6.0)
                .flex_wrap(floem::style::FlexWrap::Wrap)
        }),
    ))
    .debug_name("approval-card")
    .style(|s| {
        s.flex_col()
            .gap(6.0)
            .width_full()
            .padding(10.0)
            .background(theme::notice_bg())
            .border(1.0)
            .border_color(theme::notice_border())
            .border_radius(theme::RADIUS)
    })
}

/// What a finished Work run changed, `4 files changed, +120 −30`: a
/// click shows it on Code as the Run scope. Nothing for a question, for
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
    let counts =
        floem::reactive::Memo::new(move |_| state.run_changes.with(|m| m.get(&id).copied()));
    dyn_container(
        move || counts.get(),
        move |counts| {
            let Some((files, added, removed)) = counts.filter(|(files, _, _)| *files > 0) else {
                return Empty::new().into_any();
            };
            let (before, after) = (before.clone(), after.clone());
            let backend = backend.clone();
            let files = count_of(files as usize, "file", "files");
            Label::derived(move || format!("{files} changed, +{added} \u{2212}{removed}"))
                .on_event_stop(el::Click, move |_, _| {
                    let Some(item_id) = state
                        .item
                        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
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
                    state.show_tab(crate::review::ItemTab::Code);
                })
                .debug_name("run-changes")
                .style(|s| {
                    clickable(s)
                        .margin_top(8.0)
                        .font_size(theme::TEXT_BADGE)
                        .color(theme::accent())
                        .hover(|s| s.color(theme::fg()))
                })
                .into_any()
        },
    )
    .into_any()
}

/// The footer under a run: cost and time once it is done, the error and
/// Retry when it is not. A run still going has its Stop on its header.
fn run_footer(backend: Rc<Backend>, run: RunRecord) -> floem::AnyView {
    match run.status {
        RunStatus::Queued | RunStatus::Starting | RunStatus::Running | RunStatus::Stopping => {
            Empty::new().into_any()
        }
        RunStatus::Completed => {
            // The answer's byline names the model and how hard it thought,
            // and the steps line how long it took; the run says whether it
            // worked in the worktree and what it cost.
            let footer = [
                matches!(run.mode, perch_github::RunMode::Work).then(|| "Work".to_string()),
                run.cost_usd.map(|c| format!("${c:.2}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" \u{00b7} ");
            if footer.is_empty() {
                return Empty::new().into_any();
            }
            Label::derived(move || footer.clone())
                .style(|s| {
                    s.font_size(theme::TEXT_SECTION)
                        .color(theme::fg_subtle())
                        .selectable(false)
                })
                .into_any()
        }
        RunStatus::Stopped | RunStatus::Failed | RunStatus::Interrupted => {
            let error = run.error.clone().unwrap_or_else(|| "Failed".to_string());
            let retry_id = run.id.clone();
            Stack::horizontal((
                Label::derived(move || error.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::closed())
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

/// A finished run's steps less the notes its answer already says: the
/// closing words, and every word of a run from before notes, whose answer
/// held all it said.
fn without_answer(mut run: RunRecord, body: &str) -> RunRecord {
    run.steps
        .retain(|step| !(step.note && body.contains(step.summary.as_str())));
    run
}

/// The folded steps line: "Working…" while the run goes, then how long
/// it worked, or what it did when that is unknown.
pub(crate) fn run_summary(run: &RunRecord) -> String {
    if run.status.is_active() {
        return "Working…".to_string();
    }
    let elapsed = run.duration_ms.or_else(|| {
        let (start, end) = (run.started_at?, run.ended_at?);
        u64::try_from(end.duration_since(start).as_millis()).ok()
    });
    match elapsed {
        Some(ms) => format!("Worked for {}", worked_for(ms)),
        None => steps_summary(&run.steps),
    }
}

/// "12s", "4m 12s", "1h 3m".
pub(crate) fn worked_for(ms: u64) -> String {
    let secs = (ms + 500) / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

/// "Read 3 files, ran 1 command, edited 1 file", from the step lines.
pub fn steps_summary(steps: &[perch_github::RunStep]) -> String {
    let steps: Vec<_> = steps.iter().filter(|s| !s.note && !s.user).collect();
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
    let edits = steps
        .iter()
        .filter(|s| {
            s.ok && (s.summary.starts_with("Asked to edit ") || s.summary.starts_with("Edited "))
        })
        .count();
    let denied = steps.iter().filter(|s| !s.ok).count();
    if reads > 0 {
        parts.push(format!("read {}", count_of(reads, "file", "files")));
    }
    if runs > 0 {
        parts.push(format!("ran {}", count_of(runs, "command", "commands")));
    }
    if searches > 0 {
        parts.push(count_of(searches, "search", "searches"));
    }
    if edits > 0 {
        parts.push(format!("edited {}", count_of(edits, "file", "files")));
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
/// click opens the composer in its place. While the thread's agent works
/// it is Steer, and while the agent stops there is none.
pub fn reply_line(
    app: AppState,
    state: ReviewState,
    thread: CommentThread,
    face: ThreadFace,
) -> impl IntoView {
    let steering = match thread.running_run() {
        Some(run) if run.status == RunStatus::Stopping => return Empty::new().into_any(),
        Some(_) => true,
        None => false,
    };
    let id = thread.id.clone();
    let anchor = thread.anchor.clone();
    Label::derived(move || if steering { "Steer…" } else { "Follow up…" }.to_string())
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
                .color(theme::fg_subtle())
                .width_full()
                .height(height)
                .padding_horiz(12.0)
                .items_center()
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .selectable(false)
                .hover(|s| s.border_color(theme::fg_subtle()).cursor(CursorStyle::Text))
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
        let view = run_block(AppState::new(), state, Rc::new(backend), run);
        let mut harness = HeadlessHarness::new_with_size(root, view, 700.0, 400.0);
        harness.rebuild();
        harness.paint();
        let find = |harness: &HeadlessHarness, name: &str| {
            fn walk(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
                if id.debug_name().split_whitespace().any(|part| part == name) {
                    return Some(id);
                }
                id.children()
                    .into_iter()
                    .find_map(|child| walk(child, name))
            }
            walk(harness.root_id(), name)
        };
        assert!(find(&harness, "approval-card").is_some());
        assert!(find(&harness, "approval-command").is_some());
        let always = find(&harness, "approval-always")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.click(always.x, always.y);
        harness.rebuild();
        harness.paint();
        let mut answered = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let crate::backend::Command::AnswerApproval {
                run_id,
                approval_id,
                answer,
            } = command
            {
                answered.push((run_id, approval_id, answer));
            }
        }
        assert_eq!(
            answered,
            [(
                "run-1".to_string(),
                "a1".to_string(),
                crate::backend::ApprovalAnswer::Always
            )]
        );
        assert!(state.approvals.with(Vec::is_empty));
        assert!(
            find(&harness, "approval-card").is_none(),
            "answered, the card goes"
        );
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
            note: false,
            pending: false,
            user: false,
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

    fn note(text: &str) -> RunStep {
        RunStep {
            summary: text.into(),
            ok: true,
            note: true,
            pending: false,
            user: false,
        }
    }

    #[test]
    fn notes_are_not_counted_as_steps() {
        let steps = vec![note("Ran it once already."), step("Ran git log", true)];
        assert_eq!(steps_summary(&steps), "Ran 1 command");
        assert_eq!(steps_summary(&[note("Thinking.")]), "No steps");
    }

    #[test]
    fn a_run_is_labelled_by_how_long_it_worked() {
        let mut run = RunRecord {
            status: RunStatus::Running,
            ..RunRecord::default()
        };
        assert_eq!(run_summary(&run), "Working…");
        run.status = RunStatus::Completed;
        run.duration_ms = Some(252_400);
        assert_eq!(run_summary(&run), "Worked for 4m 12s");
        run.duration_ms = None;
        run.started_at = Some("2026-09-19T10:00:00Z".parse().unwrap());
        run.ended_at = Some("2026-09-19T10:00:09Z".parse().unwrap());
        assert_eq!(run_summary(&run), "Worked for 9s");
        run.started_at = None;
        run.steps = vec![step("Read a.rs", true)];
        assert_eq!(run_summary(&run), "Read 1 file");
        assert_eq!(worked_for(3_780_000), "1h 3m");
    }

    #[test]
    fn a_finished_run_drops_the_notes_its_answer_says() {
        let run = RunRecord {
            steps: vec![
                note("Checking the log."),
                step("Ran git log", true),
                note("It landed last week."),
            ],
            ..RunRecord::default()
        };
        let kept = without_answer(run, "It landed last week.");
        let kept: Vec<_> = kept.steps.iter().map(|s| s.summary.as_str()).collect();
        assert_eq!(kept, ["Checking the log.", "Ran git log"]);
    }

    #[test]
    fn models_are_named_by_family_and_version() {
        assert_eq!(model_label("sonnet"), "Sonnet");
        assert_eq!(model_label("fable"), "Fable");
        assert_eq!(model_label("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("claude-opus-5-5[1m]"), "Opus 5.5");
        assert_eq!(model_label("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(model_label("claude-3-5-sonnet-20241022"), "Sonnet 3.5");
        assert_eq!(model_label("gpt-6.1-sol"), "Sol 6.1");
        assert_eq!(model_label("gpt-6-astra"), "Astra 6");
        assert_eq!(model_label("gpt-5.5"), "GPT 5.5");
        assert_eq!(model_label("gpt-5.1-codex-max"), "Max 5.1");
        assert_eq!(model_label("o3"), "o3");
        assert_eq!(model_label(""), "");
    }

    #[test]
    fn agents_are_named_by_maker_model_version_and_effort() {
        assert_eq!(
            agent_name("claude-code", "claude-opus-5-5", "low"),
            "Claude Opus 5.5 (low)"
        );
        assert_eq!(
            agent_name("codex", "gpt-6.1-sol", "xhigh"),
            "Codex Sol 6.1 (xhigh)"
        );
        assert_eq!(
            agent_name("claude-code", "opus", "High"),
            "Claude Opus (high)"
        );
        assert_eq!(agent_name("", "claude-sonnet-5", ""), "Claude Sonnet 5");
        assert_eq!(
            agent_name("", "gpt-5.5", "medium"),
            "Codex GPT 5.5 (medium)"
        );
        assert_eq!(agent_name("codex", "", "low"), "Codex (low)");
        let run = RunRecord {
            backend: "claude-code".into(),
            requested_model: "opus".into(),
            requested_effort: "high".into(),
            ..RunRecord::default()
        };
        assert_eq!(
            run_agent_name(&run),
            "Claude Opus (high)",
            "what it asked for"
        );
        let run = RunRecord {
            model: "claude-opus-5-5".into(),
            effort: "medium".into(),
            ..run
        };
        assert_eq!(
            run_agent_name(&run),
            "Claude Opus 5.5 (medium)",
            "what it ran with"
        );
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

        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::Ask { question, .. }) => assert_eq!(question, "why?"),
            other => panic!("Cmd-Enter did not send: {other:?}"),
        }
        assert!(
            state.composer.get().is_none(),
            "sending closes the composer"
        );
    }

    /// Picking a model from the chip under the box gives the keyboard back
    /// to the box, to go on typing, as the native menu never took it.
    #[test]
    fn picking_from_the_model_menu_gives_the_box_the_keyboard_back() {
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
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));

        let chip = find(harness.root_id(), "composer-effort")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.pointer_down(chip.x, chip.y);
        harness.rebuild();
        harness.paint();
        assert!(crate::ui::menu::is_open());
        let high = find(harness.root_id(), "menu-row:High")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.click(high.x, high.y);
        harness.rebuild();
        harness.paint();
        assert!(!crate::ui::menu::is_open());
        assert_eq!(
            state
                .composer
                .get()
                .and_then(|c| c.destination.agent_choice())
                .map(|(_, _, e)| e),
            Some(perch_agent::Effort::High)
        );
        assert!(
            harness.is_focused(text_box),
            "the box has the keyboard back"
        );
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
                message(
                    "gh-31",
                    ThreadOrigin::GitHub,
                    Author::GitHubUser("rohan".into()),
                    "epoch",
                ),
                message("q1", ThreadOrigin::Local, Author::User, "why?"),
                message(
                    "a1",
                    ThreadOrigin::Local,
                    Author::Agent {
                        backend: "claude-code".into(),
                        model: "sonnet".into(),
                    },
                    "because",
                ),
            ],
        }
    }

    /// A running run's lines follow the thread list: a step's result
    /// turns its line red, and the agent's words show, as they arrive and
    /// not with the next step; and the message holding them is kept, not
    /// built again for each.
    #[test]
    fn a_running_run_s_lines_follow_each_event_in_the_message_they_are_in() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let mut thread = github_thread_with_private_messages();
        thread.messages[2].body = String::new();
        thread.messages[2].run = Some(RunRecord {
            id: "run-1".into(),
            status: RunStatus::Running,
            ..RunRecord::default()
        });
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        let settle = |harness: &mut HeadlessHarness| {
            harness.rebuild();
            harness.paint();
        };
        settle(&mut harness);
        // The line over the run, which a message built again would have
        // anew; it says what the run is doing now.
        let header = |harness: &HeadlessHarness| find(harness.root_id(), "run-header").unwrap();
        let before = header(&harness);
        assert!(find(harness.root_id(), "Label: \"Thinking\"").is_some());

        state.apply_run_event(
            "run-1",
            &perch_agent::RunEvent::ToolCall {
                id: "c1".into(),
                name: "Bash".into(),
                summary: "Ran cargo test".into(),
                parent_id: None,
            },
        );
        settle(&mut harness);
        assert!(
            find(harness.root_id(), "Label: \"Running\"").is_some(),
            "the step goes"
        );
        assert!(find(harness.root_id(), "Label: \"cargo test\"").is_some());
        assert!(find(harness.root_id(), "spinner").is_some());

        state.apply_run_event(
            "run-1",
            &perch_agent::RunEvent::ToolResult {
                id: "c1".into(),
                ok: false,
                summary: "exit 101".into(),
            },
        );
        state.apply_run_event(
            "run-1",
            &perch_agent::RunEvent::Text("It failed to build.".into()),
        );
        settle(&mut harness);
        assert!(
            find(harness.root_id(), "Label: \"cargo test: exit 101\"").is_some(),
            "the result reached the line"
        );
        assert!(find(harness.root_id(), "Label: \"Ran\"").is_some());
        assert!(
            find(harness.root_id(), "It failed to build.").is_some(),
            "the words show"
        );
        assert_eq!(header(&harness), before, "the message was built again");
    }

    /// A run on the reader's question draws as the agent's comment under
    /// it, with the agent's mark and the model, as its answer will.
    #[test]
    fn a_running_run_is_the_agents_comment_under_the_question() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let mut thread = github_thread_with_private_messages();
        thread.messages.truncate(2);
        thread.messages[1].run = Some(RunRecord {
            id: "run-1".into(),
            status: RunStatus::Running,
            backend: "claude-code".into(),
            requested_model: "opus".into(),
            requested_effort: "high".into(),
            ..RunRecord::default()
        });
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(find_all(harness.root_id(), "agent-avatar").len(), 1);
        assert!(
            find(harness.root_id(), "Label: \"Claude Opus (high)\"").is_some(),
            "the requested model"
        );
        assert_eq!(
            find_all(harness.root_id(), "perch-only").len(),
            2,
            "the question and the answer on its way are both Perch's"
        );
        assert_eq!(
            find_all(harness.root_id(), "comment-rail").len(),
            3,
            "rohan, the reader and the agent"
        );

        state.comment_threads.update(|threads| {
            let run = Arc::make_mut(threads)[0].messages[1].run.as_mut().unwrap();
            run.model = "claude-opus-5-5".into();
        });
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "Label: \"Claude Opus 5.5 (high)\"").is_some(),
            "the model the CLI reported"
        );
    }

    /// While the thread's agent works its foot is Steer, whose box has no
    /// pickers and sends to the run. What is sent waits at the end of the
    /// steps until the agent reads it, and stays where it did. While the
    /// run stops there is no box; once it is over, Follow up is back.
    #[test]
    fn a_running_agent_is_steered_from_its_thread_s_foot() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut thread = github_thread_with_private_messages();
        thread.messages[2].body = String::new();
        thread.messages[2].run = Some(RunRecord {
            id: "run-1".into(),
            status: RunStatus::Running,
            ..RunRecord::default()
        });
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let backend = Rc::new(backend);
        let view = Stack::vertical((
            messages_view(
                app,
                state,
                backend.clone(),
                thread.id.clone(),
                ThreadFace::Card,
            ),
            follow_up(app, state, backend, thread.id.clone(), ThreadFace::Card),
        ))
        .style(|s| s.flex_col().width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 800.0);
        let settle = |harness: &mut HeadlessHarness| {
            harness.rebuild();
            harness.paint();
        };
        settle(&mut harness);
        assert!(shown(&harness, "Steer"));
        assert!(!shown(&harness, "Follow up"));

        state.open_composer(
            Some(thread.id.clone()),
            thread.anchor.clone(),
            "Skip the tests".into(),
            sonnet_medium(),
        );
        settle(&mut harness);
        assert!(
            shown(&harness, "composer-steer-footer"),
            "no pickers: it goes to the run"
        );
        click(&mut harness, "composer-send");
        match commands.try_recv() {
            Ok(Command::SteerRun { run_id, text }) => {
                assert_eq!(
                    (run_id.as_str(), text.as_str()),
                    ("run-1", "Skip the tests")
                );
            }
            other => panic!("Send did not steer: {other:?}"),
        }
        assert!(
            state.composer.get_untracked().is_none(),
            "sent, the box goes"
        );

        state.apply_steer("run-1", "Skip the tests");
        state.apply_run_event(
            "run-1",
            &perch_agent::RunEvent::ToolCall {
                id: "c1".into(),
                name: "Bash".into(),
                summary: "Ran cargo build".into(),
                parent_id: None,
            },
        );
        settle(&mut harness);
        let steer =
            find(harness.root_id(), "run-steer").expect("the reader's words are among the steps");
        let step = find(harness.root_id(), "Label: \"cargo build\"").unwrap();
        assert!(
            steer.get_visual_rect_no_clip().y0 > step.get_visual_rect_no_clip().y1,
            "unread, it waits after what the agent does"
        );
        assert!(shown(&harness, "run-steer-unread"));

        state.apply_run_event(
            "run-1",
            &perch_agent::RunEvent::Steered("Skip the tests".into()),
        );
        state.apply_run_event(
            "run-1",
            &perch_agent::RunEvent::Text("Skipping them.".into()),
        );
        settle(&mut harness);
        assert!(
            find(harness.root_id(), "run-steer-unread").is_none(),
            "read"
        );
        let steer = find(harness.root_id(), "run-steer")
            .unwrap()
            .get_visual_rect_no_clip();
        let words = find(harness.root_id(), "Skipping them.")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(
            words.y0 > steer.y1,
            "what it says after reading comes after: {steer:?} {words:?}"
        );

        let set_status = |status: RunStatus| {
            state.comment_threads.update(|threads| {
                let mut all = threads.as_ref().clone();
                all[0].messages[2].run.as_mut().unwrap().status = status;
                *threads = Arc::new(all);
            });
        };
        set_status(RunStatus::Stopping);
        settle(&mut harness);
        assert!(!shown(&harness, "Steer"), "nothing to steer while it stops");
        set_status(RunStatus::Stopped);
        settle(&mut harness);
        assert!(shown(&harness, "Follow up"));
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
        let view = delete_confirmation(
            state,
            Rc::new(backend),
            thread.id.clone(),
            thread.is_github_bound(),
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "Delete Perch's messages?").is_none(),
            "nothing asked yet"
        );

        state.confirm_thread.set(Some("gh-31".into()));
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Delete Perch's messages?").is_some());
        assert!(find(
            root_id,
            "The questions and answers asked here go. Nothing on GitHub is touched."
        )
        .is_some());
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
    /// Delete Thread on a card collapsed to its row opens the card, where
    /// the confirmation shows; left shut, the ask would not be seen.
    #[test]
    fn delete_on_a_collapsed_row_opens_the_card_to_ask() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let thread = finding_thread("Look here");
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let card = CardId::Thread(thread.id.clone());
        state.toggle_card(&card);
        assert!(state.is_collapsed(&card));
        let view = collapsed_row(app, state, thread.clone())
            .style(|s| s.width(600.0).height(theme::COLLAPSED_CARD_MENU + 8.0));
        let view = floem::views::Stack::new((view,)).style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        let settle = |harness: &mut HeadlessHarness| {
            for _ in 0..3 {
                harness.rebuild();
                harness.paint();
            }
        };
        settle(&mut harness);
        let center = |harness: &HeadlessHarness, name: &str| {
            find(harness.root_id(), name)
                .unwrap_or_else(|| panic!("no {name}"))
                .get_visual_rect_no_clip()
                .center()
        };
        let dots = center(&harness, "thread-menu");
        harness.pointer_down(dots.x, dots.y);
        settle(&mut harness);
        harness.pointer_up(1.0, 1.0);
        settle(&mut harness);
        let delete = center(&harness, "menu-row:Delete Thread");
        harness.click(delete.x, delete.y);
        settle(&mut harness);
        assert!(!state.is_collapsed(&card), "the card opens to ask");
        assert_eq!(state.confirm_thread.get(), Some(thread.id.clone()));
    }

    /// The menu is offered on a thread of GitHub's only while Perch has
    /// put something under it; with nothing of Perch's there, there is
    /// nothing it could delete.
    #[test]
    fn a_github_thread_has_a_menu_only_while_perch_has_messages_under_it() {
        let root = TestRoot::new();
        let state = ReviewState::new();
        let mut thread = github_thread_with_private_messages();
        let with = thread_menu(state, &thread, theme::THREAD_MENU, || {});
        thread.messages.truncate(1);
        let without = thread_menu(state, &thread, theme::THREAD_MENU, || {});
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
    /// Off macOS the word keys are Ctrl-Left and Ctrl-Right.
    #[test]
    fn word_arrows_move_the_caret_by_a_word_and_keep_the_keyboard() {
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

        let word_key = if cfg!(target_os = "macos") {
            Modifiers::ALT
        } else {
            Modifiers::CONTROL
        };
        let alt = |code: Code, name: NamedKey| key(Key::Named(name), code, word_key);
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
    /// Escape puts the list away and leaves the composer open. `#` puts
    /// up the repository's issues and pull requests the same way.
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
            viewer_can_delete: false,
            viewer_can_update: false,
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
        let view = Stack::vertical((composer, crate::review::mention::popup(app, state)))
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
                            crate::review::mention::Suggestion::Reference(r) => {
                                format!("#{}", r.reference.number)
                            }
                            crate::review::mention::Suggestion::Emoji(e) => {
                                format!(":{}:", e.shortcode)
                            }
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
        assert!(
            list.y0 > text.y0 && list.x0 > text.x0 + 10.0,
            "{list:?} {text:?}"
        );
        assert!(list.x1 <= 800.0 && list.y1 <= 400.0, "{list:?}");
        assert!(
            commands.try_recv().is_err(),
            "nothing typed is not asked of GitHub"
        );

        // Letters ask GitHub once typing rests, about the last of them;
        // meanwhile the list has who matches here, then the files.
        harness.dispatch_event(typed("c"));
        harness.dispatch_event(typed("a"));
        harness.rebuild();
        assert!(
            commands.try_recv().is_err(),
            "nothing is asked while typing"
        );
        let ask = |query: &str| crate::review::mention::Ask {
            item_id: "item-1".into(),
            repo: "acme/repo".into(),
            trigger: crate::review::mention::Trigger::At,
            query: query.into(),
        };
        // The timer for `c` finds typing moved on, and asks nothing.
        state.mentions.ask_if_waiting(&backend, &ask("c"));
        assert!(
            commands.try_recv().is_err(),
            "a query typed through is not asked"
        );
        harness.dispatch_event(named(NamedKey::Backspace, Code::Backspace));
        harness.rebuild();
        state.mentions.ask_if_waiting(&backend, &ask("c"));
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::MentionableUsers { item_id, repo_full_name, query })
                if item_id == "item-1" && repo_full_name == "acme/repo" && query == "c"
        ));
        assert_eq!(
            shown().unwrap(),
            ["@cadonna", "src/client.rs", "src/lib.rs"]
        );

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
        assert!(
            harness.is_focused(text_box),
            "the box has the keyboard back"
        );

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

        // `#` offers the repository's issues and pull requests: the recent
        // ones GitHub lists for nothing typed, then those the digits or
        // the letters name. Enter puts in the number.
        harness.dispatch_event(typed(" "));
        harness.dispatch_event(typed("#"));
        harness.rebuild();
        assert_eq!(shown(), None, "nothing until GitHub answers");
        state.mentions.ask_if_waiting(
            &backend,
            &crate::review::mention::Ask {
                trigger: crate::review::mention::Trigger::Hash,
                ..ask("")
            },
        );
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::IssueReferences { item_id, repo_full_name, query })
                if item_id == "item-1" && repo_full_name == "acme/repo" && query.is_empty()
        ));
        let reference = |number: u64, title: &str| perch_github::IssueReference {
            number,
            title: title.into(),
            state: perch_github::SubjectState::PrOpen,
        };
        state.mentions.install_references(
            String::new(),
            vec![
                reference(634, "Add RFC for CDC"),
                reference(1794, "RFC 0029"),
                reference(1380, "Pluggable filter"),
            ],
        );
        harness.rebuild();
        harness.paint();
        assert_eq!(shown().unwrap(), ["#634", "#1794", "#1380"]);
        let row = find_mention_row(&harness, "mention-row:#634");
        assert!(
            harness.get_layout_rect(row).width() > 0.0,
            "the row is drawn"
        );
        harness.dispatch_event(typed("6"));
        harness.rebuild();
        assert_eq!(shown().unwrap(), ["#634"]);
        harness.dispatch_event(named(NamedKey::Enter, Code::Enter));
        harness.rebuild();
        assert_eq!(
            state.composer_draft.get(),
            "hi @calavera @src/lib.rs @a #634 "
        );
        assert_eq!(shown(), None);

        // `:` and a letter offer emoji by their shortcodes, and nothing is
        // asked of GitHub. Enter puts in the emoji.
        harness.dispatch_event(typed(":"));
        harness.rebuild();
        assert_eq!(shown(), None, "nothing until a letter");
        for ch in ["f", "i", "r"] {
            harness.dispatch_event(typed(ch));
        }
        harness.rebuild();
        harness.paint();
        assert_eq!(shown().unwrap()[0], ":fire:");
        let row = find_mention_row(&harness, "mention-row::fire:");
        assert!(
            harness.get_layout_rect(row).width() > 0.0,
            "the row is drawn"
        );
        harness.dispatch_event(named(NamedKey::Enter, Code::Enter));
        harness.rebuild();
        assert_eq!(
            state.composer_draft.get(),
            "hi @calavera @src/lib.rs @a #634 🔥 "
        );
        assert_eq!(shown(), None);
        assert!(commands.try_recv().is_err(), "nothing asked of GitHub");
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
        assert!(
            find(root_id, "Label: \"Medium\"").is_some(),
            "the effort picker"
        );

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
        assert!(
            find(root_id, "Label: \"Medium\"").is_none(),
            "no effort for GitHub"
        );
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box), "the flip kept the keyboard");

        harness.dispatch_event(enter(crate::ui::OS_MOD));
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
        assert!(
            state.composer.get().is_none(),
            "sending closes the composer"
        );
        assert_eq!(
            state.last_destination[ThreadFace::Card.index()].get(),
            Some(Destination::GitHub)
        );
        assert_eq!(state.agent_last.get(), None, "no agent was sent to");

        state.open_composer(None, Some(anchor.clone()), String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::GitHub
        );
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
        assert!(
            !state.github_offered(&composer),
            "a closed pull request takes no draft"
        );
        assert_eq!(composer.destination, Destination::agent(sonnet_medium()));
        state.open_composer(Some("gh-31".into()), None, String::new(), sonnet_medium());
        assert!(
            !state.github_offered(&state.composer.get().unwrap()),
            "nor a reply on its thread"
        );
    }

    /// On a branch with no pull request, a message that is only a
    /// suggestion goes into the file rather than to the agent, over the
    /// lines the composer is on, and the composer is still until the
    /// backend answers. Words beside it send it to the agent as before.
    #[test]
    fn a_suggestion_on_a_branch_applies_to_the_file() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nb\nc\n".into(),
            "a\nb\nc\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        let anchor = perch_github::Anchor {
            path: "src/x.rs".into(),
            start_line: 2,
            line: 2,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        let suggestion = "```suggestion\nB\n```";
        state.open_composer(
            None,
            Some(anchor.clone()),
            suggestion.into(),
            sonnet_medium(),
        );
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        let token = match commands.try_recv() {
            Ok(Command::ApplySuggestion {
                item_id,
                token,
                path,
                original,
                lines,
                ..
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(path, "src/x.rs");
                assert_eq!(
                    original,
                    crate::review::suggestion::Original {
                        first: 2,
                        lines: vec!["b".into()]
                    }
                );
                assert_eq!(lines, vec!["B".to_string()]);
                token
            }
            other => panic!("the suggestion was not applied: {other:?}"),
        };
        assert!(state.composer.get().is_some(), "still until the answer");
        harness.paint();
        assert!(find(harness.root_id(), "Label: \"Applying\u{2026}\"").is_some());
        state.settle_issue_comment(token, Ok(()));
        assert!(state.composer.get().is_none(), "applied, the composer goes");

        let root = TestRoot::new();
        state.open_composer(
            None,
            Some(anchor),
            format!("why not?\n{suggestion}"),
            sonnet_medium(),
        );
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        assert!(matches!(commands.try_recv(), Ok(Command::Ask { .. })));
    }

    /// Over a past review, a draft said after it ended carries After
    /// Review #N in its byline, not in its text: edit opens on the words
    /// alone, so saving never writes the mark into the draft (issue 11).
    #[test]
    fn a_draft_after_a_past_review_is_marked_in_its_byline_not_its_text() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let mut thread = github_thread_with_private_messages();
        thread.messages.truncate(1);
        let said = thread.messages[0].created_at;
        let mut draft = thread.messages[0].clone();
        draft.id = "d1".into();
        draft.author = Author::User;
        draft.github_id = None;
        draft.body = "I will fix it".into();
        draft.created_at = said + jiff::SignedDuration::from_hours(1);
        thread.messages.push(draft);
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let ended = said + jiff::SignedDuration::from_mins(1);
        state
            .historical_review
            .set(Some(crate::review::model::ReviewGroupSummary {
                group: perch_github::ReviewGroup {
                    id: "group-1".into(),
                    item_id: "item-1".into(),
                    snapshot_id: "snapshot-1".into(),
                    number: 1,
                    status: perch_github::ReviewGroupStatus::Completed,
                    new_findings: 0,
                    still_present: 0,
                    verified_fixed: 0,
                    not_verified: 0,
                    created_at: said,
                    started_at: Some(said),
                    ended_at: Some(ended),
                    instructions: None,
                },
                snapshot: perch_github::ReviewSnapshot {
                    id: "snapshot-1".into(),
                    item_id: "item-1".into(),
                    repository: "slatedb/slatedb".into(),
                    branch_label: None,
                    requested_base_ref: "main".into(),
                    base_tip_sha: "base".into(),
                    merge_base_sha: "base".into(),
                    head_sha: "head".into(),
                    dirty_edits_excluded: false,
                    retention_key: "snapshot-1".into(),
                    retained_ref: None,
                    manifest_path: None,
                    diff_path: None,
                    created_at: said,
                    worktree_parent_sha: None,
                },
                attempts: Vec::new(),
            }));
        let (backend, _commands) = crate::backend::Backend::recording();
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let shown = find_all(root_id, "after-review")
            .into_iter()
            .filter(|id| harness.get_layout_rect(*id).width() > 0.0)
            .count();
        assert_eq!(shown, 1, "only the draft came after the review");
        assert!(find(root_id, "Label: \"After Review #1\"").is_some());

        let pill = find(root_id, "draft-pill")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.pointer_move(pill.x, pill.y);
        harness.rebuild();
        harness.paint();
        let edit = find(root_id, "edit-draft").expect("edit");
        let c = edit.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert_eq!(state.composer_draft.get(), "I will fix it");

        // Back on the live item, the mark goes.
        state.composer.set(None);
        state.clear_historical_review();
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find_all(root_id, "after-review")
            .into_iter()
            .all(|id| harness.get_layout_rect(id).width() == 0.0));
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
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let pill = find(root_id, "draft-pill").expect("the Draft mark");
        assert!(harness.get_layout_rect(pill).width() > 0.0);
        let edit = find(root_id, "edit-draft").expect("edit");
        let delete = find(root_id, "delete-draft").expect("delete");
        assert_eq!(
            harness.get_layout_rect(edit).width(),
            0.0,
            "hidden until hovered"
        );

        // Under the pointer, the actions show.
        let body = find(root_id, "draft-pill")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.pointer_move(body.center().x, body.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(harness.get_layout_rect(edit).width(), 24.0);
        assert_eq!(harness.get_layout_rect(delete).width(), 24.0);

        let c = delete.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        match commands.try_recv() {
            Ok(Command::DeleteDraft {
                item_id,
                message_id,
            }) => {
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
        assert!(
            find(root_id, "draft-pill").is_none(),
            "the composer took the draft's row"
        );
        assert!(find(root_id, "Label: \"Add to review\"").is_some());
        // The box keeps its own padding, and the room under the last row
        // is outside its border, so it does not sit on the card's edge.
        let composer_box = find(root_id, "composer-box").expect("the composer");
        let boxed = composer_box.get_visual_rect_no_clip();
        let row = composer_box.parent().unwrap().get_visual_rect_no_clip();
        assert_eq!(boxed.y0 - row.y0, theme::THREAD_COMMENT_SPACE);
        assert_eq!(row.y1 - boxed.y1, theme::THREAD_INSET);
        let first = composer_box.children()[0].get_visual_rect_no_clip();
        assert_eq!(first.y0 - boxed.y0, 13.0, "border and padding");

        // Escape, with the text still in the box, gives the row back: the
        // words are safe in the cache.
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));
        harness.dispatch_event(key(
            Key::Named(NamedKey::Escape),
            Code::Escape,
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none(), "Escape closed the edit");
        assert!(
            find(harness.root_id(), "draft-pill").is_some(),
            "the draft is back"
        );

        // Edited and sent: the backend is asked to rewrite that draft, in
        // its thread, and the composer closes. Nothing is remembered for
        // the face; an edit is GitHub's by nature, not by choice.
        let root_id = harness.root_id();
        let pill = find(root_id, "draft-pill")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
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
        harness.dispatch_event(key(
            Key::Character("a".into()),
            Code::KeyA,
            crate::ui::OS_MOD,
        ));
        harness.dispatch_event(key(
            Key::Named(NamedKey::Backspace),
            Code::Backspace,
            Modifiers::empty(),
        ));
        for ch in "I fixed it".chars() {
            harness.dispatch_event(key(
                Key::Character(ch.to_string()),
                Code::KeyA,
                Modifiers::empty(),
            ));
        }
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::SaveDraft {
                item_id,
                thread_id,
                anchor,
                body,
                editing,
            }) => {
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

    /// Someone's comment on a line that GitHub says the reader may edit
    /// has Edit under the pointer, which opens the composer in its row
    /// with Update comment alone; sent, the new words show at once and
    /// the backend is asked to put them on GitHub. Without GitHub's say
    /// so there is no Edit.
    #[test]
    fn a_line_comment_the_reader_may_edit_is_edited_in_its_row() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let mut thread = github_thread_with_private_messages();
        thread.messages.truncate(1);
        thread.messages[0].node_id = Some("PRRC_31".into());
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let line = perch_github::ReviewComment {
            node_id: "PRRC_31".into(),
            reactions: Default::default(),
            id: 31,
            pull_request_review_id: None,
            in_reply_to_id: None,
            user: "rohan".into(),
            body: "epoch".into(),
            path: "src/x.rs".into(),
            line: Some(7),
            original_line: None,
            start_line: None,
            original_start_line: None,
            start_side: None,
            commit_id: String::new(),
            original_commit_id: String::new(),
            user_avatar_url: String::new(),
            diff_hunk: String::new(),
            side: perch_github::Side::Right,
            created_at: thread.created_at,
            updated_at: None,
            html_url: String::new(),
            viewer_can_delete: false,
            viewer_can_update: false,
        };
        let discussion = |updatable: bool| perch_github::Discussion {
            review_comments: vec![perch_github::ReviewComment {
                viewer_can_update: updatable,
                ..line.clone()
            }],
            ..Default::default()
        };
        state.discussion.set(Some(Arc::new(discussion(false))));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "edit-comment").is_none(),
            "not without GitHub's say so"
        );

        state.discussion.set(Some(Arc::new(discussion(true))));
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let edit = find(root_id, "edit-comment").expect("GitHub says the reader may");
        let at = find(root_id, "RichText")
            .expect("the text")
            .get_visual_rect_no_clip()
            .center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(harness.get_layout_rect(edit).width(), 24.0);
        let c = edit.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("Edit opened the composer");
        assert_eq!(composer.editing.as_deref(), Some("gh-31"));
        assert_eq!(state.composer_draft.get(), "epoch");
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Label: \"Update comment\"").is_some());
        assert!(find(root_id, "Label: \"Add to review\"").is_none());

        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));
        harness.dispatch_event(key(
            Key::Character("a".into()),
            Code::KeyA,
            crate::ui::OS_MOD,
        ));
        harness.dispatch_event(key(
            Key::Named(NamedKey::Backspace),
            Code::Backspace,
            Modifiers::empty(),
        ));
        for ch in "epochs".chars() {
            harness.dispatch_event(key(
                Key::Character(ch.to_string()),
                Code::KeyA,
                Modifiers::empty(),
            ));
        }
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::UpdateGithubComment {
                item_id,
                node_id,
                place,
                body,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(node_id, "PRRC_31");
                assert_eq!(place, perch_github::CommentPlace::Line);
                assert_eq!(body, "epochs");
            }
            other => panic!("Cmd-Enter did not edit the comment: {other:?}"),
        }
        assert!(state.composer.get().is_none(), "sending closes the edit");
        let shown = state.comment_threads.get()[0].messages[0].body.clone();
        assert_eq!(shown, "epochs", "shown before GitHub answers");
        let kept = state.discussion.get().unwrap();
        assert_eq!(kept.review_comments[0].body, "epochs");
    }

    /// A comment of the reader's that posted: their byline with the time,
    /// the link and the reactions, and no Draft mark, edit or delete,
    /// whether it opened its thread or answered someone's; the thread it
    /// opened has no byline of its own over it. Under the pointer it has
    /// Reply, Ask about this, the picker and the link, as anyone's
    /// comment has, and none of them otherwise.
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
        state
            .comment_threads
            .set(Arc::new(vec![theirs.clone(), ours.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let backend = Rc::new(backend);
        let view = floem::views::Stack::vertical((
            messages_view(
                app,
                state,
                backend.clone(),
                theirs.id.clone(),
                ThreadFace::Card,
            )
            .debug_name("theirs"),
            cards::thread_card(app, state, backend, Rc::new(|_| {}), ours).debug_name("ours"),
        ))
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 900.0);
        harness.rebuild();
        harness.paint();
        for name in ["theirs", "ours"] {
            let card = find(harness.root_id(), name).unwrap();
            assert!(
                find(card, "posted-when").is_some(),
                "{name}: the comment's time"
            );
            assert!(find(card, "github-link").is_some(), "{name}: its link");
            assert!(find(card, "reactions").is_some(), "{name}: its reactions");
            assert!(
                find(card, "draft-pill").is_none(),
                "{name}: not a draft any more"
            );
            assert!(
                find(card, "edit-draft").is_none(),
                "{name}: not edited here"
            );
            assert!(
                find(card, "delete-draft").is_none(),
                "{name}: not deleted here"
            );
            assert!(
                find(card, "Label: \"You\"").is_some(),
                "{name}: the reader's"
            );
        }
        assert!(
            find(find(harness.root_id(), "theirs").unwrap(), "reply-github").is_some(),
            "rohan's comment offers Reply"
        );
        assert!(
            find(find(harness.root_id(), "ours").unwrap(), "You asked").is_none(),
            "the thread the reader opened on GitHub has no byline over its comment"
        );

        let ours = find(harness.root_id(), "ours").unwrap();
        let actions = ["reply-github", "ask-about", "react-button", "github-link"];
        let drawn = |harness: &HeadlessHarness| -> Vec<bool> {
            actions
                .iter()
                .map(|name| {
                    let id = find(ours, name).unwrap_or_else(|| panic!("the comment has {name}"));
                    harness.get_layout_rect(id).height() > 0.0
                })
                .collect()
        };
        assert_eq!(
            drawn(&harness),
            [false; 4],
            "nothing until the pointer is on it"
        );
        let at = find(ours, "posted-when")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(drawn(&harness), [true; 4], "all four under the pointer");
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
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let reply = find(harness.root_id(), "reply-github").expect("Reply on rohan's comment");
        assert_eq!(
            harness.get_layout_rect(reply).width(),
            0.0,
            "hidden until hovered"
        );
        let at = find(harness.root_id(), "Label: \"rohan\"")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
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
        state.open_composer(
            Some("gh-31".into()),
            thread.anchor.clone(),
            String::new(),
            sonnet_medium(),
        );
        assert!(!state.composer.get().unwrap().destination.is_github());
        state.close_composer();

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "reply-github").is_none(),
            "nowhere for a draft to go"
        );
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
        answer.author = Author::Agent {
            backend: "claude-code".into(),
            model: "opus".into(),
        };
        answer.run = Some(perch_github::RunRecord {
            id: "r1".into(),
            backend: "claude-code".into(),
            model: "opus".into(),
            effort: "high".into(),
            status: perch_github::RunStatus::Completed,
            ..Default::default()
        });
        state.comment_threads.set(Arc::new(vec![answered.clone()]));
        state.open_composer(
            Some(answered.id.clone()),
            answered.anchor.clone(),
            String::new(),
            sonnet_medium(),
        );
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
        state.open_composer(
            Some(theirs.id.clone()),
            theirs.anchor.clone(),
            String::new(),
            sonnet_medium(),
        );
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::GitHub
        );
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
        let ask = find(harness.root_id(), "ask-about").expect("Ask about this");
        let at = harness.get_layout_rect(harness.root_id()).center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        let c = ask.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state
            .composer
            .get()
            .expect("Ask about this opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(
            composer.destination,
            Destination::agent((
                perch_agent::Backend::ClaudeCode,
                "opus".into(),
                perch_agent::Effort::High
            ))
        );
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
        assert_eq!(
            harness.get_layout_rect(post).width(),
            0.0,
            "hidden until hovered"
        );
        assert_eq!(
            harness.get_layout_rect(copy).width(),
            0.0,
            "hidden until hovered"
        );

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
        let composer = state
            .composer
            .get()
            .expect("Post to GitHub opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.anchor, thread.anchor);
        assert_eq!(composer.editing, None, "a new draft, not an edit");
        assert_eq!(state.composer_draft.get(), "because");
        assert!(
            find(root_id, "edit-finding").is_none(),
            "an answer is not edited"
        );
    }

    /// A finding's message has Edit under the pointer, so the reader can
    /// trim what the reviewer said, its signature too, before posting it.
    /// Edit opens the composer in the message's row with the text and
    /// Save alone, and a save asks the backend to rewrite that message.
    #[test]
    fn a_findings_message_is_edited_in_its_row_and_saved() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = finding_thread("**High · Lock**\n\nTaken twice.\n\n_Reported by Claude._");
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        let edit = find(root_id, "edit-finding").expect("a finding offers Edit");
        let at = find(root_id, "RichText")
            .expect("the text")
            .get_visual_rect_no_clip()
            .center();
        harness.pointer_move(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(harness.get_layout_rect(edit).width(), 24.0);

        let c = edit.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        let composer = state.composer.get().expect("Edit opened the composer");
        assert_eq!(composer.editing.as_deref(), Some("f1"));
        assert_eq!(composer.thread_id.as_deref(), Some("finding-1"));
        assert_eq!(state.composer_draft.get(), thread.messages[0].body);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(
            find(root_id, "edit-finding").is_none(),
            "the composer took the row"
        );
        assert!(find(root_id, "Label: \"Save\"").is_some());
        assert!(find(root_id, "Label: \"Add to review\"").is_none());
        assert!(
            find(root_id, "composer-destination").is_none(),
            "it goes nowhere"
        );

        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));
        harness.dispatch_event(key(
            Key::Character("a".into()),
            Code::KeyA,
            crate::ui::OS_MOD,
        ));
        harness.dispatch_event(key(
            Key::Named(NamedKey::Backspace),
            Code::Backspace,
            Modifiers::empty(),
        ));
        for ch in "Taken twice.".chars() {
            harness.dispatch_event(key(
                Key::Character(ch.to_string()),
                Code::KeyA,
                Modifiers::empty(),
            ));
        }
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::EditReviewerMessage {
                item_id,
                message_id,
                body,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(message_id, "f1");
                assert_eq!(body, "Taken twice.");
            }
            other => panic!("Cmd-Enter did not rewrite the finding: {other:?}"),
        }
        assert!(state.composer.get().is_none(), "saving closes the edit");
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
        let actions = find(root_id, "agent-actions")
            .unwrap()
            .get_visual_rect_no_clip();
        let prose = find(root_id, "RichText")
            .expect("the finding's text")
            .get_visual_rect_no_clip();
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
        assert!(
            state.github_offered_on(&neither),
            "neither, so an issue comment"
        );

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        assert!(
            !state.github_offered_on(&lines),
            "no review to take the draft"
        );
        assert!(!state.github_offered_on(&place), "nor a reply for one");
        assert!(
            state.github_offered_on(&neither),
            "a closed pull request still takes a comment"
        );

        state.pull_request.set(None);
        assert!(
            !state.github_offered_on(&neither),
            "a branch has nowhere to post"
        );
        state.issue.set(Some(Arc::new(issue())));
        assert!(
            state.github_offered_on(&neither),
            "an issue takes the comment"
        );
        assert!(!state.github_offered_on(&lines), "but no draft");
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
                author: Author::Agent {
                    backend: "claude-code".into(),
                    model: "sonnet".into(),
                },
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
        let composer = state
            .composer
            .get()
            .expect("Post to GitHub opened the composer");
        assert_eq!(composer.thread_id.as_deref(), Some("ours"));
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(composer.anchor, None);
        assert_eq!(state.composer_draft.get(), "because");
        assert!(
            state.posts_issue_comment(&composer),
            "an issue comment, not a draft"
        );

        // An issue has no pull request to comment on.
        state.pull_request.set(None);
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "post-to-github").is_none(),
            "nowhere at all"
        );
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
        assert!(
            find(harness.root_id(), "post-to-github").is_none(),
            "no pull request yet"
        );

        state.pull_request.set(Some(Arc::new(pull())));
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "post-to-github").is_some(),
            "the pull request landed"
        );

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "post-to-github").is_none(),
            "closed takes no review"
        );
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
            [
                Lane::Flush,
                Lane::PrivateStart,
                Lane::PrivateContinue,
                Lane::Flush
            ]
        );

        let mut split = thread.clone();
        split.messages.swap(2, 3);
        assert_eq!(
            Lane::of(&split),
            [
                Lane::Flush,
                Lane::PrivateStart,
                Lane::Flush,
                Lane::PrivateStart
            ]
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
    /// between them, and in the code alike.
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

            assert_eq!(
                find_all(root_id, "comment-byline").len(),
                4,
                "{face:?}: a byline per message"
            );
            let stack = find(root_id, "thread-messages").expect("the messages");
            let rows = stack.children();
            assert_eq!(rows.len(), 4, "{face:?}: a row per message");
            let labelled: Vec<bool> = rows
                .iter()
                .map(|r| find(*r, "perch-only").is_some())
                .collect();
            assert_eq!(
                labelled,
                [false, true, true, false],
                "{face:?}: the question and the answer"
            );
            // And on a grey band from one side of the box to the other,
            // so what a colleague can see is the white.
            let banded: Vec<bool> = rows
                .iter()
                .map(|r| r.debug_name().contains("private-band"))
                .collect();
            assert_eq!(banded, [false, true, true, false], "{face:?}: banded");
            let band = rows[1].get_visual_rect_no_clip();
            let messages = stack.get_visual_rect_no_clip();
            assert_eq!(
                (band.x0, band.x1),
                (messages.x0, messages.x1),
                "{face:?}: edge to edge"
            );
            for label in find_all(root_id, "perch-only") {
                assert!(
                    harness.get_layout_rect(label).width() > 0.0,
                    "{face:?}: the label is drawn"
                );
            }
            assert!(
                find(rows[3], "draft-pill").is_some(),
                "{face:?}: the draft is last"
            );
            assert!(
                find(rows[0], "ask-about").is_some(),
                "{face:?}: rohan's comment"
            );

            // Every byline starts at one place, past the avatar, and the
            // text under it starts there too, the first message's in the
            // code as well.
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
                let byline = find(*row, "comment-byline")
                    .unwrap()
                    .get_visual_rect_no_clip();
                assert_eq!(byline.x0 - left, past_avatar, "{face:?}: row {i}'s byline");
                let text = find(*row, "LinkedText")
                    .or_else(|| find(*row, "RichText"))
                    .expect("the message's text")
                    .get_visual_rect_no_clip();
                assert_eq!(text.x0 - left, past_avatar, "{face:?}: row {i}'s text");
                // The line down the avatars runs beside every message.
                assert!(
                    find(*row, "comment-rail").is_some(),
                    "{face:?}: row {i}'s rail"
                );
            }
        }
    }

    /// In the code, under a thread that ends on a grey band, the Follow up
    /// box is set off from the band by the inset it has at its sides; under
    /// a white message the message's own room is enough.
    #[test]
    fn the_inline_follow_up_box_stands_off_a_grey_band_above_it() {
        let mut offsets = Vec::new();
        for ends_private in [true, false] {
            let root = TestRoot::new();
            let app = AppState::new();
            let state = state_with_item();
            let mut thread = github_thread_with_private_messages();
            if !ends_private {
                thread.messages.push(draft_under(&thread));
            }
            state.comment_threads.set(Arc::new(vec![thread.clone()]));
            let (backend, _commands) = crate::backend::Backend::recording();
            let view = follow_up(
                app,
                state,
                Rc::new(backend),
                thread.id.clone(),
                ThreadFace::Inline,
            )
            .style(|s| s.width(600.0));
            let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
            harness.rebuild();
            harness.paint();
            let bar = find(harness.root_id(), "reply-bar")
                .unwrap()
                .get_visual_rect_no_clip();
            let line = find(harness.root_id(), "Follow up")
                .unwrap()
                .get_visual_rect_no_clip();
            offsets.push(line.y0 - bar.y0);
        }
        assert_eq!(
            offsets[0] - offsets[1],
            theme::INLINE_THREAD_INSET,
            "{offsets:?}"
        );
    }

    /// Folded, a GitHub thread that holds the reader's asks says how many,
    /// so folding it does not hide them; one of the reader's own says
    /// nothing, since all of it is.
    #[test]
    fn a_folded_github_thread_counts_the_messages_only_perch_has() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let theirs = github_thread_with_private_messages();
        let mut ours = theirs.clone();
        ours.id = "t-ours".into();
        ours.origin = ThreadOrigin::Local;
        ours.github_id = None;
        ours.messages.remove(0);
        for thread in [&theirs, &ours] {
            state.toggle_card(&CardId::Thread(thread.id.clone()));
        }
        state
            .comment_threads
            .set(Arc::new(vec![theirs.clone(), ours.clone()]));
        let (backend, _commands) = crate::backend::Backend::recording();
        let backend = Rc::new(backend);
        let view = floem::views::Stack::vertical((
            thread_card(app, state, backend.clone(), theirs).debug_name("theirs"),
            thread_card(app, state, backend, ours).debug_name("ours"),
        ))
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();

        let theirs = find(harness.root_id(), "theirs").unwrap();
        let count = find(theirs, "perch-only-count").expect("the asks are counted");
        assert_eq!(texts(count).join(" "), "2 Perch only");
        let ours = find(harness.root_id(), "ours").unwrap();
        assert!(find(ours, "perch-only-count").is_none(), "nothing to count");
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
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "perch-only").is_none());
        assert_eq!(
            find_all(root_id, "comment-byline").len(),
            2,
            "the question and the answer have bylines"
        );
        assert!(
            find(root_id, "agent-avatar").is_some(),
            "the answer is under the agent's mark"
        );

        // A draft added under it labels what was there.
        let mut with_draft = thread.clone();
        let mut draft = draft_under(&with_draft);
        draft.thread_id = "t1".into();
        with_draft.messages.push(draft);
        state.comment_threads.set(Arc::new(vec![with_draft]));
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert_eq!(
            find_all(root_id, "perch-only").len(),
            2,
            "the question and the answer are labelled"
        );
        assert!(find(root_id, "draft-pill").is_some());
    }

    /// On an issue a new question offers Ask and Work, and Work is Start
    /// work: its button says so, and sending cuts the branch with the
    /// question for the agent instead of asking it here. The issue's next
    /// question is a question again.
    #[test]
    fn work_on_an_issue_starts_work_with_the_question() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_item();
        let mut item = (*state.item.get_untracked().unwrap()).clone();
        item.subject = Some(perch_github::ItemSubject::Issue(issue()));
        state.item.set(Some(Arc::new(item)));
        state.open_composer(None, None, "Use a CAS.".into(), sonnet_medium());
        let composer = state.composer.get().unwrap();
        assert_eq!(composer.mode, perch_github::RunMode::Ask);
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "composer-mode").is_some());
        assert!(find(harness.root_id(), "Label: \"Send\"").is_some());

        click(&mut harness, "composer-mode:work");
        assert!(find(harness.root_id(), "Label: \"Start work\"").is_some());
        // The click took the keyboard; the box has it back to send from.
        state.composer_box.get().unwrap().request_focus();
        harness.rebuild();
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        match commands.try_recv() {
            Ok(Command::StartWork {
                item_id,
                instructions,
                ..
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(instructions.as_deref(), Some("Use a CAS."));
            }
            other => panic!("Work did not start work: {other:?}"),
        }
        assert!(state.composer.get().is_none(), "the text went with it");

        state.open_composer(None, None, String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().mode,
            perch_github::RunMode::Ask
        );
    }

    fn issue() -> perch_github::Issue {
        perch_github::Issue {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            number: 43,
            title: "Fence the manifest".into(),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "octocat".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: "2026-09-16T11:40:00Z".parse().unwrap(),
            repo_full_name: "acme/repo".into(),
        }
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
        key(
            Key::Named(NamedKey::Escape),
            Code::Escape,
            Modifiers::empty(),
        )
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
            Ok(Command::ResolveThread {
                item_id,
                thread_id,
                resolve,
            }) => {
                assert_eq!(
                    (item_id.as_str(), thread_id.as_str(), resolve),
                    ("item-1", "gh-31", true)
                );
            }
            other => panic!("Resolve asked nothing: {other:?}"),
        }
        assert!(state.is_resolved("gh-31"));
        assert!(state.is_collapsed(&CardId::Thread("gh-31".into())));
        assert!(
            !shown(&harness, "thread-resolve"),
            "folded away with the card"
        );
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
        let view = follow_up(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-unresolve"));

        click(&mut harness, "thread-unresolve");
        assert!(
            matches!(
                commands.try_recv(),
                Ok(Command::ResolveThread { resolve: false, .. })
            ),
            "Unresolve asks to unresolve"
        );
        assert!(
            shown(&harness, "thread-resolve"),
            "shown unresolved before GitHub answers"
        );
        assert!(!shown(&harness, "thread-unresolve"));
        click(&mut harness, "thread-resolve");
        assert!(commands.try_recv().is_err(), "the first ask is unanswered");

        state.settle_resolution("gh-31");
        harness.rebuild();
        harness.paint();
        assert!(
            shown(&harness, "thread-unresolve"),
            "refused, it is as GitHub has it"
        );
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
        let view = follow_up(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "thread-resolve"));
        assert!(
            shown(&harness, "Follow up"),
            "the line is there all the same"
        );

        thread.can_resolve = true;
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-resolve"));
        assert!(
            find(harness.root_id(), "Label: \"Resolve conversation\"").is_some(),
            "GitHub's words"
        );
        let line = find(harness.root_id(), "Follow up")
            .unwrap()
            .get_visual_rect_no_clip();
        let button = find(harness.root_id(), "thread-resolve")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(
            button.y0 > line.y1,
            "under the Follow up box: {line:?} {button:?}"
        );
        assert_eq!(
            button.x0,
            theme::THREAD_INSET,
            "at the thread's left, in its inset: {button:?}"
        );
        // The box sits past the reader's face, in the bar's inset.
        let bar = find(harness.root_id(), "reply-bar")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(bar.width(), 600.0, "the bar is the thread's width");
        assert_eq!(
            line.x0,
            theme::THREAD_INSET + theme::THREAD_AVATAR + theme::THREAD_AVATAR_GAP,
            "the box is past the face"
        );
        assert_eq!(
            line.x1,
            600.0 - theme::THREAD_INSET,
            "and reaches the bar's inset"
        );

        state.open_composer(
            Some(thread.id.clone()),
            thread.anchor.clone(),
            String::new(),
            sonnet_medium(),
        );
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "thread-resolve"));
        let composer =
            find(harness.root_id(), "composer-box").expect("the composer is under the thread");
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
    /// instead, and no Resolve, until it is published: then it is one of
    /// GitHub's and resolves there.
    #[test]
    fn a_thread_of_perchs_own_resolves_in_perch_and_a_findings_does_not() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = finding_thread("because");
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = follow_up(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(
            shown(&harness, "thread-resolve"),
            "the reader's own, whatever GitHub allows"
        );

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
                title: "The lock is held across an await.".into(),
            },
        );
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "thread-resolve"));
        assert!(!shown(&harness, "thread-unresolve"));

        // Published, the finding's thread is GitHub's to resolve.
        let mut published = thread;
        published.github_id = Some(4077107818);
        published.node_id = Some("PRRT_1".into());
        published.can_resolve = true;
        state.comment_threads.set(Arc::new(vec![published]));
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "thread-resolve"), "GitHub lets the reader");
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
        let view = cards::thread_card(
            app,
            state,
            Rc::new(backend),
            Rc::new(|_| {}),
            thread.clone(),
        )
        .style(|s| s.width(700.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 800.0);
        harness.rebuild();
        harness.paint();
        let comment = "name label - Label: \"rohan\"";
        assert!(shown(&harness, comment), "open, the comment shows");
        assert!(shown(&harness, "thread-path"), "the header names the file");
        assert!(
            !shown(&harness, "resolved-toggle"),
            "nothing to show or hide on a thread not resolved"
        );

        // The chevron folds it to its header, and opens it again.
        click(&mut harness, "thread-chevron");
        assert!(state.is_folded("gh-31"));
        assert!(
            find(harness.root_id(), comment).is_none(),
            "folded to its header"
        );
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
            Some(Command::ResolveThread {
                thread_id,
                resolve: true,
                ..
            }) => assert_eq!(thread_id, "gh-31"),
            other => panic!("Resolve asked nothing: {other:?}"),
        }
        assert!(
            find(harness.root_id(), comment).is_none(),
            "resolved, folded to its header"
        );
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
        state
            .comment_threads
            .set(Arc::new(vec![theirs.clone(), ours.clone()]));
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
        assert!(
            find(ours, "Label: \"Perch only\"").is_some(),
            "marked as the reader's own"
        );

        let theirs = find(harness.root_id(), "theirs").unwrap();
        let row = find(theirs, "resolved-row").expect("resolved, so collapsed to its row");
        assert_eq!(
            texts(find(row, "lines-heading").unwrap()).join(" "),
            "Comment on line R7",
            "the row says where the thread is, as GitHub's does"
        );
        assert!(
            find(row, "name label").is_none(),
            "and not who spoke or what they said"
        );
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
        assert_eq!(
            texts(find(theirs, "resolved-toggle").unwrap()),
            ["Hide resolved"]
        );
        assert!(find(theirs, "Label: \"Unresolve comment\"").is_some());
        assert!(
            state.is_folded("gh-31"),
            "opening it in the code leaves Conversation folded"
        );
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
        let view = messages_view(
            app,
            state,
            Rc::new(backend),
            thread.id.clone(),
            ThreadFace::Card,
        )
        .style(|s| s.width(600.0));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let rows = find(harness.root_id(), "thread-messages")
            .unwrap()
            .children();
        let marked: Vec<bool> = rows
            .iter()
            .map(|row| {
                find(*row, "author-label").is_some_and(|l| harness.get_layout_rect(l).width() > 0.0)
            })
            .collect();
        assert_eq!(
            marked,
            [false, true],
            "octocat opened the pull request; rohan did not"
        );
    }

    /// At the foot of a pull request's conversation GitHub is offered
    /// and posts an issue comment: under a thread that is on GitHub it is
    /// a reply that posts at once, under one of Perch's own with no lines
    /// it is an issue comment too, and on an issue it is not offered.
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
        assert_eq!(
            foot.destination,
            Destination::agent(sonnet_medium()),
            "an agent first"
        );
        assert!(state.github_offered(&foot));
        assert!(state.posts_issue_comment(&foot));

        state.open_composer(Some("theirs".into()), None, String::new(), sonnet_medium());
        let reply = state.composer.get().unwrap();
        assert!(state.github_offered(&reply));
        assert!(
            !state.posts_issue_comment(&reply),
            "a reply, not an issue comment"
        );
        assert!(
            state.posts_reply(&reply),
            "a reply on GitHub's thread posts at once"
        );
        assert!(!state.posts_reply(&foot));

        state.open_composer(Some("ours".into()), None, String::new(), sonnet_medium());
        let follow_up = state.composer.get().unwrap();
        assert!(state.github_offered(&follow_up));
        assert!(
            state.posts_issue_comment(&follow_up),
            "nowhere on GitHub to reply"
        );

        state.pull_request.set(None);
        state.open_composer(None, None, String::new(), sonnet_medium());
        let branch = state.composer.get().unwrap();
        assert!(!state.github_offered(&branch), "a branch takes no comment");

        state.issue.set(Some(Arc::new(issue())));
        state.open_composer(Some("ours".into()), None, String::new(), sonnet_medium());
        let on_issue = state.composer.get().unwrap();
        assert!(state.github_offered(&on_issue), "an issue takes a comment");
        assert!(state.posts_issue_comment(&on_issue));
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
        assert!(
            find(root_id, "Label: \"Medium\"").is_none(),
            "no effort for GitHub"
        );
        let text_box = state.composer_box.get().expect("the box registered itself");
        assert!(harness.is_focused(text_box));

        // The first send asks.
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        harness.paint();
        assert!(
            commands.try_recv().is_err(),
            "nothing posted before the yes"
        );
        assert!(state.composer_confirming.get());
        let root_id = harness.root_id();
        assert!(find(root_id, "Post to github.com/acme/repo/pull/42?").is_some());
        assert!(find(root_id, "composer-post").is_some());
        assert!(
            find(root_id, "Label: \"Comment\"").is_none(),
            "the send gave way to the ask"
        );

        // Escape takes it back, text and all.
        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(!state.composer_confirming.get());
        assert!(state.composer.get().is_some(), "the composer stays");
        assert_eq!(state.composer_draft.get(), "Looks good");
        assert!(find(harness.root_id(), "Label: \"Comment\"").is_some());

        // Enter alone is the yes.
        harness.dispatch_event(enter(crate::ui::OS_MOD));
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
        assert_eq!(
            state.composer_draft.get(),
            "Looks good",
            "Enter did not break the line"
        );
        assert!(!state.composer_confirming.get());
        let posting = state.posting.get().expect("the comment is in flight");
        assert_eq!(posting.token, token);
        assert_eq!(posting.body, "Looks good");
        assert!(
            state.composer.get().is_some(),
            "the composer stays open while posting"
        );
        let root_id = harness.root_id();
        assert!(find(root_id, "composer-posting").is_some());
        assert!(
            find(root_id, "Label: \"Comment\"").is_none(),
            "nothing to click while posting"
        );
        assert_eq!(
            state.last_destination[ThreadFace::Card.index()].get(),
            Some(Destination::GitHub)
        );

        // Nothing sends twice, and the keys do nothing to the text.
        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.dispatch_event(enter(Modifiers::empty()));
        harness.rebuild();
        assert!(commands.try_recv().is_err());
        assert_eq!(state.composer_draft.get(), "Looks good");
        // Nor can the box be put away from under the comment.
        assert!(
            !shown(&harness, "composer-close"),
            "no closing while posting"
        );

        // An answer to some other send is not this one's.
        state.settle_issue_comment(token + 100, Err("stale".into()));
        assert!(state.posting.get().is_some());
        assert!(state.composer_error.get().is_none());

        // Refused: the text is back with the reason under it.
        state.settle_issue_comment(token, Err("GitHub could not post the comment: 403".into()));
        harness.rebuild();
        harness.paint();
        assert!(state.posting.get().is_none());
        assert!(
            state.composer.get().is_some(),
            "the composer stays with its text"
        );
        assert_eq!(state.composer_draft.get(), "Looks good");
        let root_id = harness.root_id();
        assert!(find(root_id, "composer-error").is_some());
        assert!(find(root_id, "GitHub could not post the comment: 403").is_some());
        assert!(
            find(root_id, "Label: \"Comment\"").is_some(),
            "the send is back"
        );
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
        assert!(
            state.composer_error.get().is_none(),
            "a new try clears the old reason"
        );
        state.settle_issue_comment(token, Ok(()));
        assert!(state.posting.get().is_none());
        assert!(
            state.composer.get().is_none(),
            "posted, the composer closes"
        );
        assert_eq!(state.composer_draft.get(), "");

        // The foot still opens on an agent; GitHub is a click away.
        state.open_composer(None, None, String::new(), sonnet_medium());
        assert_eq!(
            state.composer.get().unwrap().destination,
            Destination::agent(sonnet_medium())
        );
    }

    /// Under a thread that is on GitHub the button reads Reply, and a send
    /// posts the reply at once, unasked, as github.com's Reply does: no
    /// draft waits for the review. The box is still while it goes, a
    /// refusal keeps the text, and a reply that posted closes the
    /// composer. A draft under the thread, edited, stays a draft.
    #[test]
    fn a_reply_under_githubs_thread_posts_at_once() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_pull_request();
        let thread = github_thread_with_private_messages();
        state.comment_threads.set(Arc::new(vec![thread.clone()]));
        state.open_composer(
            Some(thread.id.clone()),
            None,
            "Fixed in abc".into(),
            sonnet_medium(),
        );
        state.set_destination(Destination::GitHub);
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        let root_id = harness.root_id();
        assert!(find(root_id, "Label: \"Reply\"").is_some());
        assert!(find(root_id, "Label: \"Add to review\"").is_none());

        harness.dispatch_event(enter(crate::ui::OS_MOD));
        harness.rebuild();
        harness.paint();
        assert!(
            !state.composer_confirming.get(),
            "a reply is not asked about"
        );
        let token = match commands.try_recv() {
            Ok(Command::ReplyToThread {
                item_id,
                token,
                thread_id,
                body,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(thread_id, thread.id);
                assert_eq!(body, "Fixed in abc");
                token
            }
            other => panic!("Cmd-Enter did not reply: {other:?}"),
        };
        assert!(commands.try_recv().is_err(), "no draft beside it");
        assert_eq!(state.posting.get().map(|p| p.token), Some(token));
        assert!(find(harness.root_id(), "composer-posting").is_some());

        state.settle_issue_comment(token, Err("GitHub could not post the reply: 422".into()));
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_some(), "refused, the text stays");
        assert_eq!(state.composer_draft.get(), "Fixed in abc");

        click(&mut harness, "composer-send");
        let token = match commands.try_recv() {
            Ok(Command::ReplyToThread { token, .. }) => token,
            other => panic!("Reply did not reply: {other:?}"),
        };
        state.settle_issue_comment(token, Ok(()));
        assert!(
            state.composer.get().is_none(),
            "posted, the composer closes"
        );

        state.open_composer(
            Some(thread.id.clone()),
            None,
            "draft".into(),
            sonnet_medium(),
        );
        state
            .composer
            .update(|c| c.as_mut().unwrap().editing = Some("draft-1".into()));
        let editing = state.composer.get().unwrap();
        assert!(!state.posts_reply(&editing), "an edited draft stays one");
    }

    /// Close with comment sits beside Comment for the pull request's
    /// author, asks first, and sends the text with the close; while it
    /// goes the footer says Closing. Emptied, the box offers the close
    /// alone. It is not offered to a reader who may not close the pull
    /// request, nor on one already closed.
    #[test]
    fn close_with_comment_asks_then_closes_with_the_text() {
        let root = TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("octocat".into(), String::new())));
        let state = state_with_pull_request();
        state.open_composer(None, None, "Superseded by #43".into(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "subject-close"));
        assert!(find(harness.root_id(), "Label: \"Close with comment\"").is_some());

        click(&mut harness, "subject-close");
        assert!(
            commands.try_recv().is_err(),
            "nothing closed before the yes"
        );
        assert!(state.composer_confirming.get());
        assert!(find(
            harness.root_id(),
            "Comment on and close github.com/acme/repo/pull/42?"
        )
        .is_some());

        click(&mut harness, "composer-post");
        let token = match commands.try_recv() {
            Ok(Command::ClosePullRequest {
                item_id,
                token,
                body,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(body, "Superseded by #43");
                token
            }
            other => panic!("Close did not close: {other:?}"),
        };
        let posting = state.posting.get().expect("the close is in flight");
        assert!(posting.closing);
        assert_eq!(posting.token, token);
        assert!(find(harness.root_id(), "Closing").is_some());

        state.settle_issue_comment(token, Ok(()));
        assert!(state.composer.get().is_none(), "closed, the composer goes");

        // Someone else's pull request, with read access only: no close.
        state.open_composer(None, None, String::new(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        assert!(close_offered(app, state));
        app.viewer.set(Some(("reader".into(), String::new())));
        assert!(!close_offered(app, state));
        // Nor on a pull request that is closed already.
        app.viewer.set(Some(("octocat".into(), String::new())));
        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        assert!(!close_offered(app, state));
    }

    /// The composer at the foot of Conversation, with the close menu over
    /// the window as the app draws it, in a window `width` wide.
    fn with_close_menu(
        root: TestRoot,
        app: AppState,
        state: ReviewState,
        backend: Rc<crate::backend::Backend>,
        width: f64,
    ) -> HeadlessHarness {
        app.window_width.set(width);
        app.window_height.set(600.0);
        let composer = state.composer.get().unwrap();
        let view = Stack::vertical((
            composer_view(app, state, backend.clone(), composer, ThreadFace::Card),
            crate::review::close_menu::popup(app, state, backend),
        ))
        .style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, view, width, 600.0);
        harness.rebuild();
        harness.paint();
        harness
    }

    fn reference(number: u64, state: perch_github::SubjectState) -> perch_github::IssueReference {
        perch_github::IssueReference {
            number,
            title: format!("Issue {number}"),
            state,
        }
    }

    /// On an open issue the split button reads Close issue, and its menu
    /// offers completed, not planned and duplicate, as GitHub's does. A
    /// pick changes the face, which asks first and closes with the
    /// reason; the pick outlives a refusal. Duplicate searches the
    /// repository's other issues, and picking one goes straight to the
    /// ask. Until GitHub says who may close it only its author is offered
    /// the close, and nobody is once it is closed.
    #[test]
    fn an_issue_closes_as_completed_not_planned_or_a_duplicate() {
        use perch_github::SubjectState;

        let root = TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("octocat".into(), String::new())));
        let state = state_with_item();
        state.issue.set(Some(Arc::new(issue())));
        state.open_composer(None, None, String::new(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut harness = with_close_menu(root, app, state, Rc::new(backend), 800.0);
        assert!(find(harness.root_id(), "Label: \"Close issue\"").is_some());
        assert!(!shown(&harness, "close-ways"));

        click(&mut harness, "subject-close-menu");
        assert!(shown(&harness, "close-ways"));
        assert!(shown(&harness, "close-as-completed"));
        assert!(
            !shown(&harness, "close-as-issue-unfold"),
            "an issue is not closed as one"
        );
        // Under the button, its right edge on the button's.
        let button = find(harness.root_id(), "subject-close-menu")
            .unwrap()
            .get_visual_rect_no_clip();
        let menu = find(harness.root_id(), "close-ways")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(
            menu.y0 >= button.y1 && (menu.x1 - button.x1).abs() < 2.0,
            "{menu:?} {button:?}"
        );

        click(&mut harness, "close-as-not-planned");
        assert!(!shown(&harness, "close-ways"));
        assert!(commands.try_recv().is_err(), "a pick closes nothing");
        assert!(find(harness.root_id(), "Label: \"Close as not planned\"").is_some());
        click(&mut harness, "subject-close");
        assert!(find(
            harness.root_id(),
            "Close github.com/acme/repo/issues/43 as not planned?"
        )
        .is_some());
        click(&mut harness, "composer-post");
        let token = match commands.try_recv() {
            Ok(Command::CloseIssue {
                item_id,
                token,
                body,
                reason,
            }) => {
                assert_eq!(item_id, "item-1");
                assert_eq!(body, "");
                assert_eq!(reason, IssueCloseReason::NotPlanned);
                token
            }
            other => panic!("Not planned did not close: {other:?}"),
        };
        assert!(find(harness.root_id(), "Closing").is_some());
        state.settle_issue_comment(token, Err("GitHub could not close the issue: 403".into()));
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Label: \"Close as not planned\"").is_some());

        // A duplicate: the search lists the other issues, not this one
        // and not a pull request.
        click(&mut harness, "subject-close-menu");
        click(&mut harness, "close-as-duplicate");
        assert!(shown(&harness, "close-duplicate"));
        state.mentions.install_references(
            String::new(),
            vec![
                reference(43, SubjectState::IssueOpen),
                reference(50, SubjectState::PrOpen),
                reference(12, SubjectState::IssueClosed),
            ],
        );
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "close-duplicate-row:12"));
        assert!(!shown(&harness, "close-duplicate-row:43"));
        assert!(!shown(&harness, "close-duplicate-row:50"));
        click(&mut harness, "close-duplicate-row:12");
        assert!(!shown(&harness, "close-ways"));
        assert!(find(
            harness.root_id(),
            "Close github.com/acme/repo/issues/43 as a duplicate of #12?"
        )
        .is_some());
        click(&mut harness, "composer-post");
        match commands.try_recv() {
            Ok(Command::CloseIssue { reason, .. }) => {
                assert_eq!(reason, IssueCloseReason::Duplicate(12))
            }
            other => panic!("Duplicate did not close: {other:?}"),
        }

        // Someone else's issue waits on GitHub's word.
        app.viewer.set(Some(("reader".into(), String::new())));
        assert!(!close_offered(app, state));
        state.issue_closable.set(Some(true));
        assert!(close_offered(app, state));
        state.issue_closable.set(Some(false));
        assert!(!close_offered(app, state));
        // Nor is a closed issue closed again.
        state.issue_closable.set(Some(true));
        let mut closed = issue();
        closed.state = "closed".into();
        state.issue.set(Some(Arc::new(closed)));
        assert!(!close_offered(app, state));
    }

    /// Escape steps back out of the duplicate search to the ways, then
    /// puts the menu away; a press outside it does too.
    #[test]
    fn escape_and_a_press_outside_put_the_menu_away() {
        let root = TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("octocat".into(), String::new())));
        let state = state_with_item();
        state.issue.set(Some(Arc::new(issue())));
        state.open_composer(None, None, String::new(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let backend = Rc::new(crate::backend::Backend::detached());
        let mut harness = with_close_menu(root, app, state, backend, 800.0);
        click(&mut harness, "subject-close-menu");
        click(&mut harness, "close-as-duplicate");
        assert!(shown(&harness, "close-duplicate"));
        crate::review::close_menu::back(state);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "close-as-completed"));
        crate::review::close_menu::back(state);
        assert!(state.close_menu.get().is_none());
        harness.rebuild();
        harness.paint();

        click(&mut harness, "subject-close-menu");
        assert!(shown(&harness, "close-ways"));
        harness.click(5.0, 590.0);
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "close-ways"));
        assert_eq!(
            state.composer_close_reason.get(),
            IssueCloseReason::Completed
        );
    }

    /// A pull request's menu has Close pull request and Close as issue…,
    /// which unfolds the form without sending anything; no reasons. The
    /// yes under the ask reads Close.
    #[test]
    fn a_pull_requests_menu_closes_it_or_closes_it_as_an_issue() {
        let root = TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("octocat".into(), String::new())));
        let state = state_with_pull_request();
        state.open_composer(None, None, String::new(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut harness = with_close_menu(root, app, state, Rc::new(backend), 800.0);
        click(&mut harness, "subject-close-menu");
        assert!(shown(&harness, "close-pull-request"));
        assert!(shown(&harness, "close-as-issue-unfold"));
        assert!(!shown(&harness, "close-as-not-planned"));
        let item_id = state.item.get().unwrap().id.clone();
        assert!(!app
            .issue_drafts
            .with(|d| d.get(&item_id).is_some_and(|d| d.unfolded)));

        click(&mut harness, "close-as-issue-unfold");
        assert!(commands.try_recv().is_err(), "nothing goes from the menu");
        assert!(!shown(&harness, "close-ways"));
        assert!(app
            .issue_drafts
            .with(|d| d.get(&item_id).is_some_and(|d| d.unfolded)));

        click(&mut harness, "subject-close");
        assert!(find(harness.root_id(), "Label: \"Close\"").is_some());

        // Not for a reader who may not close the pull request.
        app.viewer.set(Some(("reader".into(), String::new())));
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "subject-close"));
    }

    /// With both closes offered, the footer keeps to the card in the
    /// narrowest column the window allows, putting what doesn't fit on a
    /// second line, and to one line where there is room.
    #[test]
    fn the_footer_with_both_closes_keeps_inside_a_narrow_card() {
        for width in [480.0, 800.0] {
            let root = TestRoot::new();
            let app = AppState::new();
            app.viewer.set(Some(("octocat".into(), String::new())));
            let state = state_with_pull_request();
            state.open_composer(None, None, "Superseded".into(), sonnet_medium());
            state.set_destination(Destination::GitHub);
            let composer = state.composer.get().unwrap();
            let backend = Rc::new(crate::backend::Backend::detached());
            let view = composer_view(app, state, backend, composer, ThreadFace::Card);
            let mut harness = HeadlessHarness::new_with_size(root, view, width, 400.0);
            harness.rebuild();
            harness.paint();
            let rects: Vec<_> = ["subject-close", "subject-close-menu", "composer-send"]
                .into_iter()
                .map(|name| {
                    find(harness.root_id(), name)
                        .unwrap()
                        .get_visual_rect_no_clip()
                })
                .collect();
            for rect in &rects {
                assert!(rect.x1 <= width, "past the card at {width}: {rect:?}");
            }
            if width == 800.0 {
                assert!(rects
                    .iter()
                    .all(|r| (r.center().y - rects[0].center().y).abs() < 2.0));
            }
        }
    }

    /// With nothing typed the button reads Close pull request, and the
    /// yes closes with no comment.
    #[test]
    fn an_empty_box_closes_the_pull_request_alone() {
        let root = TestRoot::new();
        let app = AppState::new();
        app.viewer.set(Some(("octocat".into(), String::new())));
        let state = state_with_pull_request();
        state.open_composer(None, None, String::new(), sonnet_medium());
        state.set_destination(Destination::GitHub);
        let composer = state.composer.get().unwrap();
        let (backend, mut commands) = crate::backend::Backend::recording();
        let view = composer_view(app, state, Rc::new(backend), composer, ThreadFace::Card);
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 400.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Label: \"Close pull request\"").is_some());

        click(&mut harness, "subject-close");
        assert!(find(harness.root_id(), "Close github.com/acme/repo/pull/42?").is_some());
        click(&mut harness, "composer-post");
        match commands.try_recv() {
            Ok(Command::ClosePullRequest { body, .. }) => assert_eq!(body, ""),
            other => panic!("Close did not close: {other:?}"),
        }
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
        state
            .composer_error
            .set(Some("GitHub could not post the comment: 403".into()));

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
        assert!(
            !state.posting_from(next),
            "nothing of this composer's is in flight"
        );
        assert!(state.posting.get().is_some());

        state.settle_issue_comment(3, Err("GitHub could not post the comment: 403".into()));
        assert!(state.posting.get().is_none());
        assert!(
            state.composer_error.get().is_none(),
            "the refusal was not this composer's"
        );
        assert_eq!(state.composer_draft.get(), "Another");

        state.start_posting(4, "Another".into());
        state.open_composer(None, None, "A third".into(), sonnet_medium());
        state.settle_issue_comment(4, Ok(()));
        assert!(state.posting.get().is_none());
        assert!(
            state.composer.get().is_some(),
            "the third composer is not the one that posted"
        );
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

        harness.dispatch_event(enter(crate::ui::OS_MOD));
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
            Some(theme::accent().into()),
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
        assert_eq!(
            state.composer_height.get(),
            Some(theme::COMPOSER_HEIGHT + 60.0)
        );
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
        assert_eq!(
            state.composer_height.get(),
            Some(theme::COMPOSER_MIN_HEIGHT)
        );
        harness.pointer_move(x, y + 900.0);
        assert_eq!(
            state.composer_height.get(),
            Some(theme::COMPOSER_MAX_HEIGHT)
        );

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
