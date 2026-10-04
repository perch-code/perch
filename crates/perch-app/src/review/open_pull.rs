//! Opening a pull request from a worktree of the reader's own, at the foot
//! of Conversation, where a pull request has Checks and Merge.
//!
//! Folded, one line: how many commits the branch has past its base, and
//! a button. Unfolded, the form: title, description, base and Draft.
//! What is typed is kept per item in `AppState` until Perch quits or the
//! pull request opens, so folding, or opening another item, loses
//! nothing. The backend does the pushing and the opening
//! (`backend::open_pull`) and says how far it got.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack, TextInput};
use perch_github::Item;

use crate::backend::{Backend, OpenStep, PullRequestReadiness};
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::review::threads::{resizable_editor_box, resize_cursor_sheet, Grip};
use crate::review::timeline::count_of;
use crate::state::{AppState, Checked};
use crate::theme;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::widgets::{self, action_button, checkbox, clickable, prose_editor, sidebar_glyph};

/// What the reader has put into one branch's form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullRequestDraft {
    pub title: String,
    pub body: String,
    /// The base picked from the chip; `None` goes into the branch's own.
    pub base: Option<String>,
    pub draft: bool,
    pub unfolded: bool,
    /// Perch's fills have been put in, which happens once.
    pub filled: bool,
    /// The reader has typed in the title or the description since Perch
    /// last put text there.
    pub title_typed: bool,
    pub body_typed: bool,
    /// Rises when the text is replaced from outside the form, so the form
    /// is built again around the new text.
    pub generation: u64,
}

/// Whether the reader has typed into the item's form.
pub(crate) fn typed(app: AppState, item_id: &str) -> bool {
    app.pull_request_drafts.with_untracked(|drafts| {
        drafts
            .get(item_id)
            .is_some_and(|d| d.title_typed || d.body_typed)
    })
}

/// Put an agent's title and description into the item's form.
pub(crate) fn take_description(app: AppState, item_id: &str, title: String, body: String) {
    update_draft(app, item_id, |draft| {
        draft.title = title;
        draft.body = body;
        draft.title_typed = false;
        draft.body_typed = false;
        draft.filled = true;
        draft.generation += 1;
    });
}

/// Whether the item is a branch a pull request could be opened from: no
/// subject, a worktree on disk, a repository on GitHub.
pub(crate) fn is_branch(item: &Item) -> bool {
    item.subject.is_none()
        && !perch_github::is_local(&item.repo_full_name)
        && item.checkout.as_ref().is_some_and(|c| c.on_disk)
}

fn branch_id(state: ReviewState) -> Option<String> {
    state
        .item
        .with(|i| i.as_ref().filter(|i| is_branch(i)).map(|i| i.id.clone()))
}

fn draft_of(app: AppState, item_id: &str) -> PullRequestDraft {
    app.pull_request_drafts
        .with_untracked(|drafts| drafts.get(item_id).cloned().unwrap_or_default())
}

fn update_draft(app: AppState, item_id: &str, change: impl FnOnce(&mut PullRequestDraft)) {
    app.pull_request_drafts.update(|drafts| {
        change(drafts.entry(item_id.to_string()).or_default());
    });
}

/// The branch's readiness when it has arrived and is good.
fn ready(state: ReviewState) -> Option<PullRequestReadiness> {
    state
        .pull_readiness
        .with(|r| r.as_ref().and_then(|r| r.as_ref().ok()).cloned())
}

/// Why the pull request can't be opened now, in the words the form uses.
/// `None` when it can.
pub(crate) fn refusal(app: AppState, state: ReviewState, title: &str) -> Option<String> {
    let item_id = branch_id(state)?;
    let readiness = state.pull_readiness.with(|r| r.clone());
    let readiness = match readiness {
        None => return Some("Reading the branch…".into()),
        Some(Err(message)) => return Some(message),
        Some(Ok(readiness)) => readiness,
    };
    let working = app.agent_runs.with(|runs| {
        runs.iter().any(|run| {
            run.item_id.as_deref() == Some(item_id.as_str())
                && run.mode == perch_github::RunMode::Work
                && run.status.is_active()
        })
    });
    if working {
        return Some("A Work run is running here. Wait for the run to finish.".into());
    }
    match readiness.head_branch.as_deref() {
        None => {
            return Some(format!(
                "The worktree's HEAD is detached, not on {}.",
                readiness.branch
            ))
        }
        Some(on) if on != readiness.branch => {
            return Some(format!("The worktree is on {on}, not {}.", readiness.branch))
        }
        Some(_) => {}
    }
    if readiness.base == readiness.branch {
        return Some("A pull request needs a base other than its own branch.".into());
    }
    if readiness.ahead == 0 {
        return Some(format!(
            "{} has nothing {} doesn't.",
            readiness.branch, readiness.base
        ));
    }
    if title.trim().is_empty() {
        return Some("The pull request needs a title.".into());
    }
    None
}

