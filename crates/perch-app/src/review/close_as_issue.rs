//! Closing a pull request as an issue, for a pull request that found a
//! real bug but isn't worth merging.
//!
//! Close as issue… beside Close pull request unfolds a form at the foot
//! of Conversation: the issue's title and body, and the comment the pull
//! request gets as it closes. An agent can write the issue from the pull
//! request into the form. What is typed is kept per item in `AppState`
//! until Perch quits or the issue opens. Nothing goes to GitHub until
//! the button, and then the backend opens the issue, comments and closes
//! (`backend::close_as_issue`).

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, text_editor, Container, Decorators, Empty, Label, Stack, TextInput,
};
use floem::AnyView;

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::review::open_pull::action_button;
use crate::review::threads::{resizable_editor_box, resize_cursor_sheet, Grip};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{editor_box, lay_out_every_line, sidebar_glyph};

/// What the comment says until the reader changes it. `{issue}` becomes
/// the new issue's number.
pub const DEFAULT_COMMENT: &str =
    "Thanks for finding this. I've opened {issue} for the bug, so I'm closing this pull request.";

/// What the reader has put into one pull request's form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueDraft {
    pub title: String,
    pub body: String,
    pub comment: String,
    pub unfolded: bool,
    /// Perch's fills have been put in, which happens once.
    pub filled: bool,
    /// The reader has typed in the title or the body since Perch last put
    /// text there.
    pub title_typed: bool,
    pub body_typed: bool,
    /// Rises when the text is replaced from outside the form, so the form
    /// is built again around the new text.
    pub generation: u64,
}

/// Whether the reader has typed into the item's title or body.
pub(crate) fn typed(app: AppState, item_id: &str) -> bool {
    app.issue_drafts.with_untracked(|drafts| {
        drafts
            .get(item_id)
            .is_some_and(|d| d.title_typed || d.body_typed)
    })
}

/// Put an agent's title and body into the item's form.
pub(crate) fn take_draft(app: AppState, item_id: &str, title: String, body: String) {
    update_draft(app, item_id, |draft| {
        draft.title = title;
        draft.body = body;
        draft.title_typed = false;
        draft.body_typed = false;
        draft.filled = true;
        draft.generation += 1;
    });
}

fn draft_of(app: AppState, item_id: &str) -> IssueDraft {
    app.issue_drafts
        .with_untracked(|drafts| drafts.get(item_id).cloned().unwrap_or_default())
}

fn update_draft(app: AppState, item_id: &str, change: impl FnOnce(&mut IssueDraft)) {
    app.issue_drafts.update(|drafts| {
        change(drafts.entry(item_id.to_string()).or_default());
    });
}

/// The open item's id and its pull request's number, when it is a pull
/// request.
fn pull_of(state: ReviewState) -> Option<(String, u64)> {
    let item_id = state.item.with(|i| i.as_ref().map(|i| i.id.clone()))?;
    let number = state.pull_request.with(|p| p.as_ref().map(|p| p.number))?;
    Some((item_id, number))
}

/// Open the form, filling it the first time from the pull request: its
/// title, a body that says where the bug was found, and the comment.
pub fn unfold(app: AppState, state: ReviewState, item_id: &str) {
    let pull = state
        .pull_request
        .with_untracked(|p| p.as_ref().map(|p| (p.title.clone(), p.number)));
    let Some((title, number)) = pull else {
        return;
    };
    update_draft(app, item_id, |draft| {
        if !draft.filled {
            draft.title = title;
            draft.body = crate::backend::with_found_in("", number);
            draft.comment = DEFAULT_COMMENT.to_string();
            draft.filled = true;
            draft.generation += 1;
        }
        draft.unfolded = true;
    });
    state.issue_error.set(None);
}

fn fold(app: AppState, item_id: &str) {
    update_draft(app, item_id, |draft| draft.unfolded = false);
}

