//! Window layout.

pub mod list;
pub mod row;
pub mod sidebar;
pub mod status;
pub mod toolbar;
pub mod widgets;
pub mod worktrees;

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::SignalUpdate;
use floem::views::{Decorators, Stack, dyn_container};

use crate::backend::Backend;
use crate::review::model::ReviewState;
use crate::state::{AppState, Screen};
use crate::theme;

/// The platform's command modifier.
const OS_MOD: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

pub fn app_view(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Narrowing the list drops the selection. This is the reactive scope, so
    // it is where that watch belongs.
    state.watch_filters();

    let review_backend = backend.clone();
    let open_thread: list::OpenThread = Rc::new({
        let backend = backend.clone();
        move |repo: &str, number: u64, thread_id: &str| {
            review.preparing.set(crate::review::model::Preparing::Working {
                message: "Reading the pull request".into(),
                percent: None,
            });
            // Showing the review view immediately, with its progress card,
            // beats leaving the inbox looking like the click did nothing.
            state.begin_review(review);
            // Which notification this came from, so finishing the review can
            // dismiss it. The backend never needs to know; the click already
            // has it, and a round trip would only be a way of forgetting.
            review.thread_id.set(Some(thread_id.to_string()).filter(|id| !id.is_empty()));
            backend.open_pull_request(repo, number);
        }
    });

    // Opening from the worktrees list. An archived row is restored on the
    // way, which the backend treats as the same act.
    let open_worktree: worktrees::OpenWorktree = Rc::new({
        let backend = backend.clone();
        move |worktree: &perch_github::Worktree| {
            review.preparing.set(crate::review::model::Preparing::Working {
                message: if worktree.is_archived() {
                    "Checking out the files".into()
                } else {
                    "Reading the pull request".into()
                },
                percent: None,
            });
            state.begin_review(review);
            review.thread_id.set(None);
            if worktree.is_archived() {
                backend.restore_worktree(&worktree.id);
            } else {
                backend.open_worktree(&worktree.id);
            }
        }
    });
    let list_backend = backend.clone();

    // Every way of marking a thread lands here, so each of them costs the
    // backend one write and the UI one event.
    let on_mark: list::OnMark = Rc::new({
        let backend = backend.clone();
        move |ids: Vec<String>, action: list::MarkAction| match action {
            list::MarkAction::Saved(saved) => backend.set_saved(ids, saved),
            list::MarkAction::Done(done) => backend.set_done(ids, done),
        }
    });
    let key_mark = on_mark.clone();
    let review_mark = on_mark.clone();

    Stack::vertical((
        Stack::horizontal((
            // The rail is always on screen. The panel beside it is what
            // comes and goes, and the rail's own entry is the control.
            sidebar::activity_rail(state, review),
            dyn_container(
                move || state.sidebar_open.get(),
                move |open| {
                    if open {
                        sidebar::nav_panel(state, review).into_any()
                    } else {
                        floem::views::Empty::new().into_any()
                    }
                },
            ),
            // The review view replaces the inbox in place, so the sidebar and
            // the status line stay where they were.
            dyn_container(
                move || (review.is_showing(), state.screen.get()),
                move |(open, screen)| {
                    if open {
                        crate::review::review_view(
                            state,
                            review,
                            review_backend.clone(),
                            review_mark.clone(),
                        )
                        .into_any()
                    } else if screen == Screen::Worktrees {
                        worktrees::worktrees_screen(
                            state,
                            list_backend.clone(),
                            open_worktree.clone(),
                        )
                        .into_any()
                    } else {
                        Stack::vertical((
                            toolbar::toolbar(state),
                            list::list(state, open_thread.clone(), on_mark.clone()),
                        ))
                            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full().flex_col())
                            .into_any()
                    }
                },
            )
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
        status::status_bar(state, review, backend),
    ))
    // Window-level shortcuts. Key events bubble from whatever has focus, so
    // a handler on the root view catches them wherever the reader is.
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        // Bare letters act on the inbox, the way they do on the web. They are
        // only shortcuts when nothing is being typed into and the inbox is
        // what is on screen; the root handler cannot see focus, so the search
        // box reports its own.
        if key.modifiers.is_empty()
            && !review.is_showing()
            && state.screen.get() == Screen::Threads
            && !state.search_focused.get()
            && matches!(key.code, Code::KeyE | Code::KeyS | Code::Escape)
        {
            if key.code == Code::Escape {
                if state.selection_len() == 0 {
                    return EventPropagation::Continue;
                }
                state.clear_selection();
                return EventPropagation::Stop;
            }

            // A selection is what the reader last said they meant. Without
            // one, the row under the pointer is.
            //
            // The hover only moves when the pointer does, so it can still name
            // a row that has left the screen: one just dismissed, or one left
            // behind by opening a pull request. Acting on that would undo the
            // last keystroke instead of repeating it, so check it is still on
            // screen first.
            let targets = match state.selection_len() {
                0 => state
                    .hovered
                    .get()
                    .filter(|id| state.is_visible(id))
                    .map(|id| vec![id])
                    .unwrap_or_default(),
                _ => state.selection(),
            };
            if targets.is_empty() {
                return EventPropagation::Continue;
            }

            match key.code {
                Code::KeyE => {
                    let restoring = state.would_restore(&targets);
                    key_mark(targets, list::MarkAction::Done(!restoring));
                }
                _ => {
                    let unsaving = state.would_unsave(&targets);
                    key_mark(targets, list::MarkAction::Saved(!unsaving));
                }
            }
            state.clear_selection();
            return EventPropagation::Stop;
        }

        if !key.modifiers.contains(OS_MOD) {
            return EventPropagation::Continue;
        }
        // Physical key codes, so the shortcuts hold on any keyboard layout.
        match key.code {
            // Close the open file, as in any editor.
            Code::KeyW => {
                if let Some(path) = review.active.get() {
                    review.close_tab(&path);
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            }
            // Show or hide the drawer.
            Code::Backslash => {
                state.sidebar_open.update(|open| *open = !*open);
                EventPropagation::Stop
            }
            // Back to wherever the review was opened from.
            Code::BracketLeft => {
                if review.is_showing() {
                    state.leave_review(review);
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            }
            // The two sections, numbered the way an editor numbers its
            // panels.
            Code::Digit1 => {
                state.show_screen(review, Screen::Threads);
                EventPropagation::Stop
            }
            Code::Digit2 => {
                state.show_screen(review, Screen::Worktrees);
                EventPropagation::Stop
            }
            _ => EventPropagation::Continue,
        }
    })
    .style(|s| {
        s.size_full()
            .flex_col()
            .font_family(theme::FONT_STACK.to_string())
            .font_size(theme::TEXT_BODY)
            .color(theme::FG)
            .background(theme::CANVAS)
    })
}