fn commits_ahead(readiness: &PullRequestReadiness) -> String {
    let commits = count_of(readiness.ahead as usize, "commit", "commits");
    format!("{commits} ahead of {}", readiness.base)
}

/// The surface, on a branch that has something to propose. Nothing on
/// anything else.
pub fn surface(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Asked again when the branch, its head, its dirt or the picked base
    // changes: the count, the warnings and the refusals all follow.
    let asked = Memo::new(move |_| {
        let item = state.item.with(|i| {
            i.as_ref().filter(|i| is_branch(i)).map(|i| {
                let head = i.checkout.as_ref().map(|c| c.head_sha.clone()).unwrap_or_default();
                (i.id.clone(), head)
            })
        })?;
        let base = app
            .pull_request_drafts
            .with(|drafts| drafts.get(&item.0).and_then(|d| d.base.clone()));
        Some((item.0, item.1, state.dirty.get(), base))
    });
    let ask_backend = backend.clone();
    Effect::new(move |_| {
        if let Some((item_id, _, _, base)) = asked.get() {
            ask_backend.pull_request_readiness(&item_id, base.as_deref());
        }
    });

    // Shown from the first readiness that finds commits past the base,
    // and kept while the form is open, so a base picked with nothing
    // past it says so rather than taking the form away.
    let shown = Memo::new(move |_| {
        let item_id = branch_id(state)?;
        let unfolded = app
            .pull_request_drafts
            .with(|drafts| drafts.get(&item_id).is_some_and(|d| d.unfolded));
        let has_commits = state.pull_readiness.with(|r| {
            r.as_ref()
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|r| r.ahead > 0 && r.base != r.branch)
        });
        let generation = app
            .pull_request_drafts
            .with(|drafts| drafts.get(&item_id).map_or(0, |d| d.generation));
        (unfolded || has_commits || state.pull_step.with(Option::is_some))
            .then_some((item_id, unfolded, generation))
    });

    dyn_container(
        move || shown.get(),
        move |shown| {
            let Some((item_id, unfolded, _)) = shown else {
                return Empty::new().into_any();
            };
            let content = if unfolded {
                form(app, state, backend.clone(), item_id).into_any()
            } else {
                folded(app, state, backend.clone(), item_id).into_any()
            };
            Container::new(content)
                .debug_name("open-pull-request")
                .style(|s| {
                    s.width_full()
                        .margin_top(theme::CONVERSATION_ENTRY_PADDING)
                        .background(theme::canvas())
                        .border(1.0)
                        .border_color(theme::border())
                        .border_radius(theme::RADIUS_LG)
                })
                .into_any()
        },
    )
    .style(|s| s.width_full())
}

/// One line: what the branch has, and the way into the form.
fn folded(app: AppState, state: ReviewState, backend: Rc<Backend>, item_id: String) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::PullRequest, theme::fg_muted)
            .style(|s| s.size(14.0, 14.0).flex_shrink(0.0_f32)),
        Label::derived(|| "Pull request".to_string()).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || ready(state).map(|r| commits_ahead(&r)).unwrap_or_default())
            .debug_name("open-pull-request-ahead")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .selectable(false)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            }),
        action_button(
            || "Open pull request…".to_string(),
            "open-pull-request-unfold",
            false,
            || true,
            move || unfold(app, state, &backend, &item_id),
        ),
    ))
    .style(|s| s.width_full().items_center().gap(8.0).padding_horiz(16.0).padding_vert(10.0))
}

/// Open the form, filling it the first time from what the backend read.
pub(crate) fn unfold(app: AppState, state: ReviewState, backend: &Backend, item_id: &str) {
    let readiness = ready(state);
    update_draft(app, item_id, |draft| {
        if !draft.filled {
            if let Some(readiness) = &readiness {
                draft.title = readiness.title.clone();
                draft.body = readiness.body.clone();
                draft.generation += 1;
                draft.filled = true;
            }
        }
        draft.unfolded = true;
    });
    state.pull_error.set(None);
    backend.pull_request_bases(item_id);
}