/// Why the issue can't be opened now, in the words the form uses. `None`
/// when it can.
pub(crate) fn refusal(state: ReviewState, title: &str) -> Option<String> {
    if state.offline.get() {
        return Some("GitHub is unavailable while Perch is offline.".into());
    }
    let open = state
        .pull_request
        .with(|p| p.as_ref().is_some_and(|p| p.is_open() && !p.merged));
    if !open {
        return Some("This pull request is no longer open.".into());
    }
    if title.trim().is_empty() {
        return Some("The issue needs a title.".into());
    }
    None
}

/// The form, while it is unfolded on a pull request. Nothing otherwise.
pub fn surface(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let shown = Memo::new(move |_| {
        let (item_id, number) = pull_of(state)?;
        let (unfolded, generation) = app.issue_drafts.with(|drafts| {
            drafts
                .get(&item_id)
                .map_or((false, 0), |d| (d.unfolded, d.generation))
        });
        (unfolded || state.issue_closing.get()).then_some((item_id, number, generation))
    });
    dyn_container(
        move || shown.get(),
        move |shown| {
            let Some((item_id, number, _)) = shown else {
                return Empty::new().into_any();
            };
            Container::new(form(app, state, backend.clone(), item_id, number))
                .debug_name("close-as-issue")
                .style(|s| {
                    s.width_full()
                        .margin_top(theme::CONVERSATION_ENTRY_PADDING)
                        .background(theme::CANVAS)
                        .border(1.0)
                        .border_color(theme::BORDER)
                        .border_radius(theme::RADIUS_LG)
                })
                .into_any()
        },
    )
    .style(|s| s.width_full())
}

fn form(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    item_id: String,
    number: u64,
) -> impl IntoView {
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
    let pending = move || state.issue_closing.get() || state.issue_drafting.get();
    let submit: Rc<dyn Fn()> = {
        let (backend, item_id) = (backend.clone(), item_id.clone());
        Rc::new(move || {
            if floem::reactive::Effect::untrack(|| {
                pending() || refusal(state, &title.get_untracked()).is_some()
            }) {
                return;
            }
            let draft = draft_of(app, &item_id);
            state.issue_error.set(None);
            state.issue_closing.set(true);
            backend.close_as_issue(&item_id, &draft.title, &draft.body, &draft.comment);
        })
    };

    let title_input = TextInput::new(title)
        .placeholder("Title")
        .debug_name("close-as-issue-title")
        .style(move |s| {
            s.width_full()
                .height(30.0)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(8.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::ACCENT))
                .set_disabled(pending())
        });

    let body_id = item_id.clone();
    let body = markdown_box(
        app,
        state,
        backend.clone(),
        start.body,
        "Describe the bug",
        "close-as-issue-body",
        submit.clone(),
        move |text| {
            update_draft(app, &body_id, |draft| {
                draft.body = text;
                draft.body_typed = true;
            })
        },
    );
    let body = resizable_editor_box(body_grip(state), body, true);

    let comment_id = item_id.clone();
    let comment = markdown_box(
        app,
        state,
        backend.clone(),
        start.comment,
        "No comment",
        "close-as-issue-comment",
        submit.clone(),
        move |text| update_draft(app, &comment_id, |draft| draft.comment = text),
    );
    let comment = editor_box(
        comment,
        || (theme::COMPOSER_MIN_HEIGHT, theme::COMPOSER_GROWN_HEIGHT),
        true,
    );

    let controls = {
        let (item_id, submit) = (item_id.clone(), submit.clone());
        let cancel_id = item_id.clone();
        Stack::horizontal((
            writer(app, state, backend.clone(), item_id.clone()),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            action_button(
                || "Cancel".to_string(),
                "close-as-issue-cancel",
                false,
                move || !pending(),
                move || fold(app, &cancel_id),
            ),
            action_button(
                move || {
                    if state.issue_closing.get() {
                        "Closing…".to_string()
                    } else {
                        "Open issue and close".to_string()
                    }
                },
                "close-as-issue-submit",
                true,
                move || !pending() && refusal(state, &title.get()).is_none(),
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
        heading(format!("Close #{number} as an issue")),
        title_input,
        body,
        heading(format!("Comment on #{number}")),
        comment,
        note(|| {
            "{issue} becomes the new issue's number. With no comment, the pull request just closes."
                .to_string()
        }),
        refused(state, title),
        offer(app, state, item_id.clone()),
        error_line(state),
        controls,
        resize_cursor_sheet(body_grip(state)),
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

fn heading(text: String) -> impl IntoView {
    Label::derived(move || text.clone()).style(|s| {
        s.font_size(theme::TEXT_META)
            .font_weight(theme::weight_emphasis())
            .color(theme::FG)
            .selectable(false)
    })
}

fn note(text: impl Fn() -> String + 'static) -> impl IntoView {
    Label::derived(text).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
            .text_wrap()
            .min_width(0.0)
    })
}

/// Why the button is off, while it is.
fn refused(state: ReviewState, title: RwSignal<String>) -> impl IntoView {
    let reason = move || {
        if state.issue_closing.get() {
            return None;
        }
        refusal(state, &title.get())
    };
    Label::derived(move || reason().unwrap_or_default())
        .debug_name("close-as-issue-refusal")
        .style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .apply_if(reason().is_none(), |s| s.hide())
        })
}

