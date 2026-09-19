//! The thread column: questions anchored to lines, and the agent's answers.
//!
//! A thread is a comment thread, not a chat: short questions, short
//! answers, tied to lines. The column lists the open file's threads first,
//! then the worktree's others under a divider. A composer sits at the top
//! for a new thread, or under a thread for a follow-up.
//!
//! The same thread is also a card on Conversation. The message, the run's
//! steps and footer, the follow-up line and the composer are one set of
//! views here, taking a [`ThreadFace`] for the sizes that differ, so the
//! two places cannot drift apart.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack, text_editor,
};
use perch_github::{Author, CommentMessage, CommentThread, RunRecord, RunStatus};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{AgentChoice, Composer, ReviewState};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_button, sidebar_glyph, text_button};

pub fn thread_column(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    Stack::vertical((
        header(app, state),
        body(app, state, backend, open_file),
    ))
    .style(move |s| {
        s.width(state.threads_width.get())
            .height_full()
            .flex_shrink(0.0_f32)
            .flex_col()
            .background(theme::CANVAS_SIDEBAR)
            .border_left(1.0)
            .border_color(theme::BORDER)
    })
}

fn header(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        Label::derived(|| "Threads".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .selectable(false)
                .flex_grow(1.0_f32)
        }),
        // A question about the pull request as a whole.
        icon_button(|| SidebarIcon::Plus, || theme::FG_MUTED, move || {
            state.open_composer(None, None, String::new(), defaults(app));
        }),
        icon_button(|| SidebarIcon::Close, || theme::FG_MUTED, move || {
            state.threads_open.set(false);
        }),
    ))
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_left(14.0)
            .padding_right(8.0)
            .gap(2.0)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

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
    let offered = agent.models.iter().chain(agent.codex_models()).any(|m| *m == agent.model);
    let model = if offered {
        agent.model.clone()
    } else {
        model_groups(agent).into_iter().flatten().next().map(|(_, m)| m).unwrap_or_else(|| agent.model.clone())
    };
    (crate::ask::backend_for(agent, &model), model.clone(), nearest_effort(agent, &model, effort))
}

/// Where a thread is drawn: in the column beside the editor, or as a card
/// on Conversation. The views are the same; the face picks the room they
/// take and the surface they sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadFace {
    Column,
    Card,
}

impl ThreadFace {
    /// Room between a thread's messages.
    fn message_gap(self) -> f64 {
        match self {
            ThreadFace::Column => 10.0,
            ThreadFace::Card => theme::CARD_GAP,
        }
    }
}

fn body(app: AppState, state: ReviewState, backend: Rc<Backend>, open_file: OpenFile) -> impl IntoView {
    let composer_backend = backend.clone();
    let here_backend = backend.clone();
    let here_open = open_file.clone();

    Stack::vertical((
        // A composer for a new thread sits at the top. A follow-up's sits
        // under its thread instead.
        dyn_container(
            move || state.composer.with(|c| c.as_ref().filter(|c| c.thread_id.is_none()).cloned()),
            move |composer| match composer {
                Some(c) => composer_view(app, state, composer_backend.clone(), c, ThreadFace::Column).into_any(),
                None => Empty::new().into_any(),
            },
        )
        .style(|s| s.width_full()),
        dyn_stack(
            move || {
                let path = state.active.get();
                let here = path.as_deref().map(|p| state.threads_in(p)).unwrap_or_default();
                let github = path.as_deref().map(|p| state.github_threads_in(p)).unwrap_or_default();
                let elsewhere = state.threads_elsewhere(path.as_deref());
                let mut rows: Vec<Row> = here.into_iter().map(Row::Thread).collect();
                if !github.is_empty() {
                    rows.push(Row::GitHubDivider);
                    rows.extend(github.into_iter().map(Row::GitHub));
                }
                if !elsewhere.is_empty() {
                    rows.push(Row::Divider);
                    rows.extend(elsewhere.into_iter().map(Row::Thread));
                }
                rows
            },
            |row| match row {
                Row::Thread(t) => t.id.clone(),
                Row::Divider => "divider".to_string(),
                Row::GitHubDivider => "github".to_string(),
                Row::GitHub(t) => format!("github:{}:{}", t.comment.id, t.replies.len()),
            },
            move |row| match row {
                Row::Thread(t) => thread_card(app, state, here_backend.clone(), here_open.clone(), t).into_any(),
                Row::Divider => group_heading("Elsewhere in this worktree").into_any(),
                Row::GitHubDivider => group_heading("On GitHub").into_any(),
                Row::GitHub(t) => {
                    crate::review::cards::line_thread_block(app, state, here_backend.clone(), here_open.clone(), t).into_any()
                }
            },
        )
        .style(|s| s.flex_col().width_full().gap(10.0).padding(12.0)),
        dyn_container(
            move || {
                let github = state.active.with(|p| p.as_deref().map(|p| !state.github_threads_in(p).is_empty()).unwrap_or(false));
                state.comment_threads.with(|t| t.is_empty()) && state.composer.with(Option::is_none) && !github
            },
            |empty| {
                if empty {
                    Label::derived(|| {
                        "Select lines and press a to ask about them, or + to ask about the pull request."
                            .to_string()
                    })
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::FG_SUBTLE)
                            .padding(20.0)
                            .width_full()
                            .text_wrap()
                    })
                    .into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        ),
    ))
    .style(|s| s.flex_col().width_full())
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