fn fold(app: AppState, item_id: &str) {
    update_draft(app, item_id, |draft| draft.unfolded = false);
}

/// The base the form goes into: the one picked, else the branch's own.
fn base_of(app: AppState, state: ReviewState, item_id: &str) -> Option<String> {
    app.pull_request_drafts
        .with(|drafts| drafts.get(item_id).and_then(|d| d.base.clone()))
        .or_else(|| ready(state).map(|r| r.base))
}

fn form(app: AppState, state: ReviewState, backend: Rc<Backend>, item_id: String) -> impl IntoView {
    let start = draft_of(app, &item_id);
    let title = RwSignal::new(start.title.clone());
    {
        let item_id = item_id.clone();
        Effect::new(move |_| {
            let text = title.get();
            if draft_of(app, &item_id).title != text {
                update_draft(app, &item_id, |draft| {
                    draft.title = text;
                    draft.title_typed = true;
                });
            }
        });
    }
    let pending = move || state.pull_step.with(Option::is_some) || state.describing.get();
    let submit: Rc<dyn Fn()> = {
        let (backend, item_id) = (backend.clone(), item_id.clone());
        Rc::new(move || {
            if pending() || refusal(app, state, &title.get_untracked()).is_some() {
                return;
            }
            let Some(base) = base_of(app, state, &item_id) else {
                return;
            };
            let draft = draft_of(app, &item_id);
            state.pull_error.set(None);
            state.pull_step.set(Some(OpenStep::Checking));
            backend.create_pull_request(&item_id, &draft.title, &draft.body, &base, draft.draft);
        })
    };

    let title_input = TextInput::new(title)
        .placeholder("Title")
        .debug_name("open-pull-request-title")
        .style(move |s| {
            s.width_full()
                .height(30.0)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(8.0)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::accent()))
                .set_disabled(pending())
        });

    let body = body_editor(app, state, backend.clone(), item_id.clone(), submit.clone());

    let controls = {
        let (item_id, submit) = (item_id.clone(), submit.clone());
        let cancel_id = item_id.clone();
        Stack::horizontal((
            base_chip(app, state, item_id.clone()),
            draft_box(app, item_id.clone()),
            writer(app, state, backend.clone(), item_id.clone()),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            action_button(
                || "Cancel".to_string(),
                "open-pull-request-cancel",
                false,
                move || !pending(),
                move || fold(app, &cancel_id),
            ),
            action_button(
                move || match state.pull_step.get() {
                    Some(OpenStep::Checking) => "Checking…".to_string(),
                    Some(OpenStep::Pushing) => "Pushing…".to_string(),
                    Some(OpenStep::Opening) => "Opening…".to_string(),
                    None => "Open pull request".to_string(),
                },
                "open-pull-request-submit",
                true,
                move || !pending() && refusal(app, state, &title.get()).is_none(),
                move || submit(),
            ),
        ))
        .style(|s| {
            s.width_full()
                .items_center()
                .gap(8.0)
                .flex_wrap(floem::taffy::FlexWrap::Wrap)
        })
    };

    let escape_id = item_id.clone();
    Stack::vertical((
        Label::derived(|| "Open a pull request".to_string()).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .selectable(false)
        }),
        title_input,
        body,
        notes(app, state, title),
        offer(app, state, item_id.clone()),
        error_line(state),
        controls,
        resize_cursor_sheet(pull_body_grip(state)),
    ))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape && key.modifiers.is_empty() && !pending() {
            fold(app, &escape_id);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| s.flex_col().width_full().gap(10.0).padding(16.0))
}