/// A box of Markdown going to GitHub, so `@` offers people, and files as
/// code spans. `Cmd-Enter` submits the form.
#[allow(clippy::too_many_arguments)]
fn markdown_box(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    text: String,
    placeholder: &str,
    debug: &'static str,
    submit: Rc<dyn Fn()>,
    on_text: impl Fn(String) + 'static,
) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;

    use crate::review::threads::cmd_enter;

    let editor = text_editor(text)
        .placeholder(placeholder.to_string())
        .editor_style(|s| s.hide_gutter(true).selection_color(theme::SELECTION_BG));
    let styling_doc = editor.editor().doc();
    let styling_id = state.next_token();
    let editor = editor.styling(crate::review::composer_styling::ComposerStyling::new(
        styling_doc,
        styling_id,
    ));
    lay_out_every_line(editor.editor());
    let mentioner = crate::review::mention::attach(app, state, backend, editor.editor(), |path| {
        format!("`{path}` ")
    });
    let focus_mentioner = mentioner.clone();
    let blur_mentioner = mentioner.clone();
    let key_mentioner = mentioner.clone();
    editor
        .pre_command(move |cmd| {
            if cmd_enter(&cmd) {
                submit();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(editor) = update.editor {
                on_text(editor.doc().text().to_string());
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
                if key_mentioner.key(key) {
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
                .set_disabled(state.issue_closing.get())
        })
        .debug_name(debug)
        .on_cleanup(move || mentioner.cleanup())
}

/// The body's box, which opens as tall as a pull request description's
/// and drags taller.
fn body_grip(state: ReviewState) -> Grip {
    Grip {
        height: state.issue_body_height,
        resizing: state.issue_body_resizing,
        opens_at: theme::PULL_BODY_HEIGHT,
        grows_to: theme::PULL_BODY_GROWN_HEIGHT,
        shortest: theme::COMPOSER_MIN_HEIGHT,
        tallest: theme::PULL_BODY_MAX_HEIGHT,
    }
}

/// Write with Claude or Codex, whichever was last asked here, and Stop
/// while it writes.
fn writer(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    item_id: String,
) -> impl IntoView {
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
                if state.issue_drafting.get() {
                    return "Writing…".to_string();
                }
                match choice().0 {
                    perch_agent::Backend::ClaudeCode => "Write with Claude".to_string(),
                    perch_agent::Backend::Codex => "Write with Codex".to_string(),
                }
            },
            "close-as-issue-write",
            false,
            move || !state.issue_drafting.get() && !state.issue_closing.get(),
            move || {
                state.issue_drafting.set(true);
                state.issue_draft_error.set(None);
                state.issue_draft_offer.set(None);
                backend.write_issue_draft(&item_id, choice());
            },
        ),
        action_button(
            || "Stop".to_string(),
            "close-as-issue-write-stop",
            false,
            || true,
            move || stop_backend.stop_issue_draft(&stop_id),
        )
        .style(move |s| s.apply_if(!state.issue_drafting.get(), |s| s.hide())),
    ))
    .style(|s| s.items_center().gap(6.0))
}