#[derive(Clone)]
enum Row {
    Thread(CommentThread),
    Divider,
    /// The heading over GitHub's review comments on the open file, which
    /// are not threads of the reader's own and are drawn as the nested
    /// blocks Conversation shows.
    GitHubDivider,
    GitHub(crate::review::timeline::LineThread),
}

// ---------------------------------------------------------------------
// Composer
// ---------------------------------------------------------------------

/// The box a question is typed into, with where it is anchored above and
/// the model, the effort and Send below. In the column it sits in a
/// margin of its own; as a card it fills its place.
pub fn composer_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    composer: Composer,
    face: ThreadFace,
) -> impl IntoView {
    use floem::views::editor::command::{Command, CommandExecuted};
    use floem::views::editor::core::command::EditCommand;
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
        .and_then(|id| state.comment_threads.with(|t| t.iter().find(|t| &t.id == id).map(|t| t.outdated)))
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
        .editor_style(|s| s.hide_gutter(true));
    // Markdown as it is typed: bold, italics, code and links take their
    // styles in the box, the way they will in the thread.
    let styling_doc = editor.editor().doc();
    let styling_id = state.next_token();
    let editor = editor.styling(crate::review::composer_styling::ComposerStyling::new(styling_doc, styling_id));
    let doc_editor = editor.editor().clone();
    let read_text = Rc::new(move || doc_editor.doc().text().to_string());
    // A draft the box starts with, a retried question or a quoted comment,
    // is typed under, not into.
    if !draft.is_empty() {
        let end = editor.editor().doc().text().len();
        editor
            .editor()
            .cursor
            .set(Cursor::new(CursorMode::Insert(Selection::caret(end, CursorAffinity::Backward)), None, None));
    }

    let send: Rc<dyn Fn()> = {
        let read_text = read_text.clone();
        let backend = backend.clone();
        Rc::new(move || {
            let question = read_text();
            if question.trim().is_empty() {
                return;
            }
            let Some(composer) = state.composer.get() else { return };
            let Some(item_id) = state.item.with(|i| i.as_ref().map(|i| i.id.clone())) else {
                return;
            };
            backend.ask(
                &item_id,
                composer.thread_id.clone(),
                composer.anchor.clone(),
                question.trim(),
                composer.backend,
                &composer.model,
                composer.effort,
            );
            state.agent_last.set(Some((composer.backend, composer.model.clone(), composer.effort)));
            state.close_composer();
        })
    };
    let key_send = send.clone();
    let escape_text = read_text.clone();

    let editor = editor
        // Cmd-Enter sends. The editor would otherwise take Enter for a new
        // line whatever the modifiers.
        .pre_command(move |cmd| {
            if matches!(cmd.cmd, Command::Edit(EditCommand::InsertNewLine)) && cmd.mods.contains(Modifiers::META) {
                key_send();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        // The draft, for Escape and for the root key handler, neither of
        // which can reach into the document.
        .update(move |update| {
            if let Some(editor) = update.editor {
                state.composer_draft.set(editor.doc().text().to_string());
            }
        })
        .on_event_cont(el::FocusGained, move |_, _| state.composer_focused.set(true))
        .on_event_cont(el::FocusLost, move |_, _| state.composer_focused.set(false))
        // Escape closes an empty composer. In the capture phase, because
        // the editor stops every key once it has focus and this has to
        // run first.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig { phases: floem::context::Phases::CAPTURE },
            move |_, key: &KeyboardEvent| {
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    if escape_text().trim().is_empty() {
                        state.close_composer();
                    }
                    return floem::event::EventPropagation::Stop;
                }
                floem::event::EventPropagation::Continue
            },
        )
        .style(|s| {
            s.width_full()
                .min_height(72.0)
                .max_height(220.0)
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .padding(8.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::ACCENT))
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
    // `a` can find it, and scrolls the column to itself: it sits at the end
    // of the column, or under a thread, and may have opened off screen.
    let inner = editor.editor().editor_view_id;
    let asked = std::rc::Rc::new(std::cell::Cell::new(false));
    let registered = std::rc::Rc::new(std::cell::Cell::new(None));
    let unregister = registered.clone();
    let editor = editor
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if asked.replace(true) {
                return;
            }
            let Some(id) = inner.get_untracked() else { return };
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
            if let Some(id) = unregister.get() {
                if state.composer_box.get_untracked() == Some(id) {
                    state.composer_box.set(None);
                }
            }
        });

    Stack::vertical((
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Thread, theme::ACCENT).style(|s| s.size(13.0, 13.0).margin_right(6.0)),
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
            // Put the box away. Nothing typed into it is kept.
            icon_button(|| SidebarIcon::Close, || theme::FG_SUBTLE, move || state.close_composer())
                .style(|s| s.size(22.0, 22.0).margin_right(-4.0)),
        ))
        .style(|s| s.items_center().width_full().margin_bottom(8.0)),
        editor,
        Stack::horizontal((
            model_picker(app, state),
            effort_picker(app, state),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            text_button(|| "Send".to_string(), SidebarIcon::ChevronRight, move || send()),
        ))
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0)),
    ))
    .style(move |s| {
        let base = s
            .flex_col()
            .width_full()
            .padding(12.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::ACCENT_RING)
            .border_radius(theme::RADIUS_LG);
        match face {
            ThreadFace::Column => base.margin(12.0).margin_bottom(0.0),
            ThreadFace::Card => base,
        }
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

/// The model to run with, from what config offers: Claude Code's models,
/// then Codex's under a line when there are any. A model carries its
/// backend, so picking one sets both.
fn model_picker(app: AppState, state: ReviewState) -> impl IntoView {
    use floem::menu::Menu;

    let current = move || state.composer.with(|c| c.as_ref().map(|c| model_label(&c.model)).unwrap_or_default());
    menu_button(current, move || {
        let selected = state.composer.get_untracked().map(|c| c.model).unwrap_or_default();
        let mut menu = Menu::new();
        for (i, group) in model_groups(&app.agent.get_untracked()).into_iter().enumerate() {
            if i > 0 {
                menu = menu.separator();
            }
            for (backend, model) in group {
                let label = model_label(&model);
                let pick = model.clone();
                let checked = model == selected;
                menu = menu.item(label, move |item| {
                    let pick = pick.clone();
                    item.checked(checked).action(move || {
                        let agent = app.agent.get_untracked();
                        state.composer.update(|c| {
                            if let Some(c) = c {
                                c.backend = backend;
                                c.model = pick.clone();
                                c.effort = nearest_effort(&agent, &pick, c.effort);
                            }
                        })
                    })
                });
            }
        }
        menu
    })
}

/// The picker's sections: one per backend that has models, Claude Code
/// first. Each entry names the backend the model runs on, which is
/// config's call, not the section's, so a model listed under both stays
/// with Claude Code wherever it appears.
fn model_groups(agent: &perch_github::AgentConfig) -> Vec<Vec<(perch_agent::Backend, String)>> {
    let entry = |m: &String| (crate::ask::backend_for(agent, m), m.clone());
    let claude: Vec<_> = agent.models.iter().map(entry).collect();
    let codex: Vec<_> = agent.codex_models().iter().map(entry).collect();
    [claude, codex].into_iter().filter(|g| !g.is_empty()).collect()
}

/// The effort a model keeps when the one chosen is not among those it
/// takes: the highest it does take, so a reader who asked for Ultra of a
/// model without it gets Max and sees so, rather than Codex quietly
/// running Max under the name Ultra.
fn nearest_effort(agent: &perch_github::AgentConfig, model: &str, wanted: perch_agent::Effort) -> perch_agent::Effort {
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

    let current = move || state.composer.with(|c| c.as_ref().map(|c| c.effort.label().to_string()).unwrap_or_default());
    menu_button(current, move || {
        let (selected, model) =
            state.composer.get_untracked().map(|c| (c.effort, c.model)).unwrap_or_default();
        let agent = app.agent.get_untracked();
        let mut menu = Menu::new();
        for effort in perch_agent::Effort::ALL {
            let takes = agent.effort_supported(&model, effort.name());
            menu = menu.item(effort.label(), move |item| {
                item.checked(effort == selected).enabled(takes).action(move || {
                    state.composer.update(|c| {
                        if let Some(c) = c {
                            c.effort = effort;
                        }
                    })
                })
            });
        }
        menu
    })
}

/// A small pop-up button, like the toolbar's, sized for the composer.
fn menu_button(label: impl Fn() -> String + 'static, menu: impl Fn() -> floem::menu::Menu + 'static) -> impl IntoView {
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
            .hover(|s| s.background(theme::CANVAS_SUBTLE).cursor(CursorStyle::Pointer))
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
                .font_weight(FontWeight::MEDIUM)
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

fn thread_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    thread: CommentThread,
) -> impl IntoView {
    let id = thread.id.clone();
    let flash_id = thread.id.clone();
    let flashing = RwSignal::new(false);

    let card = Stack::vertical((
        anchor_line(state, open_file, thread.clone()),
        delete_confirmation(state, backend.clone(), thread.id.clone()),
        messages_view(app, state, backend.clone(), thread.id.clone(), ThreadFace::Column),
        follow_up(app, state, backend, thread.id.clone(), ThreadFace::Column),
    ))
    .style(move |s| {
        let base = s
            .flex_col()
            .width_full()
            .padding(12.0)
            .gap(10.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_radius(theme::RADIUS_LG)
            .box_shadow_blur(4.0)
            .box_shadow_v_offset(1.0)
            .box_shadow_color(theme::SHADOW);
        if flashing.get() {
            base.border_color(theme::ACCENT)
        } else {
            base.border_color(theme::BORDER)
        }
    });

    // A gutter marker was clicked: scroll here and flash the border.
    let card_id = card.view_id();
    Effect::new(move |_| {
        if state.focus_thread.get().as_deref() == Some(flash_id.as_str()) {
            state.focus_thread.set(None);
            state.show_threads();
            card_id.scroll_to(None);
            flashing.set(true);
            floem::action::exec_after(std::time::Duration::from_millis(900), move |_| {
                flashing.set(false);
            });
        }
    });
    let _ = id;
    card
}

/// `src/x.rs:41–58`, clickable, with Outdated and the `...` menu.
fn anchor_line(state: ReviewState, open_file: OpenFile, thread: CommentThread) -> impl IntoView {
    let label = thread.anchor.as_ref().map(|a| a.label()).unwrap_or_else(|| "Worktree".to_string());
    let anchor = thread.anchor.clone();
    let delete_id = thread.id.clone();
    let has_anchor = anchor.is_some();

    Stack::horizontal((
        sidebar_glyph(
            if has_anchor { SidebarIcon::Thread } else { SidebarIcon::Branch },
            if thread.outdated { theme::FG_SUBTLE } else { theme::ACCENT },
        )
        .style(|s| s.size(13.0, 13.0).margin_right(6.0).flex_shrink(0.0_f32)),
        Label::derived(move || label.clone())
            .on_event_stop(el::Click, move |_, _| {
                if let Some(a) = &anchor {
                    open_file(&a.path);
                    state.jump_to_line.set(Some(a.start_line.saturating_sub(1) as usize));
                }
            })
            .style(move |s| {
                let base = s
                    .font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::FG_MUTED)
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false);
                if has_anchor {
                    base.hover(|s| s.color(theme::ACCENT).cursor(CursorStyle::Pointer))
                } else {
                    base
                }
            }),
        outdated_pill(thread.outdated),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        thread_menu(state, delete_id),
    ))
    .style(|s| s.items_center().width_full())
}

/// The `...` at the end of a thread's first row, and the menu under it.
/// A small heading over a group of the column's rows.
fn group_heading(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_string()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(theme::FG_SUBTLE)
            .padding_horiz(14.0)
            .padding_top(14.0)
            .padding_bottom(4.0)
            .selectable(false)
    })
}

