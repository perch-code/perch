//! Window layout.

pub mod agents;
pub mod drawer;
pub mod items;
pub mod nav_row;
pub mod notifications;
pub mod sidebar;
pub mod status;
pub mod titlebar;
pub mod widgets;

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::SignalUpdate;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::{Backend, Status};
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::drawer::DrawerView;
use crate::ui::widgets::sidebar_glyph;

/// The platform's command modifier.
pub(crate) const OS_MOD: Modifiers = if cfg!(target_os = "macos") {
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
    // Files thrown away by a scope change, and those of the tabs an item
    // opens on again, are asked for here, for the same reason.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let paths = state.reload.get();
            if !paths.is_empty() {
                state.reload.set(Vec::new());
                for path in paths {
                    crate::review::load_file(&backend, &path);
                }
            }
        });
    }
    // The names comments and answers are drawn with are looked up from
    // here as well.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let asks = review.pending_names.get();
            if asks.is_empty() {
                return;
            }
            review.pending_names.set(Vec::new());
            for (token, names) in asks {
                backend.names(token, names);
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

    let main_pane = floem::reactive::Memo::new(move |_| {
        (
            review.is_showing(),
            state.status.with(|s| matches!(s, Status::NoToken { .. })),
        )
    });

    // The main area: the review view over the sidebar, or the empty
    // state, which on a fresh install with no token is what to do about
    // that. A memo, because a signal tells its readers about every write
    // and the container builds its child again on each: the status is
    // written on every poll, and a page built again is a scroll back at
    // the top.
    // The Agents inspector's list, loaded as the drawer opens on it. Wired
    // once, here, since the drawer is built again with the pane.
    agents::wire(state, backend.clone());
    let empty_backend = backend.clone();
    let pane = dyn_container(
        move || main_pane.get(),
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
                empty_pane(state, review, empty_backend.clone(), token_help(state)).into_any()
            } else {
                empty_pane(
                    state,
                    review,
                    empty_backend.clone(),
                    widgets::centered_message(
                        SidebarIcon::Branch,
                        || "Nothing open".to_string(),
                        || "Pick a pull request, an issue, or a branch from the sidebar.".to_string(),
                    ),
                )
                .into_any()
            }
        },
    )
    // A zero basis: the pane takes what the drawers leave, and taffy
    // never measures the whole item view unconstrained to find out how
    // wide it would like to be.
    .debug_name("main-pane")
    .style(|s| {
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .height_full()
    });
    let pane_id = pane.view_id();
    // Opening a file from Cmd-P's palette, which the window's keys do.
    let go_to_file = crate::review::open_file(review, backend.clone());

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
                        // Shut, the drawer is gone altogether: the main pane
                        // has the whole window, and its bar takes the lights.
                        Empty::new().into_any()
                    }
                },
            ),
            // The drawer on the right, the open item's files or the Agents
            // inspector, is the pane's: under its bars, beside its body.
            pane,
        ))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_height(0.0)
                .width_full()
                .position(floem::style::Position::Relative)
        }),
        status::status_bar(state, review, backend.clone()),
        agents::completion_notice(state, capture_backend.clone()),
        // An `@`'s list, over the composer it is typed in.
        crate::review::mention::popup(state, review),
        // Cmd-P's palette, over everything but a drag's cursor.
        crate::review::go_to::palette(state, review, backend.clone()),
        // Over everything, and only while a drawer's edge is being dragged.
        sidebar::resize_cursor_sheet(state),
        drawer::resize_cursor_sheet(state),
    ));
    let root_id = root.view_id();
    // macOS's scroll bar setting, read again whenever it changes.
    let scroller_style = floem::ext_event::create_trigger();
    crate::scrollers::watch(scroller_style);
    root
        // The window's width, for the drawer on the right to know whether
        // it can be docked. Read off the root's own layout, which is the
        // window's size, after every layout; written only when it changes,
        // and a change restyles the drawer within the same update.
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            let width = root_id.get_layout_rect().width();
            if width > 0.0 && state.window_width.get_untracked() != width {
                state.window_width.set(width);
            }
            let height = root_id.get_layout_rect().height();
            if height > 0.0 && state.window_height.get_untracked() != height {
                state.window_height.set(height);
            }
            // The pane's row starts at the window's left edge, so where
            // the pane is in the row is where it is in the window.
            let pane = pane_id.get_layout_rect();
            if state.main_pane.get_untracked() != (pane.x0, pane.x1) {
                state.main_pane.set((pane.x0, pane.x1));
            }
        })
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
                // Cmd-P's palette is modal: while it is up, its keys are
                // its own, whatever has the keyboard, and the rest are its
                // box's.
                if review.go_to.open.get() {
                    return crate::review::go_to::key(review, &go_to_file, key);
                }
                // Cmd-P puts it up, app-wide, as the drawer keys are; Escape
                // gives the keyboard back to whatever had it.
                if key.code == Code::KeyP
                    && key.modifiers == OS_MOD
                    && crate::review::go_to::available(review)
                {
                    crate::review::go_to::open(review, focused_descendant(cx, root_id));
                    return EventPropagation::Stop;
                }
                // Cmd-Shift-E and Cmd-Shift-A are the drawer on the right,
                // on the files and on the Agents inspector, app-wide,
                // including while the editor or a composer owns focus.
                // Whatever had the keyboard gets it back when the drawer
                // shuts, so it is noted as the drawer opens; a swap from
                // one to the other keeps the note. Files is not offered
                // for an item without files.
                if key.modifiers == OS_MOD | Modifiers::SHIFT {
                    let view = match key.code {
                        Code::KeyA => Some(DrawerView::Agents),
                        Code::KeyE if state.files_available() => Some(DrawerView::Files),
                        _ => None,
                    };
                    if let Some(view) = view {
                        let opener = state
                            .drawer_shown_untracked()
                            .is_none()
                            .then(|| focused_descendant(cx, root_id))
                            .flatten();
                        drawer::toggle(state, key_backend, view, opener);
                        return EventPropagation::Stop;
                    }
                }
                // Escape first backs out of a run's details, then shuts the
                // inspector on the next press.
                if key.code == Code::Escape && key.modifiers.is_empty() && state.agents_open() {
                    if state.selected_agent_run.with(Option::is_some) {
                        agents::back_to_list(state);
                    } else {
                        drawer::close(state, key_backend, true);
                    }
                    return EventPropagation::Stop;
                }
                let typing = state.search_focused.get()
                    || review.composer_focused.get()
                    || review.submit_review_open.get()
                    || review.review_launcher_open.get();
                // The find bar's keys, from the code and from the bar alike:
                // Cmd-F puts the bar up over the code, or gives it the
                // keyboard again; Cmd-G and Cmd-Shift-G walk the matches,
                // putting the bar up without the keyboard when it is down;
                // Alt-Cmd-C, -W and -R are its toggles. Files only, with a
                // file that has an editor to find anything in; a text box
                // elsewhere keeps its own keys.
                let finding = !typing
                    && review.is_open()
                    && review.tab.get() == crate::review::model::ItemTab::Files
                    && review.active.with(Option::is_some)
                    && review.drawn_picture().is_none();
                if finding {
                    let cmd = key.modifiers == OS_MOD;
                    let cmd_shift = key.modifiers == OS_MOD | Modifiers::SHIFT;
                    let alt_cmd = key.modifiers == OS_MOD | Modifiers::ALT && review.find_open.get();
                    match key.code {
                        Code::KeyF if cmd => {
                            crate::review::find::open(review, true);
                            return EventPropagation::Stop;
                        }
                        Code::KeyG if cmd || cmd_shift => {
                            if !review.find_open.get() {
                                crate::review::find::open(review, false);
                            }
                            crate::review::find::walk(review, cmd);
                            return EventPropagation::Stop;
                        }
                        Code::KeyC if alt_cmd => {
                            review.find_options.update(|o| o.match_case = !o.match_case);
                            return EventPropagation::Stop;
                        }
                        Code::KeyW if alt_cmd => {
                            review.find_options.update(|o| o.whole_word = !o.whole_word);
                            return EventPropagation::Stop;
                        }
                        Code::KeyR if alt_cmd => {
                            review.find_options.update(|o| o.regex = !o.regex);
                            return EventPropagation::Stop;
                        }
                        _ => {}
                    }
                }
                let typing = typing || review.find_focused.get();
                if typing || !review.is_open() {
                    return EventPropagation::Continue;
                }
                // Cmd-Shift-1 shows Conversation, opening its tab again if it
                // was closed, as the strip's chat tile does; Cmd-Shift-2 goes
                // back to the open file. Here rather than with the other Cmd
                // keys because they are pressed from the code as often as
                // from anywhere.
                if key.modifiers == OS_MOD | Modifiers::SHIFT {
                    match key.code {
                        Code::Digit1 => {
                            review.show_tab(crate::review::model::ItemTab::Conversation);
                            return EventPropagation::Stop;
                        }
                        Code::Digit2 => {
                            if review.has_files() && review.active.with(Option::is_some) {
                                review.show_tab(crate::review::model::ItemTab::Files);
                            }
                            return EventPropagation::Stop;
                        }
                        _ => {}
                    }
                }
                // Cmd-] and Cmd-[ step through the tabs, wrapping at the
                // ends, as in VS Code. With none to step through, Cmd-[
                // goes on to be the way back to the sidebar.
                if key.modifiers == OS_MOD
                    && matches!(key.code, Code::BracketLeft | Code::BracketRight)
                    && crate::review::tabs::step_tab(
                        review,
                        key_backend,
                        key.code == Code::BracketRight,
                    )
                {
                    return EventPropagation::Stop;
                }
                // The navigation keys, VS Code's: F12 goes to the definition
                // of the caret's symbol, Cmd-F12 to its implementations,
                // Shift-F12 asks for its references and Shift-Alt-H for its
                // call hierarchy; Ctrl-- and Ctrl-Shift-- walk the jumps
                // back and forward.
                if key.code == Code::F12 && key.modifiers.is_empty() {
                    crate::review::navigation::definition_at_cursor(review, &references_backend);
                    return EventPropagation::Stop;
                }
                if key.code == Code::F12 && key.modifiers == OS_MOD {
                    crate::review::navigation::implementations_at_cursor(review, &references_backend);
                    return EventPropagation::Stop;
                }
                if key.code == Code::F12 && key.modifiers == Modifiers::SHIFT {
                    crate::review::references::find_at_cursor(review, &references_backend);
                    return EventPropagation::Stop;
                }
                if key.code == Code::KeyH && key.modifiers == Modifiers::SHIFT | Modifiers::ALT {
                    crate::review::hierarchy::show_at_cursor(review, &references_backend);
                    return EventPropagation::Stop;
                }
                if key.code == Code::Minus && key.modifiers == Modifiers::CONTROL {
                    review.go_back();
                    return EventPropagation::Stop;
                }
                if key.code == Code::Minus && key.modifiers == Modifiers::CONTROL | Modifiers::SHIFT {
                    review.go_forward();
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
                // Then puts the find bar away, from the code; the bar takes
                // its own Escape. A bar out of sight, on Conversation, is
                // not what Escape is about.
                if key.code == Code::Escape && key.modifiers.is_empty() && finding && review.find_open.get() {
                    crate::review::find::close(review);
                    return EventPropagation::Stop;
                }
                // Escape closes the panel under the editor, references or
                // the call hierarchy, when nothing else claims it: a
                // composer, or something open under a sidebar row.
                let claimed = review.composer.with(Option::is_some)
                    || state.renaming.with(Option::is_some)
                    || state.confirm_delete.with(Option::is_some)
                    || state.confirm_loss.with(Option::is_some);
                if key.code == Code::Escape && key.modifiers.is_empty() && !claimed && review.panel_open() {
                    review.close_panel();
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
                        // Take back the question under the composer's box, the
                        // text staying where it is.
                        Code::Escape if review.composer_confirming.get() => {
                            review.composer_confirming.set(false);
                            return EventPropagation::Stop;
                        }
                        // Close an empty composer, or one editing a draft, whose
                        // words are safe in the cache. One with a question in it
                        // stays, so an Escape meant for the selection cannot throw
                        // the question away; the composer's own close button does that.
                        Code::Escape
                            if review.composer.with(|c| {
                                c.as_ref().is_some_and(|c| c.editing.is_some())
                            }) || (review.composer.with(Option::is_some)
                                && review.composer_draft.with(|d| d.trim().is_empty())) =>
                        {
                            review.close_composer();
                            return EventPropagation::Stop;
                        }
                        _ => {}
                    }
                }

                // Cmd-W closes Conversation's tab while it shows, as it
                // closes a file's.
                if key.code == Code::KeyW
                    && key.modifiers == OS_MOD
                    && review.tab.get() == crate::review::model::ItemTab::Conversation
                    && review.conversation_open.get()
                {
                    review.close_conversation();
                    return EventPropagation::Stop;
                }
                // Cmd-W closes the open file, as in any editor; `v` checks it
                // as viewed, `]` and `[` go to the next and previous change,
                // `}` and `{` open the next and previous unviewed file. Files
                // only. The braces are shifted brackets: the physical codes
                // with Shift alone, so Cmd-[ steps through the tabs.
                if review.tab.get() == crate::review::model::ItemTab::Files {
                    let shifted = key.modifiers == Modifiers::SHIFT;
                    match key.code {
                        Code::KeyW if key.modifiers == OS_MOD => {
                            if let Some(path) = review.active.get() {
                                review.close_tab(&path);
                                return EventPropagation::Stop;
                            }
                        }
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
                                if review.open_tab(&path) {
                                    key_backend.open_file(&path);
                                }
                                review.show_in_tree(&path);
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
            // None of these while Cmd-P's palette is up: it is modal.
            if review.go_to.open.get() {
                return EventPropagation::Continue;
            }
            // Escape puts away whatever the sidebar has open under a row.
            if key.code == Code::Escape && key.modifiers.is_empty() {
                let had = state.renaming.with(Option::is_some)
                    || state.confirm_delete.with(Option::is_some)
                    || state.confirm_loss.with(Option::is_some);
                if had {
                    state.renaming.set(None);
                    state.confirm_delete.set(None);
                    state.confirm_loss.set(None);
                    return EventPropagation::Stop;
                }
            }

            // `s` saves the sidebar row under the pointer, else the selected
            // row.
            if key.modifiers.is_empty()
                && !state.search_focused.get()
                && !review.composer_focused.get()
                && !review.submit_review_open.get()
                && !review.review_launcher_open.get()
                && !review.find_focused.get()
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
                // Show or hide the sidebar drawer.
                Code::Backslash => {
                    if !key.modifiers.contains(Modifiers::SHIFT) {
                        state.sidebar_open.update(|open| *open = !*open);
                        EventPropagation::Stop
                    } else {
                        EventPropagation::Continue
                    }
                }
                // Back to the sidebar, when there are no tabs for Cmd-[ to
                // step through. The capture handler's Stop does not keep
                // the key from here when the root is its target.
                Code::BracketLeft => {
                    if review.is_showing() && !crate::review::tabs::has_tabs(review) {
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
        .style(move |s| {
            scroller_style.track();
            s.size_full()
                .flex_col()
                .position(floem::style::Position::Relative)
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_body())
                .color(theme::FG)
                .background(theme::CANVAS)
                // A selection over a label, a fenced block's or a title's,
                // in the same blue as one over prose and in an editor.
                .custom_style_class(|s: floem::views::LabelCustomStyle| {
                    s.selection_color(theme::SELECTION_BG)
                })
                // Every scroll's bars as macOS's own: when it overlays
                // them, there while the view scrolls and while the pointer
                // is on one, and fading out once it stops.
                .custom_style_class(|s: floem::views::scroll::ScrollCustomStyle| {
                    s.show_bars_when_idle(!crate::scrollers::overlaid())
                })
        })
}

/// The main pane with nothing open in it. While the drawer is shut the
/// lights and the drawer control have nowhere else to be, so a bare bar
/// carries them; while it is open the drawer's strip has them and the
/// message runs to the top. The drawer on the right can still show the
/// agents, beside the message.
fn empty_pane(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    message: impl IntoView + 'static,
) -> impl IntoView {
    Stack::vertical((
        dyn_container(
            move || state.sidebar_open.get(),
            move |open| {
                if open {
                    Empty::new().into_any()
                } else {
                    titlebar::title_bar(Container::new(sidebar::lights_lane(state, 0.0)))
                        .debug_name("empty bar")
                        .style(|s| {
                            s.height(theme::TITLEBAR_HEIGHT)
                                .width_full()
                                .flex_shrink(0.0_f32)
                                .items_center()
                        })
                        .into_any()
                }
            },
        )
        .style(|s| s.width_full().flex_shrink(0.0_f32)),
        Stack::horizontal((
            Container::new(message).debug_name("pane-body").style(|s| {
                s.flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .min_width(0.0)
                    .height_full()
            }),
            drawer::panel(state, review, backend),
        ))
        .style(|s| {
            s.width_full()
                .flex_grow(1.0_f32)
                .min_height(0.0)
                .position(floem::style::Position::Relative)
        }),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
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
                    .font_weight(theme::weight_emphasis())
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
            .font_size(theme::CODE_FONT_SIZE)
            .color(theme::FG)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .padding_horiz(10.0)
            .padding_vert(5.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    /// The window with nothing open and the drawer shut: the lights and
    /// the drawer control have only the main pane's bare bar to sit in.
    #[test]
    fn with_nothing_open_and_the_drawer_shut_a_bare_bar_carries_the_lights_and_the_way_back() {
        let state = AppState::new();
        let review = ReviewState::new();
        state.sidebar_open.set(false);
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();

        let bar = named(harness.root_id(), "empty bar").expect("no bare bar");
        let bounds = bar.get_visual_rect_no_clip();
        assert_eq!(
            (bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            (0.0, 0.0, 1400.0, theme::TITLEBAR_HEIGHT)
        );
        let toggle = named(harness.root_id(), "drawer toggle")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(toggle.x0, theme::ACTIVITY_RAIL_WIDTH, "after the lights");

        // Beside the lights is the bar's own surface: the window drags
        // from it.
        let hit = harness
            .element_id_at(theme::ACTIVITY_RAIL_WIDTH / 2.0, bounds.center().y)
            .expect("nothing under the lane");
        assert_eq!(hit.owning_id(), bar, "the lane took the press");

        harness.click(toggle.center().x, toggle.center().y);
        harness.rebuild();
        harness.paint();
        assert!(
            state.sidebar_open.get(),
            "the control did not open the drawer"
        );
        assert!(
            named(harness.root_id(), "empty bar").is_none(),
            "the drawer's strip has the lights now"
        );
    }

    /// A label anywhere in the window selects in the editor's blue, not
    /// floem's teal: the root sets the label class's colour.
    #[test]
    fn a_selection_over_a_label_is_the_editors_blue() {
        use floem::peniko::Brush;
        use floem::views::editor::SelectionColor;
        let state = AppState::new();
        let review = ReviewState::new();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();

        let label = named(harness.root_id(), "Label: ").expect("no label in the window");
        assert_eq!(
            harness.get_computed_style(label).get(SelectionColor),
            Brush::Solid(theme::SELECTION_BG)
        );
    }

    /// The window on Files with one changed file open in the editor.
    fn window_with_a_file_open() -> (HeadlessHarness, ReviewState) {
        use crate::review::model::{ItemTab, Phase};
        use perch_github::{Item, ItemCheckout};
        use std::sync::Arc;

        let root = TestRoot::new();
        let state = AppState::new();
        let review = ReviewState::new();
        let now = "2026-09-18T09:59:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "wt".into();
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            source: None,
        });
        review.item.set(Some(Arc::new(item)));
        review.ready.set(true);
        review.set_phase(Phase::Ready);
        review.tab.set(ItemTab::Files);
        review.composer_autofocus.set(false);
        let file = Arc::new(crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "fn alpha() {}\nfn beta() {\n    alpha();\n}\n".into(),
            "fn alpha() {}\nfn old() {\n    alpha();\n}\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));
        review.open_tab("src/x.rs");
        review.finish_load("src/x.rs", file);

        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, review)
    }

    /// A key as the window receives it.
    fn key(code: Code, key: Key, modifiers: Modifiers) -> floem::event::Event {
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

    /// Cmd-W as the window receives it.
    fn cmd_w() -> floem::event::Event {
        key(Code::KeyW, Key::Character("w".into()), OS_MOD)
    }

    /// Cmd-Shift-E and Cmd-Shift-A are the drawer on the right, on the
    /// files and on the agents: each opens the drawer on its own, swaps
    /// it from the other, and shuts it from itself. The pane beside it
    /// keeps the open file and its editor through all of it, and the
    /// editor gets the keyboard back when the drawer shuts.
    #[test]
    fn the_drawer_keys_open_swap_and_shut_the_drawer_around_the_editor() {
        let (mut harness, review) = window_with_a_file_open();
        let code = named(harness.root_id(), "tinted editor").expect("no editor in the window");
        let rect = code.get_visual_rect_no_clip();
        harness.click(rect.x0 + 200.0, rect.y0 + 40.0);
        harness.rebuild();
        harness.paint();
        let editor = harness
            .element_id_at(rect.x0 + 200.0, rect.y0 + 40.0)
            .expect("nothing under the click")
            .owning_id();
        assert!(harness.is_focused(editor));
        let drawer = named(harness.root_id(), "right-drawer").expect("no drawer in the window");
        assert_eq!(drawer.get_visual_rect_no_clip().width(), 0.0, "shut, it takes no room");

        let files = key(Code::KeyE, Key::Character("e".into()), OS_MOD | Modifiers::SHIFT);
        let agents = key(Code::KeyA, Key::Character("a".into()), OS_MOD | Modifiers::SHIFT);
        harness.dispatch_event(files.clone());
        harness.rebuild();
        harness.paint();
        assert!(
            named(harness.root_id(), "files-panel").is_some(),
            "Cmd-Shift-E did not open the drawer on the files"
        );
        let drawer = named(harness.root_id(), "right-drawer")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(drawer.x1, 1400.0);
        assert!(drawer.width() >= theme::DRAWER_MIN_WIDTH);
        // The same editor, moved over rather than built again.
        let code_now = named(harness.root_id(), "tinted editor").unwrap();
        assert_eq!(code_now, code, "the editor was rebuilt");
        assert!(code.get_visual_rect_no_clip().x1 <= drawer.x0, "the editor is under the drawer");
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));

        harness.dispatch_event(agents.clone());
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files-panel").is_none());
        assert!(named(harness.root_id(), "agents-panel").is_some(), "Cmd-Shift-A did not swap");

        harness.dispatch_event(files.clone());
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files-panel").is_some(), "Cmd-Shift-E did not swap back");

        harness.dispatch_event(files);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files-panel").is_none(), "Cmd-Shift-E did not shut it");
        assert_eq!(
            named(harness.root_id(), "right-drawer").unwrap().get_visual_rect_no_clip().width(),
            0.0
        );
        assert_eq!(code.get_visual_rect_no_clip().x1, 1400.0, "the editor has the window again");
        assert!(harness.is_focused(editor), "the editor did not get the keyboard back");
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));

        // Escape shuts the agents, as it did when they were an overlay.
        harness.dispatch_event(agents);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "agents-panel").is_some());
        harness.dispatch_event(key(Code::Escape, Key::Named(NamedKey::Escape), Modifiers::empty()));
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "agents-panel").is_none());
    }

    /// Cmd-W closes the open file from the code too. The editor keeps
    /// every key once a click in it has given it the keyboard, so the
    /// shortcut is caught on the way down, not on the way back up.
    #[test]
    fn cmd_w_closes_the_open_file_while_the_editor_has_the_keyboard() {
        let (mut harness, review) = window_with_a_file_open();
        let code = named(harness.root_id(), "tinted editor").expect("no editor in the window");
        let rect = code.get_visual_rect_no_clip();
        // A click on the code, as one placing the caret before reading.
        let (x, y) = (rect.x0 + 200.0, rect.y0 + 40.0);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        let code = harness.element_id_at(x, y).expect("nothing under the click");
        assert!(
            harness.is_focused(code.owning_id()),
            "the press did not give the editor the keyboard"
        );
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));

        harness.dispatch_event(cmd_w());
        harness.rebuild();
        harness.paint();
        assert_eq!(review.active.get(), None, "the file is still open");
        assert!(review.tabs.with(Vec::is_empty), "the tab is still there");
        // Conversation's tab, the one left, shows in its place; the next
        // Cmd-W closes it.
        assert_eq!(review.tab.get(), crate::review::model::ItemTab::Conversation);
        harness.dispatch_event(cmd_w());
        harness.rebuild();
        assert!(!review.conversation_open.get());

        // With nothing open, the key is left for whoever else wants it.
        harness.dispatch_event(cmd_w());
        harness.rebuild();
        assert_eq!(review.active.get(), None);
        assert!(!review.conversation_open.get());
    }

    /// Cmd-] and Cmd-[ step through the tabs in the strip's order,
    /// Conversation's then the files', wrapping at both ends, from the
    /// code as from anywhere. With no tab left, Cmd-[ is the way back to
    /// the sidebar again.
    #[test]
    fn cmd_brackets_step_through_the_tabs_and_wrap() {
        use crate::review::model::ItemTab;
        let (mut harness, review) = window_with_a_file_open();
        review.open_tab("src/y.rs");
        review.open_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        let next = || key(Code::BracketRight, Key::Character("]".into()), OS_MOD);
        let previous = || key(Code::BracketLeft, Key::Character("[".into()), OS_MOD);
        let showing = |review: &ReviewState| match review.tab.get() {
            ItemTab::Conversation => "chat".to_string(),
            ItemTab::Files => review.active.get().unwrap_or_default(),
        };

        let mut seen = Vec::new();
        for _ in 0..4 {
            harness.dispatch_event(next());
            harness.rebuild();
            seen.push(showing(&review));
        }
        assert_eq!(seen, ["src/y.rs", "chat", "src/x.rs", "src/y.rs"]);

        let mut seen = Vec::new();
        for _ in 0..4 {
            harness.dispatch_event(previous());
            harness.rebuild();
            seen.push(showing(&review));
        }
        assert_eq!(seen, ["src/x.rs", "chat", "src/y.rs", "src/x.rs"]);

        // With every tab closed, Cmd-[ leaves the item.
        review.close_tab("src/x.rs");
        review.close_tab("src/y.rs");
        review.close_conversation();
        harness.rebuild();
        assert!(review.is_showing());
        harness.dispatch_event(previous());
        harness.rebuild();
        assert!(!review.is_showing(), "Cmd-[ did not go back to the sidebar");
    }

    /// On Conversation, Cmd-W closes Conversation's tab, as it would a
    /// file's, and leaves the file's be: the file shows in its place. The
    /// file's goes on the next.
    #[test]
    fn cmd_w_on_conversation_closes_it_and_leaves_the_files_tab() {
        use crate::review::model::ItemTab;
        let (mut harness, review) = window_with_a_file_open();
        review.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(cmd_w());
        harness.rebuild();
        harness.paint();
        assert!(!review.conversation_open.get(), "Conversation's tab is still there");
        assert_eq!(review.tab.get(), ItemTab::Files);
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));
        assert_eq!(review.tabs.with(Vec::len), 1);
        assert!(named(harness.root_id(), "chat tab").is_none());
        assert!(named(harness.root_id(), "tinted editor").is_some(), "the file is not showing");

        harness.dispatch_event(cmd_w());
        harness.rebuild();
        assert!(review.tabs.with(Vec::is_empty), "the file's tab is still there");
    }

    /// The review launcher's instructions are a text box: `a` typed there
    /// is an `a`, not the key that opens a composer and takes the keyboard.
    #[test]
    fn the_review_launchers_box_keeps_the_keys_the_window_would_take() {
        let (mut harness, review) = window_with_a_file_open();
        let control = named(harness.root_id(), "review-launch-control")
            .expect("no review launcher in the bar")
            .get_visual_rect_no_clip();
        harness.click(control.center().x, control.center().y);
        harness.rebuild();
        harness.paint();
        assert!(review.review_launcher_open.get(), "the click did not open it");

        let rect = named(harness.root_id(), "review-instructions")
            .expect("no instructions box")
            .get_visual_rect_no_clip();
        harness.click(rect.center().x, rect.center().y);
        harness.rebuild();
        harness.paint();
        let editor = harness
            .element_id_at(rect.center().x, rect.center().y)
            .expect("nothing under the click")
            .owning_id();
        assert!(harness.is_focused(editor));

        for (ch, code) in [
            ("P", Code::KeyP),
            ("l", Code::KeyL),
            ("e", Code::KeyE),
            ("a", Code::KeyA),
            ("s", Code::KeyS),
            ("e", Code::KeyE),
        ] {
            let modifiers = if ch == "P" { Modifiers::SHIFT } else { Modifiers::empty() };
            harness.dispatch_event(key(code, Key::Character(ch.into()), modifiers));
            harness.rebuild();
            harness.paint();
        }
        assert!(review.composer.with(Option::is_none), "`a` opened a composer");
        assert!(harness.is_focused(editor), "the box lost the keyboard");
        assert_eq!(review.review_launcher_instructions.get(), "Please");
    }
}