/// The description: a composer's box going to GitHub, so `@` offers
/// people, and files as code spans.
fn body_editor(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    item_id: String,
    submit: Rc<dyn Fn()>,
) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;

    use crate::review::threads::cmd_enter;

    let editor =
        prose_editor(draft_of(app, &item_id).body, "Description", Some(state.next_token()));
    let mentioner = crate::review::mention::attach(
        app,
        state,
        backend,
        editor.editor(),
        |path| format!("`{path}` "),
    );
    let focus_mentioner = mentioner.clone();
    let blur_mentioner = mentioner.clone();
    let key_mentioner = mentioner.clone();
    let tabs = crate::review::markdown_toolbar::Tabs::new(editor.editor(), || false);
    let key_tabs = tabs.clone();
    let editor = editor
        .pre_command(move |cmd| {
            if cmd_enter(&cmd) {
                submit();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(editor) = update.editor {
                let text = editor.doc().text().to_string();
                app.pull_request_drafts.update(|drafts| {
                    let draft = drafts.entry(item_id.clone()).or_default();
                    draft.body = text;
                    draft.body_typed = true;
                });
            }
        })
        .on_event_cont(el::FocusGained, move |_, _| focus_mentioner.focus(true))
        .on_event_cont(el::FocusLost, move |_, _| blur_mentioner.focus(false))
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                if key_mentioner.key(key) || key_tabs.key(key) {
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
                .set_disabled(state.pull_step.with(Option::is_some))
        })
        .debug_name("open-pull-request-body")
        .on_cleanup(move || mentioner.cleanup());
    tabs.view(
        state,
        resizable_editor_box(pull_body_grip(state), editor, true),
        theme::PULL_BODY_MAX_HEIGHT,
    )
}

/// The description's box, which opens about as tall as GitHub's and
/// drags taller, as a comment's does.
fn pull_body_grip(state: ReviewState) -> Grip {
    Grip {
        height: state.pull_body_height,
        resizing: state.pull_body_resizing,
        opens_at: theme::PULL_BODY_HEIGHT,
        grows_to: theme::PULL_BODY_GROWN_HEIGHT,
        shortest: theme::COMPOSER_MIN_HEIGHT,
        tallest: theme::PULL_BODY_MAX_HEIGHT,
    }
}

/// Write with Claude or Codex, whichever was last asked here, and Stop
/// while it writes.
fn writer(app: AppState, state: ReviewState, backend: Rc<Backend>, item_id: String) -> impl IntoView {
    let choice = move || {
        state
            .agent_last
            .get()
            .unwrap_or_else(|| crate::review::threads::defaults(app))
    };
    let stop_backend = backend.clone();
    let stop_id = item_id.clone();
    Stack::horizontal((
        action_button(
            move || {
                if state.describing.get() {
                    return "Writing…".to_string();
                }
                match choice().0 {
                    perch_agent::Backend::ClaudeCode => "Write with Claude".to_string(),
                    perch_agent::Backend::Codex => "Write with Codex".to_string(),
                }
            },
            "open-pull-request-write",
            false,
            move || !state.describing.get() && state.pull_step.with(Option::is_none),
            move || {
                let Some(base) = base_of(app, state, &item_id) else {
                    return;
                };
                state.describing.set(true);
                state.description_error.set(None);
                state.description_offer.set(None);
                backend.write_pull_request_description(&item_id, &base, choice());
            },
        ),
        action_button(
            || "Stop".to_string(),
            "open-pull-request-write-stop",
            false,
            || true,
            move || stop_backend.stop_pull_request_description(&stop_id),
        )
        .style(move |s| s.apply_if(!state.describing.get(), |s| s.hide())),
    ))
    .style(|s| s.items_center().gap(6.0))
}

/// An agent's answer waiting because the reader had typed: Replace takes
/// it, Keep leaves what they wrote.
fn offer(app: AppState, state: ReviewState, item_id: String) -> impl IntoView {
    let shown = move || state.description_offer.with(Option::is_some);
    Stack::horizontal((
        Label::derived(|| "The description is written. Replace what you wrote?".to_string())
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg())
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            }),
        action_button(
            || "Keep mine".to_string(),
            "open-pull-request-keep",
            false,
            || true,
            move || state.description_offer.set(None),
        ),
        action_button(
            || "Replace".to_string(),
            "open-pull-request-replace",
            false,
            || true,
            move || {
                if let Some((title, body)) = state.description_offer.get_untracked() {
                    state.description_offer.set(None);
                    take_description(app, &item_id, title, body);
                }
            },
        ),
    ))
    .debug_name("open-pull-request-offer")
    .style(move |s| {
        s.width_full()
            .items_center()
            .gap(8.0)
            .apply_if(!shown(), |s| s.hide())
    })
}

