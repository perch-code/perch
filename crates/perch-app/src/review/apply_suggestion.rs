//! Apply suggestion under a suggested change on an open pull request, as
//! github.com has it. The button opens a popover holding the commit's
//! message, `Update <path>` to start with and a description if the
//! reader wants one; Commit changes commits the change on the pull
//! request's head branch and resolves the thread, as github.com does.
//!
//! The popover hangs under the button over the window, as a menu does,
//! and is as modal: a press outside puts it away, the wheel does not
//! scroll what is under it, and it keeps the keyboard until it goes.
//! While the commit is out it stays, and says why if GitHub refuses.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::kurbo::{Rect, Size};
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, text_editor, Container, Decorators, Empty, Label, Stack, TextInput,
};
use floem::{AnyView, View, ViewId};
use perch_github::{CommentThread, Side};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::markdown::Foot;
use crate::review::model::ReviewState;
use crate::review::open_pull::action_button;
use crate::review::suggestion::{headline_for, Commit, Original};
use crate::theme;
use crate::ui::widgets::{editor_box, lay_out_every_line, sidebar_glyph};

/// The popover's width, GitHub's.
const WIDTH: f64 = 400.0;
/// The least room kept between the popover and the window's edge.
const EDGE: f64 = 8.0;
/// Between the button and the popover.
const GAP: f64 = 4.0;

/// The foot under a suggested change in a message of `thread` that is on
/// GitHub. None where Apply suggestion can never be pressed: a thread off
/// the head's lines, an outdated one, or one whose lines are not known.
pub fn foot(
    state: ReviewState,
    backend: Rc<Backend>,
    thread: &CommentThread,
    original: Option<Original>,
) -> Option<Foot> {
    let anchor = thread.anchor.clone()?;
    let original = original?;
    if thread.outdated || anchor.side != Side::Right {
        return None;
    }
    let thread_id = thread.id.clone();
    Some(Foot::new(move |lines| {
        bar(
            state,
            backend.clone(),
            thread_id.clone(),
            anchor.path.clone(),
            original.clone(),
            lines,
        )
    }))
}

/// Why Apply suggestion is off, as github.com words it, or `None` when
/// it can be pressed.
fn refusal(state: ReviewState, thread_id: &str, unchanged: bool) -> Option<&'static str> {
    if unchanged {
        Some("This suggestion is invalid because no changes were made to the code.")
    } else if state.offline.get() {
        Some("GitHub is unavailable while Perch is offline.")
    } else if state.is_resolved(thread_id) {
        Some("This suggestion has been applied or marked resolved.")
    } else {
        None
    }
}

/// The row under the lines with Apply suggestion at its end, while the
/// pull request is open.
fn bar(
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
    path: String,
    original: Original,
    lines: Vec<String>,
) -> AnyView {
    let unchanged = lines == original.lines;
    let open = Memo::new(move |_| {
        state
            .pull_request
            .with(|p| p.as_ref().is_some_and(|p| p.is_open() && !p.merged))
    });
    let refused_id = thread_id.clone();
    let refused = Memo::new(move |_| refusal(state, &refused_id, unchanged));
    let seed = Commit {
        thread_id,
        headline: headline_for(&path),
        path,
        original,
        lines,
        body: String::new(),
    };
    dyn_container(
        move || open.get().then(|| refused.get()),
        move |shown| {
            let Some(refused) = shown else {
                return Empty::new().into_any();
            };
            let trigger = Rc::new(Cell::new(None::<ViewId>));
            let (backend, seed, at) = (backend.clone(), seed.clone(), trigger.clone());
            let button = action_button(
                || "Apply suggestion".to_string(),
                "apply-suggestion",
                false,
                move || refused.is_none(),
                move || {
                    if let Some(trigger) = at.get() {
                        open_popover(state, backend.clone(), trigger, seed.clone());
                    }
                },
            );
            trigger.set(Some(button.id()));
            let button = match refused {
                Some(why) => crate::review::cards::tip(button, why.to_string()).into_any(),
                None => button.into_any(),
            };
            Stack::horizontal((Empty::new().style(|s| s.flex_grow(1.0_f32)), button))
                .debug_name("suggestion-foot")
                .style(|s| {
                    s.width_full()
                        .items_center()
                        .padding(8.0)
                        .border_top(1.0)
                        .border_color(theme::border_muted())
                })
                .into_any()
        },
    )
    .style(|s| s.width_full())
    .into_any()
}