/// "Delete this thread?" with Delete and Cancel, under the thread that
/// asked, on a card or in the column alike. Nothing else deletes.
pub fn delete_confirmation(state: ReviewState, backend: Rc<Backend>, thread_id: String) -> impl IntoView {
    let asking = Memo::new(move |_| state.confirm_thread.with(|c| c.as_deref() == Some(thread_id.as_str())));
    dyn_container(
        move || asking.get(),
        move |asking| {
            if !asking {
                return Empty::new().into_any();
            }
            let backend = backend.clone();
            Stack::vertical((
                Label::derived(|| "Delete this thread?".to_string()).style(|s| {
                    s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG).selectable(false)
                }),
                Label::derived(|| "The question, its answers and the anchor go. Nothing on GitHub is touched.".to_string())
                    .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED).text_wrap().width_full().margin_top(2.0)),
                Stack::horizontal((
                    text_button(|| "Delete".to_string(), SidebarIcon::Trash, move || {
                        if let Some(id) = state.confirm_thread.get_untracked() {
                            backend.delete_comment_thread(&id);
                        }
                        state.confirm_thread.set(None);
                    }),
                    text_button(|| "Cancel".to_string(), SidebarIcon::Close, move || state.confirm_thread.set(None)),
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
    .style(|s| s.width_full())
}

pub fn thread_menu(state: ReviewState, thread_id: String) -> impl IntoView {
    use floem::menu::Menu;

    Container::new(
        sidebar_glyph(SidebarIcon::Ellipsis, theme::FG_SUBTLE).style(|s| s.pointer_events_none()),
    )
    .popout_menu(move || {
        let id = thread_id.clone();
        // Delete asks first, under the byline, wherever the thread is
        // showing; the confirmation is the one thing that deletes.
        Menu::new().item("Delete Thread\u{2026}", move |item| {
            let id = id.clone();
            item.action(move || state.confirm_thread.set(Some(id.clone())))
        })
    })
    .style(|s| {
        clickable(s)
            .size(22.0, 22.0)
            .items_center()
            .justify_center()
            .border_radius(11.0)
            .hover(|s| s.background(theme::BADGE_BG))
    })
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
    dyn_stack(
        move || {
            state
                .comment_threads
                .with(|t| t.iter().find(|t| t.id == list_id).map(|t| t.messages.clone()).unwrap_or_default())
        },
        |m| (m.id.clone(), m.body.len(), m.run.as_ref().map(|r| (r.status.name(), r.steps.len(), r.error.clone()))),
        move |m| {
            let thread = state.comment_threads.with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
            match thread {
                Some(thread) => message_view(app, state, backend.clone(), thread, m, face).into_any(),
                None => Empty::new().into_any(),
            }
        },
    )
    .style(move |s| s.flex_col().width_full().gap(face.message_gap()))
}

/// The follow-up composer under a thread when one is open for it, else the
/// line that opens one.
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
        state
            .comment_threads
            .with(|t| t.iter().find(|t| t.id == running_thread).is_some_and(|t| t.running_run().is_some()))
    });
    dyn_container(
        move || {
            let composer = state
                .composer
                .with(|c| c.as_ref().filter(|c| c.thread_id.as_deref() == Some(composer_thread.as_str())).cloned());
            (composer, running.get())
        },
        move |(composer, _)| match composer {
            Some(c) => composer_view(app, state, backend.clone(), c, face).into_any(),
            None => {
                let thread = state.comment_threads.with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
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
/// produced it, or under a question the run that is answering it.
pub fn message_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    message: CommentMessage,
    face: ThreadFace,
) -> impl IntoView {
    let is_agent = matches!(message.author, Author::Agent { .. });
    let body = message.body.clone();

    // Questions and answers are both markdown; the reader writes it too.
    // A finished run sits on the answer, and reads in the order it went:
    // what the agent did, then what it said, then what it cost. While it
    // runs, or when it failed, the run sits under the question whole.
    let finished = message.run.as_ref().is_some_and(|r| r.status == RunStatus::Finished);
    match message.run.clone() {
        Some(run) if is_agent && finished => Stack::vertical((
            run_surface(steps_view(state, run.clone()), face),
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
    }
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
    run_surface(
        Stack::vertical((
            steps_view(state, run.clone()),
            run_footer(app, state, backend, thread, question, run, face),
        ))
        .style(|s| s.flex_col().width_full().gap(6.0)),
        face,
    )
}

/// The box a run's lines sit in. The column's cards are white, so the run
/// sits on the page grey; a thread card is the page grey, so the run sits
/// on white.
fn run_surface(inner: impl IntoView + 'static, face: ThreadFace) -> impl IntoView {
    Container::new(inner).style(move |s| {
        let base = s.width_full().padding(8.0).border_radius(theme::RADIUS);
        match face {
            ThreadFace::Column => base.background(theme::CANVAS_SUBTLE),
            ThreadFace::Card => base.background(theme::CANVAS).border(1.0).border_color(theme::BORDER_MUTED),
        }
    })
}

/// The footer under a run: Stop while it goes, cost and time once it is
/// done, the error and Retry when it is not.
fn run_footer(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
    question: String,
    run: RunRecord,
    face: ThreadFace,
) -> floem::AnyView {
    let cancel_id = run.id.clone();
    match run.status {
        RunStatus::Running => text_button(|| "Stop".to_string(), SidebarIcon::Stop, move || {
            backend.cancel_run(&cancel_id)
        })
        .into_any(),
        RunStatus::Finished => Label::derived(move || {
            let secs = run.duration_ms.unwrap_or(0) as f64 / 1000.0;
            let cost = run.cost_usd.map(|c| format!(" \u{00b7} ${c:.2}")).unwrap_or_default();
            let effort = perch_agent::Effort::parse(&run.effort).map(|e| e.label().to_string()).unwrap_or_else(|| run.effort.clone());
            // A card's byline already names the model and the effort.
            match face {
                ThreadFace::Column => format!("{} \u{00b7} {effort} \u{00b7} {secs:.0}s{cost}", model_label(&run.model)),
                ThreadFace::Card => format!("{secs:.0}s{cost}"),
            }
        })
        .style(|s| s.font_size(theme::TEXT_SECTION).color(theme::FG_SUBTLE).selectable(false))
        .into_any(),
        RunStatus::Stopped | RunStatus::Failed => {
            let error = run.error.clone().unwrap_or_else(|| "Failed".to_string());
            let thread_id = thread.id.clone();
            let anchor = thread.anchor.clone();
            let is_first = thread.messages.first().is_some_and(|m| m.run.as_ref().is_some_and(|r| r.id == run.id) || m.body == question);
            Stack::horizontal((
                Label::derived(move || error.clone()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::CLOSED)
                        .text_wrap()
                        .min_width(0.0)
                        .flex_grow(1.0_f32)
                }),
                text_button(|| "Retry".to_string(), SidebarIcon::Refresh, move || {
                    // Retry a thread's first question fresh; a follow-up in
                    // its thread.
                    let thread_id = if is_first && thread.messages.len() <= 1 { None } else { Some(thread_id.clone()) };
                    state.open_composer(thread_id, anchor.clone(), question.clone(), defaults(app));
                }),
            ))
            .style(|s| s.items_center().width_full().gap(8.0))
            .into_any()
        }
    }
}

/// The step lines, folded to "Read 4 files, ran 2 commands" until the
/// line is clicked, and open while the run goes.
fn steps_view(state: ReviewState, run: RunRecord) -> impl IntoView {
    let running = run.status == RunStatus::Running;
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
                    if expanded_chevron() { base.rotate(90.0) } else { base }
                }),
                Label::derived(move || {
                    if running && steps.is_empty() { "Working…".to_string() } else { summary.clone() }
                })
                .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED).selectable(false)),
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
                            .color(if step.ok { theme::FG_MUTED } else { theme::CLOSED })
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
    let count = |prefix: &str| steps.iter().filter(|s| s.summary.starts_with(prefix)).count();
    let mut parts = Vec::new();
    let reads = count("Read ");
    let runs = count("Ran ");
    let searches = count("Searched ") + count("Listed ");
    let denied = steps.iter().filter(|s| !s.ok).count();
    if reads > 0 {
        parts.push(format!("read {reads} {}", if reads == 1 { "file" } else { "files" }));
    }
    if runs > 0 {
        parts.push(format!("ran {runs} {}", if runs == 1 { "command" } else { "commands" }));
    }
    if searches > 0 {
        parts.push(format!("{searches} {}", if searches == 1 { "search" } else { "searches" }));
    }
    if denied > 0 {
        parts.push(format!("{denied} not allowed"));
    }
    if parts.is_empty() {
        return if steps.is_empty() { "No steps".to_string() } else { format!("{} steps", steps.len()) };
    }
    let mut text = parts.join(", ");
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    text
}