/// An agent's issue waiting because the reader had typed: Replace takes
/// it, Keep leaves what they wrote.
fn offer(app: AppState, state: ReviewState, item_id: String) -> impl IntoView {
    let shown = move || state.issue_draft_offer.with(Option::is_some);
    Stack::horizontal((
        Label::derived(|| "The issue is written. Replace what you wrote?".to_string()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG)
                .min_width(0.0)
                .flex_grow(1.0_f32)
        }),
        action_button(
            || "Keep mine".to_string(),
            "close-as-issue-keep",
            false,
            || true,
            move || state.issue_draft_offer.set(None),
        ),
        action_button(
            || "Replace".to_string(),
            "close-as-issue-replace",
            false,
            || true,
            move || {
                if let Some((title, body)) = state.issue_draft_offer.get_untracked() {
                    state.issue_draft_offer.set(None);
                    take_draft(app, &item_id, title, body);
                }
            },
        ),
    ))
    .debug_name("close-as-issue-offer")
    .style(move |s| {
        s.width_full()
            .items_center()
            .gap(8.0)
            .apply_if(!shown(), |s| s.hide())
    })
}

fn error_line(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || {
            state
                .issue_error
                .get()
                .or_else(|| state.issue_draft_error.get())
        },
        |message| -> AnyView {
            let Some(message) = message else {
                return Empty::new().into_any();
            };
            Stack::horizontal((
                sidebar_glyph(SidebarIcon::Warning, theme::CLOSED)
                    .style(|s| s.size(14.0, 14.0).margin_right(7.0).flex_shrink(0.0_f32)),
                Label::derived(move || message.clone())
                    .debug_name("close-as-issue-error")
                    .style(|s| {
                        s.min_width(0.0)
                            .flex_grow(1.0_f32)
                            .font_size(theme::TEXT_META)
                            .color(theme::CLOSED)
                    }),
            ))
            .style(|s| s.width_full().items_start())
            .into_any()
        },
    )
    .style(|s| s.width_full())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Command, DescriptionOutcome, Event};
    use floem::headless::{HeadlessHarness, TestRoot};
    use perch_github::{Item, ItemSubject, PullRequest};
    use std::sync::Arc;

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
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
        settle(harness);
    }

    fn settle(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 12,
            title: "Fix the crash on empty manifests".into(),
            html_url: "https://github.com/acme/repo/pull/12".into(),
            author_login: "drive-by".into(),
            author_avatar_url: String::new(),
            head_sha: "head-7".into(),
            head_ref: "fix".into(),
            base_sha: "base".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 1,
            deletions: 0,
            changed_files: 1,
            repo_full_name: "acme/repo".into(),
            head_repo_full_name: Some("drive-by/repo".into()),
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

    fn state_on_pull() -> ReviewState {
        let mut item = Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-7".into();
        item.subject = Some(ItemSubject::PullRequest(pull()));
        let state = ReviewState::new();
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull())));
        state.ready.set(true);
        state
    }

    fn commands_of(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    #[test]
    fn the_form_shows_once_unfolded_and_fills_from_the_pull_request_once() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_pull();
        let view = surface(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 900.0);
        settle(&mut harness);
        assert!(named(harness.root_id(), "close-as-issue").is_none());

        unfold(app, state, "item-7");
        settle(&mut harness);
        assert!(named(harness.root_id(), "close-as-issue").is_some());
        assert!(find(harness.root_id(), "Close #12 as an issue").is_some());
        assert!(find(harness.root_id(), "Comment on #12").is_some());
        let draft = draft_of(app, "item-7");
        assert_eq!(draft.title, "Fix the crash on empty manifests");
        assert_eq!(draft.body, "Found in #12.");
        assert_eq!(draft.comment, DEFAULT_COMMENT);
        assert!(!typed(app, "item-7"), "Perch's own fill is not typing");

        // Folded and unfolded again, what was there stays.
        update_draft(app, "item-7", |d| d.comment = "Closing.".into());
        click(&mut harness, "close-as-issue-cancel");
        assert!(named(harness.root_id(), "close-as-issue").is_none());
        unfold(app, state, "item-7");
        settle(&mut harness);
        assert_eq!(draft_of(app, "item-7").comment, "Closing.");
    }

    #[test]
    fn the_button_sends_the_form_and_the_answer_settles_it() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_pull();
        let (backend, mut commands) = Backend::recording();
        let view = surface(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 900.0);
        unfold(app, state, "item-7");
        update_draft(app, "item-7", |d| {
            d.title = "Empty manifests crash the loader".into();
            d.body = "It panics.\n\nFound in #12.".into();
        });
        settle(&mut harness);
        commands_of(&mut commands);

        click(&mut harness, "close-as-issue-submit");
        let sent = commands_of(&mut commands);
        assert!(
            matches!(
                sent.as_slice(),
                [Command::CloseAsIssue { item_id, title, body, comment }]
                    if item_id == "item-7"
                        && title == "Empty manifests crash the loader"
                        && body == "It panics.\n\nFound in #12."
                        && comment == DEFAULT_COMMENT
            ),
            "{sent:?}"
        );
        assert!(state.issue_closing.get_untracked());
        assert!(find(harness.root_id(), "Closing…").is_some());
        click(&mut harness, "close-as-issue-submit");
        assert!(commands_of(&mut commands).is_empty(), "nothing goes twice");

        // A refusal keeps the text and says why.
        app.apply_with_review(
            Event::ClosedAsIssue {
                item_id: "item-7".into(),
                result: Err("GitHub could not open the issue: gone".into()),
            },
            state,
        );
        settle(&mut harness);
        assert!(!state.issue_closing.get_untracked());
        assert!(find(harness.root_id(), "GitHub could not open the issue").is_some());
        assert_eq!(
            draft_of(app, "item-7").title,
            "Empty manifests crash the loader"
        );

        // Once the issue is open the text is spent and the form goes.
        app.apply_with_review(
            Event::ClosedAsIssue {
                item_id: "item-7".into(),
                result: Ok(58),
            },
            state,
        );
        settle(&mut harness);
        assert_eq!(state.issue_error.get_untracked(), None);
        assert_eq!(draft_of(app, "item-7"), IssueDraft::default());
        assert!(named(harness.root_id(), "close-as-issue").is_none());
    }

    #[test]
    fn an_empty_title_a_closed_pull_request_and_offline_refuse() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_pull();
        let (backend, mut commands) = Backend::recording();
        let view = surface(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 760.0, 900.0);
        unfold(app, state, "item-7");
        update_draft(app, "item-7", |d| d.title = "  ".into());
        settle(&mut harness);
        commands_of(&mut commands);

        assert_eq!(
            refusal(state, "  ").as_deref(),
            Some("The issue needs a title.")
        );
        assert!(find(harness.root_id(), "The issue needs a title.").is_some());
        click(&mut harness, "close-as-issue-submit");
        assert!(commands_of(&mut commands).is_empty());

        state.offline.set(true);
        assert!(refusal(state, "Bug").unwrap().contains("offline"));
        state.offline.set(false);
        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        assert_eq!(
            refusal(state, "Bug").as_deref(),
            Some("This pull request is no longer open.")
        );
    }

    fn written(title: &str, body: &str) -> Event {
        Event::IssueDraft {
            item_id: "item-7".into(),
            outcome: DescriptionOutcome::Written {
                title: title.into(),
                body: body.into(),
            },
        }
    }

    #[test]
    fn an_agents_issue_fills_an_untouched_form_and_asks_over_typed_text() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_pull();
        let (backend, mut commands) = Backend::recording();
        let view = surface(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 900.0, 900.0);
        unfold(app, state, "item-7");
        settle(&mut harness);
        assert!(find(harness.root_id(), "Write with Claude").is_some());

        commands_of(&mut commands);
        click(&mut harness, "close-as-issue-write");
        assert!(state.issue_drafting.get_untracked());
        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::WriteIssueDraft { item_id, .. } if item_id == "item-7"
        )));
        assert!(find(harness.root_id(), "Writing…").is_some());
        click(&mut harness, "close-as-issue-submit");
        assert!(
            commands_of(&mut commands).is_empty(),
            "no closing while it writes"
        );

        app.apply_with_review(written("Agent title", "Agent body"), state);
        settle(&mut harness);
        assert!(!state.issue_drafting.get_untracked());
        let draft = draft_of(app, "item-7");
        assert_eq!(
            (draft.title.as_str(), draft.body.as_str()),
            ("Agent title", "Agent body")
        );
        assert_eq!(
            draft.comment, DEFAULT_COMMENT,
            "the comment is the reader's"
        );

        // Typed over: the next answer waits for the reader.
        update_draft(app, "item-7", |d| {
            d.body = "Mine".into();
            d.body_typed = true;
        });
        app.apply_with_review(written("Second", "Second body"), state);
        settle(&mut harness);
        assert_eq!(draft_of(app, "item-7").body, "Mine");
        click(&mut harness, "close-as-issue-keep");
        assert_eq!(draft_of(app, "item-7").body, "Mine");
        app.apply_with_review(written("Third", "Third body"), state);
        settle(&mut harness);
        click(&mut harness, "close-as-issue-replace");
        assert_eq!(draft_of(app, "item-7").body, "Third body");
        assert!(!typed(app, "item-7"));

        // A failure says why and leaves the text.
        app.apply_with_review(
            Event::IssueDraft {
                item_id: "item-7".into(),
                outcome: DescriptionOutcome::Failed("No claude".into()),
            },
            state,
        );
        settle(&mut harness);
        assert_eq!(draft_of(app, "item-7").body, "Third body");
        assert!(find(harness.root_id(), "No claude").is_some());
    }

    #[test]
    fn the_form_fits_a_narrow_column() {
        for width in [480.0, 845.0] {
            let root = TestRoot::new();
            let app = AppState::new();
            let state = state_on_pull();
            let view = surface(app, state, Rc::new(Backend::detached()));
            let mut harness = HeadlessHarness::new_with_size(root, view, width, 900.0);
            unfold(app, state, "item-7");
            settle(&mut harness);
            let form = named(harness.root_id(), "close-as-issue").unwrap();
            let outer = harness.get_layout_rect(form);
            assert!(
                outer.x1 <= width + 0.5,
                "the form spills past {width}: {outer:?}"
            );
            for name in [
                "close-as-issue-title",
                "close-as-issue-body",
                "close-as-issue-comment",
                "close-as-issue-write",
                "close-as-issue-cancel",
                "close-as-issue-submit",
            ] {
                let id = named(harness.root_id(), name).unwrap();
                let rect = id.get_visual_rect_no_clip();
                assert!(
                    rect.x1 <= outer.x1 + 0.5,
                    "{name} spills past {width}: {rect:?}"
                );
                assert!(rect.width() > 20.0, "{name} squeezed at {width}: {rect:?}");
            }
        }
    }
}