/// The base, picked from the repository's branches on GitHub.
fn base_chip(app: AppState, state: ReviewState, item_id: String) -> impl IntoView {
    let label_id = item_id.clone();
    let menu = move || base_menu(app, state, item_id.clone());
    Stack::horizontal((
        Label::derived(move || {
            base_of(app, state, &label_id)
                .map(|base| format!("into {base}"))
                .unwrap_or_else(|| "Base".to_string())
        })
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle)
            .style(|s| s.size(9.0, 9.0).pointer_events_none()),
    ))
    .dropdown(menu)
    .style(|s| {
        clickable(s)
            .items_center()
            .height(30.0)
            .padding_horiz(9.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()))
    })
    .debug_name("open-pull-request-base")
}

fn base_menu(app: AppState, state: ReviewState, item_id: String) -> Menu {
    let current = base_of(app, state, &item_id);
    let own = ready(state).map(|r| r.base);
    let mut menu = Menu::new();
    match state.pull_bases.get_untracked() {
        None => {
            menu = menu.item("Listing branches…", |item| item.enabled(false));
        }
        Some(Err(message)) => {
            menu = menu.item(message, |item| item.enabled(false));
        }
        Some(Ok(bases)) => {
            for base in bases {
                let checked = current.as_deref() == Some(base.as_str());
                let (item_id, picked) = (item_id.clone(), base.clone());
                let own = own.clone();
                menu = menu.item(base, move |item| {
                    item.checked(checked).action(move || {
                        // The branch's own base is no pick at all, so the
                        // form follows it if the branch moves.
                        let pick = (own.as_deref() != Some(picked.as_str())).then(|| picked.clone());
                        update_draft(app, &item_id, |draft| draft.base = pick);
                    })
                });
            }
        }
    }
    menu
}

fn draft_box(app: AppState, item_id: String) -> impl IntoView {
    let checked_id = item_id.clone();
    let toggle_id = item_id.clone();
    let toggle = move || update_draft(app, &toggle_id, |draft| draft.draft = !draft.draft);
    let click = toggle.clone();
    Stack::horizontal((
        checkbox(
            move || {
                let on = app
                    .pull_request_drafts
                    .with(|drafts| drafts.get(&checked_id).is_some_and(|d| d.draft));
                if on {
                    Checked::Full
                } else {
                    Checked::Empty
                }
            },
            toggle.clone(),
        ),
        Label::derived(|| "Draft".to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .selectable(false)
                .margin_left(6.0)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| click())
    .style(|s| clickable(s).items_center())
    .debug_name("open-pull-request-draft")
}

/// A warning that stops nothing, and the reason Open pull request is off.
fn notes(app: AppState, state: ReviewState, title: RwSignal<String>) -> impl IntoView {
    let dirty = move || {
        ready(state).filter(|r| r.dirty_files > 0).map(|r| match r.dirty_files {
            1 => "1 file has uncommitted changes. It won't be in the pull request.".to_string(),
            n => format!("{n} files have uncommitted changes. They won't be in the pull request."),
        })
    };
    let refused = move || {
        if state.pull_step.with(Option::is_some) {
            return None;
        }
        refusal(app, state, &title.get())
    };
    Stack::vertical((
        Label::derived(move || dirty().unwrap_or_default())
            .debug_name("open-pull-request-dirty")
            .style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::draft_yellow())
                    .apply_if(dirty().is_none(), |s| s.hide())
            }),
        Label::derived(move || refused().unwrap_or_default())
            .debug_name("open-pull-request-refusal")
            .style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .apply_if(refused().is_none(), |s| s.hide())
            }),
    ))
    .style(|s| s.flex_col().gap(4.0))
}

