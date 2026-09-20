//! Window layout.

pub mod agents;
pub mod items;
pub mod notifications;
pub mod sidebar;
pub mod status;
pub mod titlebar;
pub mod widgets;

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::SignalUpdate;
use floem::text::FontWeight;
use floem::views::{dyn_container, Container, Decorators, Label, Stack};

use crate::backend::{Backend, Status};
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::sidebar_glyph;

/// The platform's command modifier.
const OS_MOD: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

pub fn app_view(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    state.review.set(Some(review));

    let review_backend = backend.clone();
    // Opening from the sidebar. An archived row is restored on the way,
    // which the backend treats as the same act.
    let open_item: items::OpenItem = Rc::new({
        let backend = backend.clone();
        move |item: &perch_github::Item| {
            state.pending_agent_navigation.set(None);
            state.agent_archived_owner.set(None);
            let message = match &item.subject {
                _ if item.is_archived() => "Checking out the files",
                Some(perch_github::ItemSubject::Issue(_)) => "Reading the issue",
                Some(perch_github::ItemSubject::PullRequest(_)) => "Reading the pull request",
                None => "Reading the branch",
            };
            let navigation_id = if item.is_archived() {
                backend.restore_item(&item.id)
            } else {
                backend.open_item(&item.id)
            };
            state.begin_item_review(review, message, &item.id, navigation_id);
        }
    });
    let panel_backend = backend.clone();
    let panel_open = open_item.clone();
    let review_open_item = open_item.clone();
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
    // And the runs the reader opened out of a fold, or folded again, go
    // to the store from here too. The item is read as the write is sent:
    // the effect runs on the write, before anything else can open another.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            use crate::review::model::RevealWrite;
            let writes = review.pending_reveals.get();
            if writes.is_empty() {
                return;
            }
            review.pending_reveals.set(Vec::new());
            let Some(item_id) = review
                .item
                .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
            else {
                return;
            };
            for write in writes {
                match write {
                    RevealWrite::Reveal {
                        path,
                        blob,
                        first,
                        last,
                    } => backend.reveal(&item_id, &path, &blob, first, last),
                    RevealWrite::Reset {
                        path,
                        blob,
                        first,
                        last,
                    } => backend.reset_reveals(&item_id, &path, &blob, first, last),
                }
            }
        });
    }

    let root = Stack::vertical((
        Stack::horizontal((
            dyn_container(
                move || state.sidebar_open.get(),
                move |open| {
                    if open {
                        // The handle is absolutely placed over the drawer's
                        // right edge, so the drawer is its positioning box.
                        floem::views::Container::new(Stack::horizontal((
                            sidebar::sidebar(state, panel_backend.clone(), panel_open.clone()),
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
            // The main area: the review view over the sidebar, or the
            // empty state, which on a fresh install with no token is what
            // to do about that.
            dyn_container(
                move || {
                    (
                        review.is_showing(),
                        state.status.with(|s| matches!(s, Status::NoToken { .. })),
                    )
                },
                move |(open, no_token)| {
                    if open {
                        crate::review::review_view(
                            state,
                            review,
                            review_backend.clone(),
                            review_open_item.clone(),
                        )
                        .into_any()
                    } else if no_token {
                        token_help(state).into_any()
                    } else {
                        widgets::centered_message(
                            SidebarIcon::Branch,
                            || "Nothing open".to_string(),
                            || {
                                "Pick a pull request, an issue, or a branch from the sidebar."
                                    .to_string()
                            },
                        )
                        .into_any()
                    }
                },
            )
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
        status::status_bar(state, review, backend.clone()),
        // The Agents inspector overlays the workspace. Keeping it outside the
        // review's keyed content preserves the open file, scroll position and
        // composer while app-wide work is inspected.
        agents::panel(state, review, backend),
        agents::completion_notice(state, capture_backend.clone()),
        // Over everything, and only while the drawer's edge is being dragged.
        sidebar::resize_cursor_sheet(state),
        agents::resize_cursor_sheet(state),
    ));
    let root_id = root.view_id();
    root
        // The keys the code editor would otherwise keep. The editor stops
        // every key once it has focus, and these are pressed right after
        // clicking in it, so they are caught on the way down to it rather
        // than on the way back up. Target as well, for when nothing has focus
        // and floem hands the key straight to whoever listens. Nothing here
        // runs while a text box has the keyboard.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE_AND_TARGET,
            },
            move |cx, key: &KeyboardEvent| {
                let key_backend = &capture_backend;
                // Cmd-Shift-A is app-wide, including while the editor or a
                // composer owns focus. Escape first backs out of details, then
                // closes the inspector on the next press.
                if key.code == Code::KeyA && key.modifiers == OS_MOD | Modifiers::SHIFT {
                    let opener = (!state.agents_open.get())
                        .then(|| focused_descendant(cx, root_id))
                        .flatten();
                    agents::toggle(state, key_backend, opener);
                    return EventPropagation::Stop;
                }
                if key.code == Code::Escape && key.modifiers.is_empty() && state.agents_open.get() {
                    if state.selected_agent_run.with(Option::is_some) {
                        agents::back_to_list(state);
                    } else {
                        agents::close(state, key_backend, true);
                    }
                    return EventPropagation::Stop;
                }
                let typing = state.search_focused.get()
                    || review.composer_focused.get()
                    || review.submit_review_open.get();
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
                            let has_files = review
                                .item
                                .with(|i| i.as_ref().is_some_and(|i| i.checkout.is_some()));
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
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && review.confirm_thread.with(Option::is_some)
                {
                    review.confirm_thread.set(None);
                    return EventPropagation::Stop;
                }
                // Escape closes the references panel when nothing else claims
                // it: a composer, or something open under a sidebar row.
                let claimed = review.composer.with(Option::is_some)
                    || state.renaming.with(Option::is_some)
                    || state.confirm_delete.with(Option::is_some);
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
                                match review
                                    .composer_box
                                    .get()
                                    .filter(|_| review.composer.with(Option::is_some))
                                {
                                    Some(id) => {
                                        id.scroll_to(None);
                                        id.request_focus();
                                    }
                                    None => review.open_composer(
                                        None,
                                        None,
                                        String::new(),
                                        crate::review::threads::defaults(state),
                                    ),
                                }
                                return EventPropagation::Stop;
                            }
                            let anchor = review.anchor_from_selection();
                            review.open_composer(
                                None,
                                anchor,
                                String::new(),
                                crate::review::threads::defaults(state),
                            );
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

                // `v` checks the open file as viewed, `]` and `[` go to the next
                // and previous change, `}` and `{` open the next and previous
                // unviewed file. Files only. The braces are shifted brackets:
                // the physical codes with Shift alone, so Cmd-[ stays back.
                if review.tab.get() == crate::review::model::ItemTab::Files {
                    let shifted = key.modifiers == Modifiers::SHIFT;
                    match key.code {
                        Code::BracketRight | Code::BracketLeft if key.modifiers.is_empty() => {
                            crate::review::tabs::jump_to_change(
                                review,
                                key.code == Code::BracketRight,
                            );
                            return EventPropagation::Stop;
                        }
                        Code::KeyV if key.modifiers.is_empty() => {
                            let item_id = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
                            let file = review.active.get().and_then(|path| {
                                review
                                    .changed_files
                                    .with(|f| f.iter().find(|f| f.path == path).cloned())
                            });
                            // A deletion has no content to have viewed.
                            if let (Some(item_id), Some(file)) = (item_id, file) {
                                if let Some(blob) = &file.head_blob {
                                    key_backend.set_viewed(
                                        &item_id,
                                        &file.path,
                                        blob,
                                        !review.is_viewed(&file),
                                    );
                                }
                            }
                            return EventPropagation::Stop;
                        }
                        Code::BracketRight | Code::BracketLeft if shifted => {
                            let active = review.active.get();
                            let forward = key.code == Code::BracketRight;
                            if let Some(path) = review.next_unviewed(active.as_deref(), forward) {
                                // The same path a click in the rail takes.
                                review.expanded.update(|expanded| {
                                    crate::review::tree::reveal(expanded, &path)
                                });
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
                    || state.confirm_delete.with(Option::is_some);
                if had {
                    state.renaming.set(None);
                    state.confirm_delete.set(None);
                    return EventPropagation::Stop;
                }
            }

            // `s` saves the sidebar row under the pointer, else the selected
            // row.
            if key.modifiers.is_empty()
                && !state.search_focused.get()
                && !review.composer_focused.get()
                && !review.submit_review_open.get()
                && key.code == Code::KeyS
            {
                let target = state
                    .hovered_item
                    .get()
                    .or_else(|| state.open_item.get())
                    .and_then(|id| state.item(&id));
                if let Some(item) = target {
                    key_backend.mark_saved(&item.id, !item.saved);
                    return EventPropagation::Stop;
                }
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
                // Show or hide the sidebar drawer.
                Code::Backslash => {
                    if !key.modifiers.contains(Modifiers::SHIFT) {
                        state.sidebar_open.update(|open| *open = !*open);
                        EventPropagation::Stop
                    } else {
                        EventPropagation::Continue
                    }
                }
                // Back to the sidebar.
                Code::BracketLeft => {
                    if review.is_showing() {
                        let navigation_id = key_backend.close_review();
                        state.begin_close(review, navigation_id);
                        return EventPropagation::Stop;
                    }
                    EventPropagation::Continue
                }
                // Cmd-1 is the sidebar's search. With the drawer shut it opens
                // the drawer, and the box takes the keyboard once it is built.
                Code::Digit1 => {
                    if state.sidebar_open.get() {
                        if let Some(id) = state.search_box.get() {
                            id.request_focus();
                        }
                    } else {
                        state.focus_search.set(true);
                        state.sidebar_open.set(true);
                    }
                    EventPropagation::Stop
                }
                _ => EventPropagation::Continue,
            }
        })
        .style(|s| {
            s.size_full()
                .flex_col()
                .position(floem::style::Position::Relative)
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .color(theme::FG)
                .background(theme::CANVAS)
        })
}

/// Floem exposes focus membership rather than its private current-focus slot.
/// Walking the existing root is cheap for the one app-wide shortcut and lets
/// the overlay restore the exact editor, composer, or sidebar control.
fn focused_descendant(
    cx: &floem::context::EventCx<'_>,
    root: floem::ViewId,
) -> Option<floem::ViewId> {
    if cx.window_state.is_focused(root) {
        return Some(root);
    }
    root.children()
        .into_iter()
        .find_map(|child| focused_descendant(cx, child))
}

/// What to do when no token resolved, in the main area's empty state. Built
/// the way `centered_message` is, so it reads as the same screen as
/// "Nothing open", with the two ways of signing in under the words.
fn token_help(state: AppState) -> impl IntoView {
    let detail = move || match state.status.get() {
        Status::NoToken { detail } => detail,
        _ => String::new(),
    };

    Container::new(
        Stack::vertical((
            Container::new(sidebar_glyph(SidebarIcon::Warning, theme::FG_SUBTLE)).style(|s| {
                s.size(48.0, 48.0)
                    .border_radius(24.0)
                    .background(theme::CANVAS_SUBTLE)
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "No GitHub token found".to_string()).style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .margin_bottom(6.0)
            }),
            Label::derived(detail).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_MUTED)
                    .max_width(420.0)
                    .margin_bottom(16.0)
            }),
            Stack::vertical((
                code_line("export GITHUB_TOKEN=ghp_..."),
                code_line("gh auth login"),
            ))
            .style(|s| s.gap(6.0)),
            Label::derived(|| "The token needs the notifications scope.".to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .margin_top(14.0)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(|s| s.size_full().items_center().justify_center().padding(40.0))
}

/// One command to copy, in the code font.
fn code_line(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_string()).style(|s| {
        s.font_family(theme::CODE_FONT_STACK.to_string())
            .font_size(theme::TEXT_META)
            .color(theme::FG)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .padding_horiz(10.0)
            .padding_vert(5.0)
    })
}