/// The popover while it is up: its overlay, and whether a commit from it
/// is out, which keeps it up.
struct Open {
    overlay: ViewId,
    busy: Box<dyn Fn() -> bool>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

/// Whether the popover is up.
#[cfg(test)]
pub fn is_open() -> bool {
    OPEN.with(|open| open.borrow().is_some())
}

/// Put the popover away, unless its commit is out.
pub fn close() {
    let busy = OPEN.with(|open| open.borrow().as_ref().is_some_and(|o| (o.busy)()));
    if !busy {
        remove();
    }
}

fn remove() {
    if let Some(open) = OPEN.with(|open| open.borrow_mut().take()) {
        open.overlay.clear_focus();
        floem::action::remove_overlay(open.overlay);
    }
}

/// Where a popover `size` big goes under `trigger`, its right edge at
/// the trigger's, in a window `window` big: above it without room below.
fn place(trigger: Rect, size: Size, window: Size) -> (f64, f64) {
    let x = (trigger.x1 - size.width)
        .min(window.width - EDGE - size.width)
        .max(EDGE);
    let below = trigger.y1 + GAP;
    let above = trigger.y0 - GAP - size.height;
    let y = if below + size.height <= window.height - EDGE || above < EDGE {
        below
    } else {
        above
    };
    (x, y)
}

fn open_popover(state: ReviewState, backend: Rc<Backend>, trigger: ViewId, seed: Commit) {
    crate::ui::menu::close();
    remove();
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    let rect = trigger.get_visual_rect_no_clip();
    let scope = Scope::new();
    let token = scope.enter(|| RwSignal::new(None::<u64>));
    let busy = move || {
        token.get_untracked().is_some_and(|t| {
            state
                .suggestion_commits
                .with_untracked(|c| matches!(c.get(&t), Some(None)))
        })
    };
    let view = scope.enter(|| popover(state, backend, item_id, rect, seed, token, scope));
    let overlay = floem::action::add_overlay(view);
    OPEN.with(|open| {
        *open.borrow_mut() = Some(Open {
            overlay,
            busy: Box::new(busy),
        })
    });
}

fn popover(
    state: ReviewState,
    backend: Rc<Backend>,
    item_id: String,
    trigger: Rect,
    seed: Commit,
    token: RwSignal<Option<u64>>,
    scope: Scope,
) -> impl View {
    let headline = RwSignal::new(seed.headline.clone());
    let body = RwSignal::new(String::new());
    let answer = move || {
        token
            .get()
            .and_then(|t| state.suggestion_commits.with(|c| c.get(&t).cloned()))
    };
    let pending = move || matches!(answer(), Some(None));
    // Committed: the popover goes, and the thread resolves under it.
    Effect::new(move |_| {
        if matches!(answer(), Some(Some(Ok(())))) {
            remove();
        }
    });
    let commit: Rc<dyn Fn()> = Rc::new(move || {
        let text = headline.get_untracked();
        if Effect::untrack(pending) || text.trim().is_empty() {
            return;
        }
        let commit = Commit {
            headline: text,
            body: body.get_untracked(),
            ..seed.clone()
        };
        if let Some(old) = token.get_untracked() {
            state.forget_suggestion_commit(old);
        }
        let sent = backend.commit_suggestion(&item_id, commit);
        state.start_suggestion_commit(sent);
        token.set(Some(sent));
    });

    let title = Label::new("Apply suggestion").style(|s| {
        s.font_size(theme::TEXT_BODY)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg())
            .flex_grow(1.0_f32)
            .selectable(false)
    });
    let close_button = crate::review::submit_review::icon_action(
        SidebarIcon::Close,
        "apply-suggestion-close",
        move || !pending(),
        close,
    );
    let heading = Stack::horizontal((title, close_button)).style(|s| {
        s.width_full()
            .items_center()
            .padding_left(14.0)
            .padding_right(8.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    });

    let enter_commit = commit.clone();
    let headline_input = TextInput::new(headline)
        .placeholder("Commit message")
        .debug_name("apply-suggestion-headline")
        // Enter commits, as on github.com; the box would take it itself.
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if key.code == Code::Enter && key.modifiers.is_empty() {
                enter_commit();
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
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
    let headline_id = headline_input.id();

    let editor = text_editor("")
        .placeholder("Add an optional extended description\u{2026}".to_string())
        .editor_style(|s| s.hide_gutter(true).selection_color(theme::selection_bg()));
    lay_out_every_line(editor.editor());
    let key_commit = commit.clone();
    let editor = editor
        .pre_command(move |cmd| {
            use floem::views::editor::command::CommandExecuted;
            if crate::review::threads::cmd_enter(&cmd) {
                key_commit();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(editor) = update.editor {
                body.set(editor.doc().text().to_string());
            }
        })
        .style(move |s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .set_disabled(pending())
        })
        .debug_name("apply-suggestion-description");
    let description = editor_box(
        editor,
        || (theme::COMPOSER_MIN_HEIGHT, theme::COMPOSER_GROWN_HEIGHT),
        true,
    );

    let error = dyn_container(
        move || match answer() {
            Some(Some(Err(message))) => Some(message),
            _ => None,
        },
        |message| -> AnyView {
            let Some(message) = message else {
                return Empty::new().into_any();
            };
            Stack::horizontal((
                sidebar_glyph(SidebarIcon::Warning, theme::closed)
                    .style(|s| s.size(14.0, 14.0).margin_right(7.0).flex_shrink(0.0_f32)),
                Label::derived(move || message.clone())
                    .debug_name("apply-suggestion-error")
                    .style(|s| {
                        s.min_width(0.0)
                            .flex_grow(1.0_f32)
                            .font_size(theme::TEXT_META)
                            .color(theme::closed())
                    }),
            ))
            .style(|s| s.width_full().items_start())
            .into_any()
        },
    )
    .style(|s| s.width_full());

    let button_commit = commit.clone();
    let controls = Stack::horizontal((
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            move || {
                if pending() {
                    "Committing\u{2026}".to_string()
                } else {
                    "Commit changes".to_string()
                }
            },
            "apply-suggestion-commit",
            true,
            move || !pending() && !headline.with(|h| h.trim().is_empty()),
            move || button_commit(),
        ),
    ))
    .style(|s| s.width_full().items_center());

    let form = Stack::vertical((headline_input, description, error, controls))
        .style(|s| s.flex_col().width_full().gap(10.0).padding(14.0));

    let measured = RwSignal::new(None::<Size>);
    let window = RwSignal::new(Size::ZERO);
    let panel =
        Container::new(Stack::vertical((heading, form)).style(|s| s.flex_col().width_full()));
    let panel_id = panel.id();
    let panel = panel
        .on_event_stop(el::PointerDown, |_, _| {})
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if measured.get_untracked().is_none() {
                let size = panel_id.get_layout_rect().size();
                if size.width > 0.0 && size.height > 0.0 {
                    measured.set(Some(size));
                }
            }
        })
        .debug_name("apply-suggestion-popover")
        .style(move |s| {
            let s = s
                .absolute()
                .flex_col()
                .width(WIDTH)
                .apply(crate::ui::menu::surface(floem::style::Style::new()));
            match measured.get() {
                Some(size) if window.get().width > 0.0 => {
                    let (x, y) = place(trigger, size, window.get());
                    s.inset_left(x).inset_top(y)
                }
                // Laid out once in the corner to be measured, where the
                // pointer must not find it.
                _ => s.inset_left(0.0).inset_top(0.0).pointer_events_none(),
            }
        });

    let asked = Cell::new(false);
    let root = Container::new(panel);
    let root_id = root.id();
    root.on_event_stop(el::PointerDown, |_, _| close())
        .on_event_stop(el::PointerWheel, |_, _| {})
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if key.code == Code::Escape && key.modifiers.is_empty() {
                close();
            } else if key.code == Code::Enter && key.modifiers == Modifiers::META {
                commit();
            }
            // Nothing typed here reaches the window's keys.
            EventPropagation::Stop
        })
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            let size = root_id.get_layout_rect().size();
            if size.width > 0.0 && window.get_untracked() != size {
                window.set(size);
            }
            if !asked.replace(true) {
                headline_id.request_focus();
            }
        })
        .on_cleanup(move || {
            if let Some(t) = token.get_untracked() {
                state.forget_suggestion_commit(t);
            }
            scope.dispose();
        })
        .debug_name("apply-suggestion")
        .style(|s| {
            s.inset(0.0)
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_META)
                .font_weight(theme::weight_body())
                .color(theme::fg())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use perch_github::{
        Anchor, Author, CommentMessage, Item, ItemSubject, PullRequest, ThreadOrigin,
    };

    use crate::backend::Command;
    use crate::review::threads::ThreadFace;
    use crate::state::AppState;

    fn named(id: ViewId, name: &str) -> Option<ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| named(c, name))
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no {name}"));
        let at = id.get_visual_rect_no_clip().center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 12,
            title: "Fix".into(),
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

    /// A pull request with one thread from GitHub whose comment suggests
    /// `body` on line 40, which its hunk has as `let c = 2;`.
    fn state_with_suggestion(body: &str) -> ReviewState {
        let mut item = Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-7".into();
        item.subject = Some(ItemSubject::PullRequest(pull()));
        let state = ReviewState::new();
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull())));
        let at: jiff::Timestamp = "2026-09-20T09:00:00Z".parse().unwrap();
        let thread = CommentThread {
            id: "t1".into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::GitHub,
            github_id: Some(7),
            anchor: Some(Anchor {
                path: "src/manifest.rs".into(),
                start_line: 40,
                line: 40,
                side: Side::Right,
                commit_sha: "head-7".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: Some("PRRT_1".into()),
            can_resolve: true,
            created_at: at,
            messages: vec![CommentMessage {
                id: "m1".into(),
                thread_id: "t1".into(),
                origin: ThreadOrigin::GitHub,
                author: Author::GitHubUser("rohan".into()),
                body: body.into(),
                created_at: at,
                run: None,
                github_id: Some(7),
                node_id: Some("PRRC_7".into()),
                html_url: None,
                diff_hunk: Some("@@ -39,2 +39,2 @@\n a\n-b\n+let c = 2;".into()),
                reactions: Default::default(),
            }],
        };
        state.comment_threads.set(Arc::new(vec![thread]));
        state
    }

    /// Apply suggestion opens the popover with GitHub's message; Commit
    /// changes sends the change against the lines it replaces and waits;
    /// a refusal is said in the popover, which stays, and a commit puts
    /// it away.
    #[test]
    fn apply_suggestion_commits_from_its_popover() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_suggestion("```suggestion\nlet c = 3;\n```");
        let (backend, mut commands) = Backend::recording();
        let view = crate::review::threads::messages_view(
            app,
            state,
            Rc::new(backend),
            "t1".into(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 800.0);
        settle(&mut harness);

        click(&mut harness, "apply-suggestion");
        assert!(is_open(), "the popover opens");
        let headline = named(harness.root_id(), "apply-suggestion-headline").expect("no headline");
        assert!(
            harness.is_focused(headline),
            "the message takes the keyboard"
        );
        let popover = named(harness.root_id(), "apply-suggestion-popover").unwrap();
        let button = named(harness.root_id(), "apply-suggestion").unwrap();
        let (panel, under) = (
            popover.get_visual_rect_no_clip(),
            button.get_visual_rect_no_clip(),
        );
        assert_eq!(
            panel.x1.round(),
            under.x1.round(),
            "hung from the button's right edge"
        );
        assert!(panel.y0 >= under.y1, "under the button");

        // Enter in the message commits, as on github.com.
        harness.dispatch_event(floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Named(NamedKey::Enter),
            code: Code::Enter,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        }));
        settle(&mut harness);
        let token = match commands.try_recv() {
            Ok(Command::CommitSuggestion {
                item_id,
                token,
                commit,
            }) => {
                assert_eq!(item_id, "item-7");
                assert_eq!(
                    commit,
                    Commit {
                        thread_id: "t1".into(),
                        path: "src/manifest.rs".into(),
                        original: Original {
                            first: 40,
                            lines: vec!["let c = 2;".into()]
                        },
                        lines: vec!["let c = 3;".into()],
                        headline: "Update src/manifest.rs".into(),
                        body: String::new(),
                    }
                );
                token
            }
            other => panic!("nothing committed: {other:?}"),
        };
        // Out: a press outside does not put it away.
        harness.click(5.0, 5.0);
        settle(&mut harness);
        assert!(is_open(), "kept while the commit is out");

        state.settle_suggestion_commit(
            token,
            Err("The lines this suggestion replaces have changed".into()),
        );
        settle(&mut harness);
        assert!(
            named(harness.root_id(), "apply-suggestion-error").is_some(),
            "the refusal is said"
        );
        assert!(is_open());

        click(&mut harness, "apply-suggestion-commit");
        let token = match commands.try_recv() {
            Ok(Command::CommitSuggestion { token, .. }) => token,
            other => panic!("not sent again: {other:?}"),
        };
        state.settle_suggestion_commit(token, Ok(()));
        settle(&mut harness);
        assert!(!is_open(), "committed, it goes");
        assert!(named(harness.root_id(), "apply-suggestion-popover").is_none());
    }

    /// A press outside or Escape puts it away before anything is sent.
    #[test]
    fn a_press_outside_puts_the_popover_away() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_suggestion("```suggestion\nlet c = 3;\n```");
        let (backend, mut commands) = Backend::recording();
        let view = crate::review::threads::messages_view(
            app,
            state,
            Rc::new(backend),
            "t1".into(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 800.0);
        settle(&mut harness);
        click(&mut harness, "apply-suggestion");
        assert!(is_open());
        harness.click(5.0, 790.0);
        settle(&mut harness);
        assert!(!is_open());
        assert!(commands.try_recv().is_err(), "nothing sent");
    }

    /// No change, or a thread resolved, leaves the button off; a closed
    /// pull request, or an outdated thread, has none.
    #[test]
    fn apply_suggestion_is_off_or_gone_where_github_has_it_so() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_with_suggestion("```suggestion\nlet c = 2;\n```");
        let (backend, mut commands) = Backend::recording();
        let backend = Rc::new(backend);
        let view = crate::review::threads::messages_view(
            app,
            state,
            backend.clone(),
            "t1".into(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 800.0);
        settle(&mut harness);
        click(&mut harness, "apply-suggestion");
        assert!(!is_open(), "no change: off");
        assert!(commands.try_recv().is_err());

        let mut closed = pull();
        closed.state = "closed".into();
        state.pull_request.set(Some(Arc::new(closed)));
        settle(&mut harness);
        assert!(
            named(harness.root_id(), "apply-suggestion").is_none(),
            "closed: gone"
        );

        let root = TestRoot::new();
        let state = state_with_suggestion("```suggestion\nlet c = 3;\n```");
        state
            .comment_threads
            .update(|t| Arc::make_mut(t)[0].outdated = true);
        let view = crate::review::threads::messages_view(
            app,
            state,
            backend,
            "t1".into(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 800.0);
        settle(&mut harness);
        assert!(named(harness.root_id(), "suggested-change").is_some());
        assert!(
            named(harness.root_id(), "apply-suggestion").is_none(),
            "outdated: gone"
        );
    }

    #[test]
    fn the_popover_hangs_under_the_button_from_its_right_edge() {
        let window = Size::new(1000.0, 800.0);
        let size = Size::new(400.0, 240.0);
        let button = Rect::new(700.0, 100.0, 820.0, 130.0);
        assert_eq!(place(button, size, window), (420.0, 134.0));
        let near_left = Rect::new(20.0, 100.0, 140.0, 130.0);
        assert_eq!(place(near_left, size, window).0, EDGE, "kept in the window");
        let low = Rect::new(700.0, 700.0, 820.0, 730.0);
        assert_eq!(
            place(low, size, window),
            (420.0, 456.0),
            "above without room below"
        );
    }
}
