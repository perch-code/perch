//! Window layout.

pub mod list;
pub mod row;
pub mod sidebar;
pub mod status;
pub mod titlebar;
pub mod toolbar;
pub mod widgets;
pub mod items;

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
    state.review.set(Some(review));

    let review_backend = backend.clone();
    let open_thread: list::OpenThread = Rc::new({
        let backend = backend.clone();
        move |repo: &str, kind: perch_github::ItemKind, number: u64, thread_id: &str| {
            // Showing the review view immediately, with its progress card,
            // beats leaving the inbox looking like the click did nothing.
            state.begin_review(
                review,
                match kind {
                    perch_github::ItemKind::PullRequest => "Reading the pull request",
                    perch_github::ItemKind::Issue => "Reading the issue",
                },
            );
            // Which notification this came from, so finishing the review can
            // dismiss it. The backend never needs to know; the click already
            // has it, and a round trip would only be a way of forgetting.
            review.thread_id.set(Some(thread_id.to_string()).filter(|id| !id.is_empty()));
            backend.open_subject(repo, kind, number);
        }
    });

    // Opening from the sidebar. An archived row is restored on the way,
    // which the backend treats as the same act.
    let open_item: items::OpenItem = Rc::new({
        let backend = backend.clone();
        move |item: &perch_github::Item| {
            let message = match &item.subject {
                _ if item.is_archived() => "Checking out the files",
                Some(perch_github::ItemSubject::Issue(_)) => "Reading the issue",
                Some(perch_github::ItemSubject::PullRequest(_)) => "Reading the pull request",
                None => "Reading the branch",
            };
            state.begin_review(review, message);
            state.open_item.set(Some(item.id.clone()));
            review.thread_id.set(None);
            if item.is_archived() {
                backend.restore_item(&item.id);
            } else {
                backend.open_item(&item.id);
            }
        }
    });
    let panel_backend = backend.clone();
    let panel_open = open_item.clone();
    let review_open_item = open_item.clone();

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
    let key_backend = backend.clone();
    let capture_backend = backend.clone();
    let references_backend = backend.clone();

    // Settings the sidebar asks to be remembered go through here, which is
    // where the backend handle is.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            if let Some((key, value)) = state.pending_meta.get() {
                state.pending_meta.set(None);
                backend.set_meta(&key, &value);
            }
        });
    }
    // Files thrown away by a scope change are asked for again here, for
    // the same reason.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let paths = state.reload.get();
            if !paths.is_empty() {
                state.reload.set(Vec::new());
                for path in paths {
                    backend.open_file(&path);
                }
            }
        });
    }

    Stack::vertical((
        Stack::horizontal((
            dyn_container(
                move || state.sidebar_open.get(),
                move |open| {
                    if open {
                        // The handle is absolutely placed over the drawer's
                        // right edge, so the drawer is its positioning box.
                        floem::views::Container::new(Stack::horizontal((
                            sidebar::sidebar(state, review, panel_backend.clone(), panel_open.clone()),
                            sidebar::resize_handle(state),
                        )))
                        .style(|s| {
                            s.height_full()
                                .flex_shrink(0.0_f32)
                                .position(floem::style::Position::Relative)
                        })
                        .into_any()
                    } else {
                        sidebar::collapsed_sidebar(state).into_any()
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
                            review_open_item.clone(),
                        )
                        .into_any()
                    } else if screen == Screen::Sidebar {
                        widgets::centered_message(
                            crate::icons::SidebarIcon::Branch,
                            || "Nothing open".to_string(),
                            || "Pick a pull request, an issue, or a branch from the sidebar.".to_string(),
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
        // Over everything, and only while the drawer's edge is being dragged.
        sidebar::resize_cursor_sheet(state),
    ))
    // The keys the code editor would otherwise keep. The editor stops
    // every key once it has focus, and these are pressed right after
    // clicking in it, so they are caught on the way down to it rather
    // than on the way back up. Target as well, for when nothing has focus
    // and floem hands the key straight to whoever listens. Nothing here
    // runs while a text box has the keyboard.
    .on_event_with_config(
        el::KeyDown,
        floem::context::EventCallbackConfig { phases: floem::context::Phases::CAPTURE_AND_TARGET },
        move |_, key: &KeyboardEvent| {
            let key_backend = &capture_backend;
            let typing = state.search_focused.get() || review.composer_focused.get();
            if typing || !review.is_open() {
                return EventPropagation::Continue;
            }
            // Cmd-Shift-1 and Cmd-Shift-2 are the item view's tabs. Here
            // rather than with the other Cmd keys because they are pressed
            // from the code as often as from anywhere.
            if key.modifiers == OS_MOD | Modifiers::SHIFT {
                match key.code {
                    Code::Digit1 => {
                        review.show_tab(crate::review::model::ItemTab::Conversation);
                        return EventPropagation::Stop;
                    }
                    Code::Digit2 => {
                        let has_files = review.item.with(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()));
                        if has_files {
                            review.show_tab(crate::review::model::ItemTab::Files);
                        }
                        return EventPropagation::Stop;
                    }
                    _ => {}
                }
            }
            // Shift-F12 asks for references to the cursor's symbol.
            if key.code == Code::F12 && key.modifiers == Modifiers::SHIFT {
                crate::review::references::find_at_cursor(review, &references_backend);
                return EventPropagation::Stop;
            }
            // Escape backs out of a thread's Delete first.
            if key.code == Code::Escape && key.modifiers.is_empty() && review.confirm_thread.with(Option::is_some) {
                review.confirm_thread.set(None);
                return EventPropagation::Stop;
            }
            // Escape closes the references panel when nothing else claims
            // it: a composer, or something open under a sidebar row.
            let claimed = review.composer.with(Option::is_some)
                || state.renaming.with(Option::is_some)
                || state.confirm_delete.with(Option::is_some)
                || state.new_project_for.with(Option::is_some);
            if key.code == Code::Escape
                && key.modifiers.is_empty()
                && !claimed
                && review.references.with(Option::is_some)
            {
                review.references.set(None);
                return EventPropagation::Stop;
            }

            // `a` opens a composer for the selection, or for the worktree when
            // no file is open. Caught on the way down for the same reason as
            // Shift-F12: the code editor keeps every key once it has focus,
            // and `a` is pressed right after selecting lines in it.
            if key.modifiers.is_empty() {
                match key.code {
                    Code::KeyA => {
                        // On Conversation the composer is laid in place at the
                        // end of the column, about the item as a whole: `a`
                        // opens it, or takes the keyboard back to it.
                        if review.tab.get() == crate::review::model::ItemTab::Conversation {
                            match review.composer_box.get().filter(|_| review.composer.with(Option::is_some)) {
                                Some(id) => {
                                    id.scroll_to(None);
                                    id.request_focus();
                                }
                                None => review.open_composer(None, None, String::new(), crate::review::threads::defaults(state)),
                            }
                            return EventPropagation::Stop;
                        }
                        let anchor = review.anchor_from_selection();
                        review.open_composer(None, anchor, String::new(), crate::review::threads::defaults(state));
                        return EventPropagation::Stop;
                    }
                    // Close an empty composer. One with a question in it stays,
                    // so an Escape meant for the selection cannot throw the
                    // question away; the composer's own close button does that.
                    Code::Escape
                        if review.composer.with(Option::is_some)
                            && review.composer_draft.with(|d| d.trim().is_empty()) =>
                    {
                        review.close_composer();
                        return EventPropagation::Stop;
                    }
                    _ => {}
                }
            }

            // `v` checks the open file as viewed, `}` and `{` open the next and
            // previous unviewed file. Files only. The braces are shifted
            // brackets: the physical codes with Shift alone, so Cmd-[ stays
            // back.
            if review.tab.get() == crate::review::model::ItemTab::Files {
                let shifted = key.modifiers == Modifiers::SHIFT;
                match key.code {
                    Code::KeyV if key.modifiers.is_empty() => {
                        let item_id = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
                        let file = review.active.get().and_then(|path| {
                            review.changed_files.with(|f| f.iter().find(|f| f.path == path).cloned())
                        });
                        // A deletion has no content to have viewed.
                        if let (Some(item_id), Some(file)) = (item_id, file) {
                            if let Some(blob) = &file.head_blob {
                                key_backend.set_viewed(&item_id, &file.path, blob, !review.is_viewed(&file));
                            }
                        }
                        return EventPropagation::Stop;
                    }
                    Code::BracketRight | Code::BracketLeft if shifted => {
                        let active = review.active.get();
                        let forward = key.code == Code::BracketRight;
                        if let Some(path) = review.next_unviewed(active.as_deref(), forward) {
                            // The same path a click in the rail takes.
                            review.expanded.update(|expanded| crate::review::tree::reveal(expanded, &path));
                            if review.open_tab(&path) {
                                key_backend.open_file(&path);
                            }
                        }
                        return EventPropagation::Stop;
                    }
                    _ => {}
                }
            }
            EventPropagation::Continue
        },
    )
    // Window-level shortcuts. Key events bubble from whatever has focus, so
    // a handler on the root view catches them wherever the reader is.
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        // Escape puts away whatever the sidebar has open under a row.
        if key.code == Code::Escape && key.modifiers.is_empty() {
            let had = state.renaming.with(Option::is_some)
                || state.confirm_delete.with(Option::is_some)
                || state.new_project_for.with(Option::is_some);
            if had {
                state.renaming.set(None);
                state.confirm_delete.set(None);
                state.new_project_for.set(None);
                return EventPropagation::Stop;
            }
        }

        // `e` and `s` on the sidebar row under the pointer, else on the
        // selected row, as on inbox rows. The inbox's own rows come first
        // when it is the screen and nothing in the sidebar is hovered.
        if key.modifiers.is_empty()
            && !state.search_focused.get()
            && !review.composer_focused.get()
            && matches!(key.code, Code::KeyE | Code::KeyS)
        {
            let target = state
                .hovered_item
                .get()
                .or_else(|| {
                    let inbox_showing = !review.is_showing() && state.screen.get() == Screen::Inbox;
                    if inbox_showing { None } else { state.open_item.get() }
                })
                .and_then(|id| state.item(&id));
            if let Some(item) = target {
                match key.code {
                    Code::KeyE => key_backend.mark_item(&item.id, None, Some(!item.is_done())),
                    _ => key_backend.mark_item(&item.id, Some(!item.saved), None),
                }
                return EventPropagation::Stop;
            }
        }

        // Bare letters act on the inbox, the way they do on the web. They are
        // only shortcuts when nothing is being typed into and the inbox is
        // what is on screen; the root handler cannot see focus, so the search
        // box reports its own.
        if key.modifiers.is_empty()
            && !review.is_showing()
            && state.screen.get() == Screen::Inbox
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
            // Show or hide the drawer, or with shift, the thread column.
            Code::Backslash => {
                if key.modifiers.contains(Modifiers::SHIFT) {
                    if review.is_open() {
                        review.threads_open.update(|open| *open = !*open);
                    }
                } else {
                    state.sidebar_open.update(|open| *open = !*open);
                }
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
            // Cmd-1 is the sidebar's search; Cmd-2 is the inbox.
            Code::Digit1 => {
                if state.sidebar_open.get() && (review.is_showing() || state.screen.get() == Screen::Sidebar) {
                    if let Some(id) = state.search_box.get() {
                        id.request_focus();
                    }
                } else {
                    state.show_screen(review, Screen::Sidebar);
                    state.sidebar_open.set(true);
                }
                EventPropagation::Stop
            }
            Code::Digit2 => {
                state.show_screen(review, Screen::Inbox);
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