/// The one-line invitation at the bottom of a thread.
pub fn reply_line(app: AppState, state: ReviewState, thread: CommentThread, face: ThreadFace) -> impl IntoView {
    let running = thread.running_run().is_some();
    if running {
        return Empty::new().into_any();
    }
    let id = thread.id.clone();
    let anchor = thread.anchor.clone();
    Label::derived(|| "Follow up…".to_string())
        .on_event_stop(el::Click, move |_, _| {
            state.open_composer(Some(id.clone()), anchor.clone(), String::new(), defaults(app));
        })
        .style(move |s| {
            let base = clickable(s)
                .font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .width_full()
                .padding_vert(4.0)
                .hover(|s| s.color(theme::ACCENT));
            // On a card the line stands where the composer's box will be,
            // and takes the box's shape.
            match face {
                ThreadFace::Column => base,
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

/// The strip along the column's left edge that drags it wider.
pub fn resize_handle(state: ReviewState) -> impl IntoView {
    use floem::event::DragConfig;
    use floem::style::Position;

    let grab = RwSignal::new(0.0_f64);

    Empty::new()
        .on_event_stop(el::PointerDown, move |cx, e: &PointerButtonEvent| {
            grab.set(state.threads_width.get());
            if let Some(pointer) = e.pointer.pointer_id {
                if pointer.is_primary_pointer() {
                    cx.request_pointer_capture(pointer);
                }
            }
        })
        .on_event_stop(el::GainedPointerCapture, |cx, token| {
            cx.start_drag(
                *token,
                DragConfig {
                    threshold: 1.0,
                    animation_duration: std::time::Duration::ZERO,
                    easing: std::rc::Rc::new(floem::easing::Linear),
                    custom_data: None,
                    track_targets: false,
                },
                false,
            );
        })
        .on_event_stop(el::DragMove, move |_, e: &floem::event::DragMoveEvent| {
            // The column's left edge moves with the pointer, so its width
            // is what it was at the grab plus how far left the pointer went.
            let dx = e.start_state.logical_point().x - e.current_state.logical_point().x;
            let width = (grab.get() + dx).clamp(theme::THREADS_MIN_WIDTH, theme::THREADS_MAX_WIDTH);
            state.threads_width.set(width);
        })
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(0.0)
                .inset_bottom(0.0)
                .inset_left(-3.0)
                .width(7.0)
                .z_index(10)
                .hover(|s| s.cursor(CursorStyle::ColResize))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::RunStep;

    fn step(summary: &str, ok: bool) -> RunStep {
        RunStep { summary: summary.into(), ok }
    }

    #[test]
    fn steps_are_summarised_by_kind() {
        let steps = vec![
            step("Read a.rs", true),
            step("Read b.rs", true),
            step("Ran git log", true),
            step("Asked to edit a.rs: not allowed", false),
        ];
        assert_eq!(steps_summary(&steps), "Read 2 files, ran 1 command, 1 not allowed");
        assert_eq!(steps_summary(&[]), "No steps");
        assert_eq!(steps_summary(&[step("Read a.rs", true)]), "Read 1 file");
    }

    #[test]
    fn aliases_are_capitalised_and_ids_left_alone() {
        assert_eq!(model_label("sonnet"), "Sonnet");
        assert_eq!(model_label("fable"), "Fable");
        assert_eq!(model_label("claude-sonnet-5"), "claude-sonnet-5");
    }

    #[test]
    fn a_fresh_composer_starts_on_a_model_that_is_offered() {
        use perch_agent::{Backend, Effort};
        let mut agent = perch_github::AgentConfig { model: "sonnet".into(), effort: "ultra".into(), ..Default::default() };
        assert_eq!(default_choice(&agent), (Backend::ClaudeCode, "sonnet".to_string(), Effort::Ultra));

        // Claude Code is not installed: its aliases are gone, and Codex is.
        agent.models.clear();
        agent.codex.models = vec!["gpt-5.5".into(), "gpt-6-astra".into()];
        agent.codex.efforts.insert("gpt-5.5".into(), ["low", "xhigh"].map(String::from).to_vec());
        assert_eq!(default_choice(&agent), (Backend::Codex, "gpt-5.5".to_string(), Effort::XHigh));

        // Nothing at all: config's choice stands, and the run will say so.
        agent.codex.models.clear();
        assert_eq!(default_choice(&agent), (Backend::ClaudeCode, "sonnet".to_string(), Effort::Ultra));
    }

    #[test]
    fn picking_a_model_without_the_effort_drops_to_the_highest_it_takes() {
        use perch_agent::Effort;
        let mut agent = perch_github::AgentConfig::default();
        agent.codex.efforts.insert("gpt-5.5".into(), ["low", "medium", "high", "xhigh"].map(String::from).to_vec());
        agent.codex.efforts.insert("odd".into(), vec!["medium".into()]);
        assert_eq!(nearest_effort(&agent, "gpt-5.5", Effort::Ultra), Effort::XHigh);
        assert_eq!(nearest_effort(&agent, "gpt-5.5", Effort::High), Effort::High, "a level it takes is kept");
        assert_eq!(nearest_effort(&agent, "odd", Effort::Low), Effort::Medium, "even upward, to the one it has");
        assert_eq!(nearest_effort(&agent, "sonnet", Effort::Ultra), Effort::Ultra, "no catalog, no doubt");
    }

    #[test]
    fn the_picker_lists_claude_then_codex_and_each_model_knows_its_backend() {
        use perch_agent::Backend;
        let agent = perch_github::AgentConfig {
            models: vec!["opus".into(), "sonnet".into()],
            codex: perch_github::CodexConfig { binary: "codex".into(), models: vec!["gpt-5.6-sol".into()], ..Default::default() },
            ..perch_github::AgentConfig::default()
        };
        assert_eq!(
            model_groups(&agent),
            vec![
                vec![(Backend::ClaudeCode, "opus".to_string()), (Backend::ClaudeCode, "sonnet".to_string())],
                vec![(Backend::Codex, "gpt-5.6-sol".to_string())],
            ]
        );

        let claude_only = perch_github::AgentConfig { models: vec!["sonnet".into()], ..perch_github::AgentConfig::default() };
        assert_eq!(model_groups(&claude_only), vec![vec![(Backend::ClaudeCode, "sonnet".to_string())]], "no separator without Codex");

        let shared = perch_github::AgentConfig {
            models: vec!["shared".into()],
            codex: perch_github::CodexConfig { binary: "codex".into(), models: vec!["shared".into()], ..Default::default() },
            ..perch_github::AgentConfig::default()
        };
        assert_eq!(model_groups(&shared)[1], vec![(Backend::ClaudeCode, "shared".to_string())], "config decides, not the section");
    }

}

/// The column in a headless window, driven by clicks.
#[cfg(test)]
mod column {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::{Code, KeyboardEvent, Modifiers};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::{
        Anchor, Author, CommentMessage, CommentThread, Item, ItemCheckout, ItemSubject, PullRequest,
        RunRecord, RunStatus, RunStep, Side, ThreadOrigin,
    };

    use crate::review::model::ReviewState;
    use crate::state::AppState;

    fn run(status: RunStatus) -> RunRecord {
        RunRecord {
            id: "r1".into(),
            thread_id: "t1".into(),
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "medium".into(),
            session_id: Some("sess".into()),
            status,
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            steps: vec![
                RunStep { summary: "Read src/x.rs".into(), ok: true },
                RunStep { summary: "Asked to edit src/x.rs".into(), ok: false },
            ],
            error: None,
            started_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            ended_at: Some("2026-09-18T10:00:14Z".parse().unwrap()),
        }
    }

    /// A question and its answer, both finished.
    fn thread() -> CommentThread {
        let created_at = "2026-09-18T09:59:00Z".parse().unwrap();
        let message = |id: &str, author: Author, body: &str| CommentMessage {
            id: id.into(),
            thread_id: "t1".into(),
            origin: ThreadOrigin::Local,
            author,
            body: body.into(),
            created_at,
            run: Some(run(RunStatus::Finished)),
        };
        CommentThread {
            id: "t1".into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: 41,
                line: 58,
                side: Side::Right,
                commit_sha: "abc".into(),
            }),
            outdated: false,
            resolved: false,
            created_at,
            messages: vec![
                message("m1", Author::User, "why?"),
                message(
                    "m2",
                    Author::Agent { backend: "claude-code".into(), model: "sonnet".into() },
                    "because\n\n```rust\nfn x() {}\n```\n\n- one\n- two",
                ),
            ],
        }
    }

    /// The whole window with a pull request open and the column showing
    /// one thread. The whole window rather than the column alone, because
    /// the column sits in a `dyn_container` of its own, and what that
    /// container does on a signal write is part of what is being tested.
    fn app_harness() -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let pr = PullRequest {
            number: 812,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "f".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        };
        let now = "2026-09-18T09:59:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(crate::review::model::Phase::Ready);
        // These tests drive the thread column beside the editor, on Files.
        state.tab.set(crate::review::model::ItemTab::Files);
        state.pull_request.set(Some(Arc::new(pr)));
        state.comment_threads.set(Arc::new(vec![thread()]));
        state.threads_open.set(true);
        // See the field: a focused editor cannot be driven headlessly.
        state.composer_autofocus.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    /// Where the header's close button sits: the last child of a header
    /// that is `TAB_HEIGHT` tall, `padding_right` 8, button 26 square, in
    /// a column `THREADS_WIDTH` wide against the window's right edge under
    /// the review header and the item view's tab strip.
    fn close_button_center() -> (f64, f64) {
        let x = 1400.0 - 8.0 - 13.0;
        let y = crate::theme::REVIEW_HEADER_HEIGHT + crate::theme::TAB_HEIGHT + crate::theme::TAB_HEIGHT / 2.0;
        (x, y)
    }

    #[test]
    fn the_close_button_hides_the_column() {
        let (mut harness, state) = app_harness();
        assert!(state.threads_open.get());
        let (x, y) = close_button_center();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.threads_open.get(), "click at ({x}, {y}) over {under:?} did not close the column");
    }

    #[test]
    fn the_close_button_hides_the_column_with_a_composer_open() {
        let (mut harness, state) = app_harness();
        // The state in the report: an unanchored composer, its box focused.
        state.comment_threads.set(Arc::new(vec![]));
        state.open_composer(None, None, String::new(), (perch_agent::Backend::ClaudeCode, "sonnet".into(), perch_agent::Effort::Medium));
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_some());
        let (x, y) = close_button_center();
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.threads_open.get(), "click at ({x}, {y}) over {under:?} did not close the column");
    }

    #[test]
    fn the_close_button_works_at_retina_scale_after_a_hover() {
        let (mut harness, state) = app_harness();
        harness.set_scale(2.0);
        harness.rebuild();
        harness.paint();
        let (x, y) = close_button_center();
        harness.pointer_move(x, y);
        harness.rebuild();
        harness.paint();
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.threads_open.get(), "did not close at 2x");
    }

    #[test]
    fn the_close_button_hides_the_column_when_the_open_file_has_threads() {
        let (mut harness, state) = app_harness();
        // The thread is anchored in src/x.rs; make that the open file, as
        // the effect that opens the column for a file with threads sees it.
        let file = Arc::new(crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "fn a() {}\n".repeat(80),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));
        // One rebuild per write, as the window gets: the tab opens on a
        // click and the file lands from the backend a frame later.
        state.open_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        state.finish_load("src/x.rs", file);
        harness.rebuild();
        harness.paint();
        assert!(state.threads_open.get());
        let (x, y) = close_button_center();
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(!state.threads_open.get(), "the column reopened itself");
    }

    fn escape() -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: floem::prelude::KeyState::Down,
            key: floem::prelude::Key::Named(floem::prelude::NamedKey::Escape),
            code: Code::Escape,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    /// With the box unfocused, as it is here, Escape reaches the root
    /// handler. Focused, the composer's own capture-phase listener does the
    /// same thing before the editor can swallow the key; that path needs a
    /// real window.
    #[test]
    fn escape_in_an_empty_box_closes_the_composer() {
        let (mut harness, state) = app_harness();
        state.comment_threads.set(Arc::new(vec![]));
        state.open_composer(None, None, String::new(), (perch_agent::Backend::ClaudeCode, "sonnet".into(), perch_agent::Effort::Medium));
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none(), "escape did not close the empty composer");
        assert!(state.threads_open.get(), "the column stays");
    }

    #[test]
    fn escape_keeps_a_composer_with_a_question_in_it() {
        let (mut harness, state) = app_harness();
        state.comment_threads.set(Arc::new(vec![]));
        state.open_composer(None, None, "why is this here?".into(), (perch_agent::Backend::ClaudeCode, "sonnet".into(), perch_agent::Effort::Medium));
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_some(), "escape threw the question away");
    }

    #[test]
    fn the_composers_close_button_dismisses_it() {
        let (mut harness, state) = app_harness();
        state.comment_threads.set(Arc::new(vec![]));
        state.open_composer(None, None, "half a thought".into(), (perch_agent::Backend::ClaudeCode, "sonnet".into(), perch_agent::Effort::Medium));
        harness.rebuild();
        harness.paint();
        // The anchor row is the card's first row; its close button is the
        // last thing on it. Card: 12 margin + 12 padding from the column's
        // right edge; button 22 wide with a -4 margin.
        let x = 1400.0 - 12.0 - 12.0 + 4.0 - 11.0;
        let y = crate::theme::REVIEW_HEADER_HEIGHT + 2.0 * crate::theme::TAB_HEIGHT + 12.0 + 12.0 + 11.0;
        let under = harness.element_id_at(x, y);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_none(), "click at ({x}, {y}) over {under:?} did not close the composer");
        assert!(state.composer_draft.get().is_empty());
    }

    /// Opening a composer used to write `threads_open` whether or not the
    /// column was already open. The write rebuilt the column, and the
    /// composer's editor, torn down with the old column before its
    /// deferred listener registration ran, was left registered as a dead
    /// view. The next box tree commit then panicked inside floem asking it
    /// for a window origin. The follow-up line is found by clicking up the
    /// column from the bottom, so the test is not tied to the layout.
    #[test]
    fn a_follow_up_opens_without_rebuilding_the_column() {
        let (mut harness, state) = app_harness();
        let x = 1400.0 - crate::theme::THREADS_WIDTH + 40.0;
        let mut y = 860.0;
        while y > 0.0 && state.composer.get().is_none() {
            harness.click(x, y);
            harness.rebuild();
            harness.paint();
            y -= 6.0;
        }
        assert!(state.composer.get().and_then(|c| c.thread_id).is_some(), "never hit the follow-up line");
        harness.pointer_move(x, y + 40.0);
        harness.rebuild();
        harness.paint();
    }
}