fn error_line(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || state.pull_error.get().or_else(|| state.description_error.get()),
        |message| widgets::error_line(message, "open-pull-request-error"),
    )
    .style(|s| s.width_full())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Command;
    use floem::headless::{HeadlessHarness, TestRoot};
    use perch_github::ItemCheckout;
    use std::sync::Arc;

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children().into_iter().find_map(|child| named(child, name))
    }

    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no view {name}"));
        let center = id.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        harness.rebuild();
        harness.paint();
    }

    fn settle(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
    }

    fn branch() -> Item {
        let mut item = Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-7".into();
        item.name = Some("Fix the fence".into());
        item.checkout = Some(ItemCheckout {
            branch: "feature".into(),
            base_ref: "main".into(),
            head_sha: "head-7".into(),
            path: "/tmp/perch-open-pull-test".into(),
            owned: false,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        item
    }

    fn readiness(ahead: u32) -> PullRequestReadiness {
        PullRequestReadiness {
            branch: "feature".into(),
            base: "main".into(),
            ahead,
            dirty_files: 0,
            head_branch: Some("feature".into()),
            title: "Add the gate".into(),
            body: "Because the fence had none.".into(),
        }
    }

    fn pull() -> perch_github::PullRequest {
        perch_github::PullRequest {
            number: 12,
            title: "Add the gate".into(),
            html_url: "https://github.com/acme/repo/pull/12".into(),
            author_login: "me".into(),
            author_avatar_url: String::new(),
            head_sha: "head-7".into(),
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
            review_requested: false,
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

    fn state_on(item: Item) -> ReviewState {
        let state = ReviewState::new();
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state
    }

    fn commands_of(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    #[test]
    fn the_surface_asks_and_shows_only_on_a_branch_with_commits_past_its_base() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        let (backend, mut commands) = Backend::recording();
        let view = surface(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 600.0);
        settle(&mut harness);

        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::PullRequestReadiness { item_id, base: None } if item_id == "item-7"
        )));
        assert!(named(harness.root_id(), "open-pull-request").is_none(), "nothing read yet");

        state.pull_readiness.set(Some(Ok(readiness(0))));
        settle(&mut harness);
        assert!(named(harness.root_id(), "open-pull-request").is_none(), "nothing to propose");

        state.pull_readiness.set(Some(Ok(readiness(3))));
        settle(&mut harness);
        assert!(named(harness.root_id(), "open-pull-request").is_some());
        assert!(find(harness.root_id(), "3 commits ahead of main").is_some());

        // A branch that became a pull request has Merge there instead.
        let mut pulled = branch();
        pulled.subject = Some(perch_github::ItemSubject::PullRequest(pull()));
        state.item.set(Some(Arc::new(pulled)));
        settle(&mut harness);
        assert!(named(harness.root_id(), "open-pull-request").is_none());
    }

    #[test]
    fn the_form_fills_once_and_opens_with_what_it_holds() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        let (backend, mut commands) = Backend::recording();
        let view = surface(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 700.0);
        state.pull_readiness.set(Some(Ok(readiness(1))));
        settle(&mut harness);

        click(&mut harness, "open-pull-request-unfold");
        assert!(commands_of(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::PullRequestBases { item_id } if item_id == "item-7")));
        let draft = draft_of(app, "item-7");
        assert_eq!(draft.title, "Add the gate");
        assert_eq!(draft.body, "Because the fence had none.");
        assert!(named(harness.root_id(), "open-pull-request-title").is_some());

        // Typed text isn't filled over: fold, change what the backend
        // reads, unfold.
        update_draft(app, "item-7", |d| d.title = "My title".into());
        click(&mut harness, "open-pull-request-cancel");
        assert!(named(harness.root_id(), "open-pull-request-title").is_none());
        let mut later = readiness(2);
        later.title = "Fix the fence".into();
        state.pull_readiness.set(Some(Ok(later)));
        settle(&mut harness);
        click(&mut harness, "open-pull-request-unfold");
        assert_eq!(draft_of(app, "item-7").title, "My title");

        update_draft(app, "item-7", |d| {
            d.draft = true;
            d.base = Some("develop".into());
        });
        settle(&mut harness);
        commands_of(&mut commands);
        state.pull_readiness.set(Some(Ok(PullRequestReadiness {
            base: "develop".into(),
            ..readiness(2)
        })));
        settle(&mut harness);
        click(&mut harness, "open-pull-request-submit");
        let sent = commands_of(&mut commands);
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::CreatePullRequest { item_id, title, body, base, draft: true }
                    if item_id == "item-7" && title == "My title"
                        && body == "Because the fence had none." && base == "develop"
            )),
            "{sent:?}"
        );
        assert_eq!(state.pull_step.get_untracked(), Some(OpenStep::Checking));
        assert!(find(harness.root_id(), "Checking…").is_some());

        // A second press while it runs sends nothing.
        click(&mut harness, "open-pull-request-submit");
        assert!(commands_of(&mut commands)
            .iter()
            .all(|c| !matches!(c, Command::CreatePullRequest { .. })));
    }

    #[test]
    fn a_running_work_run_a_switched_branch_and_an_empty_title_refuse() {
        let _root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        state.pull_readiness.set(Some(Ok(readiness(1))));
        assert_eq!(refusal(app, state, "Title"), None);
        assert_eq!(
            refusal(app, state, " "),
            Some("The pull request needs a title.".into())
        );

        let mut elsewhere = readiness(1);
        elsewhere.head_branch = Some("other".into());
        state.pull_readiness.set(Some(Ok(elsewhere)));
        assert_eq!(
            refusal(app, state, "Title"),
            Some("The worktree is on other, not feature.".into())
        );
        let mut detached = readiness(1);
        detached.head_branch = None;
        state.pull_readiness.set(Some(Ok(detached)));
        assert!(refusal(app, state, "Title").unwrap().contains("detached"));

        state.pull_readiness.set(Some(Ok(readiness(1))));
        let at: jiff::Timestamp = "2026-09-20T09:00:00Z".parse().unwrap();
        let run = crate::ui::agents::model::AgentRunSummary {
            id: "run".into(),
            item_id: Some("item-7".into()),
            thread_id: Some("t".into()),
            review_attempt_id: None,
            repository: "acme/repo".into(),
            worktree: "/tmp/perch-open-pull-test".into(),
            purpose: crate::ui::agents::model::AgentRunPurpose::Question,
            request: "work".into(),
            backend: "claude-code".into(),
            model: "model".into(),
            effort: "high".into(),
            mode: perch_github::RunMode::Work,
            status: crate::ui::agents::model::AgentRunStatus::Running,
            terminal_reason: None,
            queue_position: None,
            queued_at: at,
            started_at: Some(at),
            ended_at: None,
            latest_activity: None,
            steps: Arc::new(Vec::new()),
            response: String::new(),
            response_truncated: false,
            error: None,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            retry_of: None,
            source_available: true,
            last_event_sequence: 0,
        };
        app.agent_runs.set(Arc::new(vec![run]));
        assert_eq!(
            refusal(app, state, "Title"),
            Some("A Work run is running here. Wait for the run to finish.".into())
        );
    }

    #[test]
    fn uncommitted_files_warn_and_do_not_refuse() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        let view = surface(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 700.0);
        state.pull_readiness.set(Some(Ok(PullRequestReadiness {
            dirty_files: 4,
            ..readiness(1)
        })));
        settle(&mut harness);
        click(&mut harness, "open-pull-request-unfold");
        assert!(find(
            harness.root_id(),
            "4 files have uncommitted changes. They won't be in the pull request."
        )
        .is_some());
        assert_eq!(refusal(app, state, "Title"), None);
    }

    #[test]
    fn an_opened_pull_request_takes_the_draft_and_a_failure_keeps_it() {
        let _root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        update_draft(app, "item-7", |d| d.title = "Kept".into());
        state.pull_step.set(Some(OpenStep::Pushing));

        app.apply_with_review(
            crate::backend::Event::OpenPullRequestFailed {
                item_id: "item-7".into(),
                message: "Couldn't push".into(),
            },
            state,
        );
        assert_eq!(state.pull_step.get_untracked(), None);
        assert_eq!(state.pull_error.get_untracked().as_deref(), Some("Couldn't push"));
        assert_eq!(draft_of(app, "item-7").title, "Kept");

        app.apply_with_review(
            crate::backend::Event::PullRequestOpened {
                item_id: "item-7".into(),
                number: 12,
                notice: Some("#12 was already open on this branch.".into()),
            },
            state,
        );
        assert_eq!(state.pull_error.get_untracked(), None);
        assert_eq!(draft_of(app, "item-7"), PullRequestDraft::default());
        assert_eq!(
            app.notice().as_deref(),
            Some("#12 was already open on this branch.")
        );
    }

    fn written(title: &str, body: &str) -> crate::backend::Event {
        crate::backend::Event::PullRequestDescription {
            item_id: "item-7".into(),
            outcome: crate::backend::DescriptionOutcome::Written {
                title: title.into(),
                body: body.into(),
            },
        }
    }

    #[test]
    fn an_agents_description_fills_an_untouched_form_and_asks_over_typed_text() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        let (backend, mut commands) = Backend::recording();
        let view = surface(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 900.0, 700.0);
        state.pull_readiness.set(Some(Ok(readiness(2))));
        settle(&mut harness);
        click(&mut harness, "open-pull-request-unfold");
        assert!(!typed(app, "item-7"), "Perch's own fill is not typing");
        assert!(find(harness.root_id(), "Write with Claude").is_some());

        commands_of(&mut commands);
        click(&mut harness, "open-pull-request-write");
        assert!(state.describing.get_untracked());
        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::WritePullRequestDescription { item_id, base, .. }
                if item_id == "item-7" && base == "main"
        )));
        assert!(find(harness.root_id(), "Writing…").is_some());

        app.apply_with_review(written("Agent title", "Agent body"), state);
        settle(&mut harness);
        assert!(!state.describing.get_untracked());
        let draft = draft_of(app, "item-7");
        assert_eq!((draft.title.as_str(), draft.body.as_str()), ("Agent title", "Agent body"));
        assert!(named(harness.root_id(), "open-pull-request-title").is_some());

        // Typed over: the next answer waits for the reader.
        update_draft(app, "item-7", |d| {
            d.body = "Mine".into();
            d.body_typed = true;
        });
        app.apply_with_review(written("Second", "Second body"), state);
        settle(&mut harness);
        assert_eq!(draft_of(app, "item-7").body, "Mine");
        assert_eq!(
            state.description_offer.get_untracked(),
            Some(("Second".into(), "Second body".into()))
        );
        click(&mut harness, "open-pull-request-replace");
        assert_eq!(draft_of(app, "item-7").body, "Second body");
        assert!(!typed(app, "item-7"));
        assert_eq!(state.description_offer.get_untracked(), None);

        // A failure says why and leaves the text.
        app.apply_with_review(
            crate::backend::Event::PullRequestDescription {
                item_id: "item-7".into(),
                outcome: crate::backend::DescriptionOutcome::Failed("No claude".into()),
            },
            state,
        );
        settle(&mut harness);
        assert_eq!(draft_of(app, "item-7").body, "Second body");
        assert!(find(harness.root_id(), "No claude").is_some());
    }

    #[test]
    fn the_form_fits_a_narrow_column() {
        for width in [480.0, 845.0] {
            let root = TestRoot::new();
            let app = AppState::new();
            let state = state_on(branch());
            let view = surface(app, state, Rc::new(Backend::detached()));
            let mut harness = HeadlessHarness::new_with_size(root, view, width, 700.0);
            state.pull_readiness.set(Some(Ok(readiness(2))));
            settle(&mut harness);
            click(&mut harness, "open-pull-request-unfold");
            let form = named(harness.root_id(), "open-pull-request").unwrap();
            let outer = harness.get_layout_rect(form);
            for name in [
                "open-pull-request-base",
                "open-pull-request-draft",
                "open-pull-request-write",
                "open-pull-request-cancel",
                "open-pull-request-submit",
                "open-pull-request-title",
            ] {
                let id = named(harness.root_id(), name).unwrap();
                let rect = id.get_visual_rect_no_clip();
                assert!(outer.x1 <= width + 0.5, "the form spills past {width}: {outer:?}");
                assert!(rect.x1 <= outer.x1 + 0.5, "{name} spills past {width}: {rect:?}");
                assert!(rect.width() > 20.0, "{name} squeezed at {width}: {rect:?}");
            }
        }
    }

    /// The description opens about as tall as GitHub's, and its grip
    /// drags it taller, as a comment's does, and keeps the height.
    #[test]
    fn the_description_opens_tall_and_drags_taller() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on(branch());
        let view = surface(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 1200.0);
        state.pull_readiness.set(Some(Ok(readiness(2))));
        settle(&mut harness);
        click(&mut harness, "open-pull-request-unfold");

        let text_box = named(harness.root_id(), "open-pull-request-body")
            .and_then(|body| body.parent())
            .expect("the description's box");
        let height = move || text_box.get_size().expect("laid out").height;
        assert_eq!(height(), theme::PULL_BODY_HEIGHT);

        let grip = find(harness.root_id(), "composer resize handle").expect("the grip");
        let center = grip.get_visual_rect_no_clip().center();
        harness.pointer_down(center.x, center.y);
        harness.pointer_move(center.x, center.y + 150.0);
        assert!(state.pull_body_resizing.get());
        harness.pointer_up(center.x, center.y + 150.0);
        settle(&mut harness);
        assert!(!state.pull_body_resizing.get());
        assert_eq!(state.pull_body_height.get(), Some(theme::PULL_BODY_HEIGHT + 150.0));
        assert_eq!(height(), theme::PULL_BODY_HEIGHT + 150.0);
        assert_eq!(state.composer_height.get(), None, "a comment's box is its own");
    }
}
