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
use perch_github::{Author, CommentMessage, CommentThread, RunRecord, RunStatus};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::doc::CardId;
use crate::review::model::{AgentChoice, Composer, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_button, name_label, sidebar_glyph, text_button, Name};

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
    /// Room between a thread's messages.
    fn message_gap(self) -> f64 {
        match self {
            ThreadFace::Inline => theme::THREAD_GAP,
            ThreadFace::Card => 20.0,
        }
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

    let send: Rc<dyn Fn()> = {
        let read_text = read_text.clone();
        let backend = backend.clone();
        Rc::new(move || {
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
            backend.ask(
                &item_id,
                composer.thread_id.clone(),
                composer.anchor.clone(),
                question.trim(),
                composer.backend,
                &composer.model,
                composer.effort,
            );
            state.agent_last.set(Some((
                composer.backend,
                composer.model.clone(),
                composer.effort,
            )));
            state.composer_sent();
        })
    };
    let key_send = send.clone();
    let escape_text = read_text.clone();

    let editor = editor
        // Cmd-Enter sends, before the editor can take it for a line.
        .pre_command(move |cmd| {
            if cmd_enter(&cmd) {
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
        .on_event_cont(el::FocusGained, move |_, _| {
            state.composer_focused.set(true)
        })
        .on_event_cont(el::FocusLost, move |_, _| state.composer_focused.set(false))
        // Escape closes an empty composer. In the capture phase, because
        // the editor stops every key once it has focus and this has to
        // run first.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
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
        .style(move |s| {
            let base = s
                .width_full()
                .min_height(72.0)
                .max_height(220.0)
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .padding(10.0)
                .background(theme::CANVAS)
                .border_radius(10.0);
            match face {
                ThreadFace::Card => base.border(0.0),
                ThreadFace::Inline => base
                    .border(1.0)
                    .border_color(theme::BORDER)
                    .focus(|s| s.border_color(theme::ACCENT)),
            }
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
            // since the thread's card takes the composer's place.
            icon_button(
                || SidebarIcon::Close,
                || theme::FG_SUBTLE,
                move || state.close_composer(),
            )
            .style(|s| s.size(theme::THREAD_MENU, theme::THREAD_MENU)),
        ))
        .style(|s| s.items_center().width_full().margin_bottom(8.0)),
        editor,
        Stack::horizontal((
            model_picker(app, state),
            effort_picker(app, state),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            send_button(send.clone()),
        ))
        .style(|s| s.items_center().width_full().gap(6.0).margin_top(8.0)),
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

/// The one high-contrast action in the composer, matching Codex's dark
/// primary controls while the model and effort remain quiet selectors.
fn send_button(on_click: Rc<dyn Fn()>) -> impl IntoView {
    Container::new(
        Stack::horizontal((
            Label::derived(|| "Send".to_string()).style(|s| {
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

/// The model to run with, from what config offers: Claude Code's models,
/// then Codex's under a line when there are any. A model carries its
/// backend, so picking one sets both.
fn model_picker(app: AppState, state: ReviewState) -> impl IntoView {
    use floem::menu::Menu;

    let current = move || {
        state.composer.with(|c| {
            c.as_ref()
                .map(|c| model_label(&c.model))
                .unwrap_or_default()
        })
    };
    menu_button(current, move || {
        let selected = state
            .composer
            .get_untracked()
            .map(|c| c.model)
            .unwrap_or_default();
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
    [claude, codex]
        .into_iter()
        .filter(|g| !g.is_empty())
        .collect()
}

/// The effort a model keeps when the one chosen is not among those it
/// takes: the highest it does take, so a reader who asked for Ultra of a
/// model without it gets Max and sees so, rather than Codex quietly
/// running Max under the name Ultra.
fn nearest_effort(
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

    let current = move || {
        state.composer.with(|c| {
            c.as_ref()
                .map(|c| c.effort.label().to_string())
                .unwrap_or_default()
        })
    };
    menu_button(current, move || {
        let (selected, model) = state
            .composer
            .get_untracked()
            .map(|c| (c.effort, c.model))
            .unwrap_or_default();
        let agent = app.agent.get_untracked();
        let mut menu = Menu::new();
        for effort in perch_agent::Effort::ALL {
            let takes = agent.effort_supported(&model, effort.name());
            menu = menu.item(effort.label(), move |item| {
                item.checked(effort == selected)
                    .enabled(takes)
                    .action(move || {
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

/// A thread of the reader's own inline in the code: dashed chrome led by a
/// chevron that collapses it to one row, with no anchor label since it is
/// already under those lines. It holds the messages, the run under an
/// agent's answer with its steps on one line, and the follow-up line. A
/// gutter marker click flashes its border.
pub fn thread_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread: CommentThread,
) -> impl IntoView {
    let card_id = CardId::Thread(thread.id.clone());
    let flashing = RwSignal::new(false);

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
        // Page grey inside a dashed border, as on Conversation, so it
        // cannot be taken for a line of GitHub's. Collapsed, it is its one
        // row exactly. The layer sets its width from the edges it is placed
        // against.
        let base = s
            .background(theme::CANVAS_SUBTLE)
            .border(floem::kurbo::Stroke::new(1.0).with_dashes(0.0, theme::THREAD_DASH))
            .border_radius(theme::RADIUS_LG)
            .apply_if(collapsed.get(), |s| {
                s.height(theme::CODE_LINE_HEIGHT as f64)
            });
        if flashing.get() {
            base.border_color(theme::ACCENT)
        } else {
            base.border_color(theme::BORDER)
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
    let menu = thread_menu(state, thread.id.clone(), theme::THREAD_MENU).into_any();
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
        delete_confirmation(state, backend.clone(), thread.id.clone())
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

/// A thread of the reader's own collapsed to its one row: who asked, the
/// first line of what they asked, how many messages follow, Outdated,
/// and the `...` menu, smaller than the open card's so it fits the row.
fn collapsed_row(app: AppState, state: ReviewState, thread: CommentThread) -> impl IntoView {
    let first = thread.messages.first();
    let author = first.map(|m| m.author.clone()).unwrap_or(Author::User);
    let line = first
        .map(|m| crate::review::timeline::first_line(&m.body))
        .unwrap_or_default();
    // Set off from the pill before it as the pills are from what is
    // before them: the pill's fill and the menu's hover fill are one
    // colour, and would touch.
    let menu = thread_menu(state, thread.id.clone(), theme::COLLAPSED_CARD_MENU)
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
/// asked, on its card. Nothing else deletes.
pub fn delete_confirmation(
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
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
            Stack::vertical((
                Label::derived(|| "Delete this thread?".to_string()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::FG)
                        .selectable(false)
                }),
                Label::derived(|| {
                    "The question, its answers and the anchor go. Nothing on GitHub is touched."
                        .to_string()
                })
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
                                backend.delete_comment_thread(&id);
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
/// in a round button `size` across.
pub fn thread_menu(state: ReviewState, thread_id: String, size: f64) -> impl IntoView {
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
    .style(move |s| {
        clickable(s)
            .size(size, size)
            .items_center()
            .justify_center()
            .border_radius(size / 2.0)
            .flex_shrink(0.0_f32)
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
            state.comment_threads.with(|t| {
                t.iter()
                    .find(|t| t.id == list_id)
                    .map(|t| t.messages.clone())
                    .unwrap_or_default()
            })
        },
        |m| {
            (
                m.id.clone(),
                m.body.len(),
                m.run
                    .as_ref()
                    .map(|r| (r.status.name(), r.steps.len(), r.error.clone())),
            )
        },
        move |m| {
            let thread = state
                .comment_threads
                .with_untracked(|t| t.iter().find(|t| t.id == thread_id).cloned());
            match thread {
                Some(thread) => {
                    message_view(app, state, backend.clone(), thread, m, face).into_any()
                }
                None => Empty::new().into_any(),
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
    dyn_container(
        move || {
            let composer = state.composer.with(|c| {
                c.as_ref()
                    .filter(|c| c.thread_id.as_deref() == Some(composer_thread.as_str()))
                    .cloned()
            });
            let drawn_here = match face {
                ThreadFace::Inline => state.follow_up_is_inline(&composer_thread),
                ThreadFace::Card => true,
            };
            (composer.filter(|_| drawn_here), running.get())
        },
        move |(composer, _)| match composer {
            // Under a thread the composer is a child of this container,
            // not a card placed by its edges, and takes the width here.
            Some(c) => composer_view(app, state, backend.clone(), c, face)
                .style(|s| s.width_full())
                .into_any(),
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
}
