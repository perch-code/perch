//! Window layout.

pub mod agents;
pub mod browse;
pub mod drawer;
pub mod feedback;
pub mod invite;
pub mod items;
pub mod menu;
pub mod nav_row;
pub mod new_work;
pub mod notifications;
pub mod open_box;
pub mod sidebar;
pub mod titlebar;
pub mod toast;
pub mod update;
pub mod usage;
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
            // An item open in another window is looked at there.
            if crate::windows::raise_holding(&item.id, backend.window()) {
                return;
            }
            state.pending_agent_navigation.set(None);
            state.agent_archived_owner.set(None);
            let message = match &item.subject {
                _ if item.is_archived() => "Checking out the files",
                Some(perch_github::ItemSubject::Issue(_)) => "Reading the issue",
                Some(perch_github::ItemSubject::PullRequest(_)) => "Reading the pull request",
                None => "Reading the worktree",
            };
            let navigation_id = if item.is_archived() {
                backend.restore_item(&item.id)
            } else {
                backend.open_item(&item.id)
            };
            state.begin_item_review(review, message, &item.id, navigation_id);
            if let Some(subject) = &item.subject {
                crate::review::notified::follow_subject(
                    state,
                    subject.repo_full_name(),
                    subject.kind(),
                    subject.number(),
                );
            }
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
    // And the commits they link are shown on Files from here.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let Some(scope) = review.pending_scope.get() else {
                return;
            };
            review.pending_scope.set(None);
            let item_id = review
                .item
                .with_untracked(|i| i.as_ref().map(|i| i.id.clone()));
            if let Some(item_id) = item_id {
                backend.set_scope(&item_id, scope);
            }
        });
    }
    // What the review has to tell the reader goes up in the popup from
    // here, which is where the app state is.
    floem::reactive::Effect::new(move |_| {
        if let Some(text) = review.pending_notice.get() {
            review.pending_notice.set(None);
            state.notify(text, crate::state::Tone::Warning);
        }
    });
    floem::reactive::Effect::new(move |_| {
        if let Some(offer) = review.pending_offer.get() {
            review.pending_offer.set(None);
            state.offer_server(offer, true);
        }
    });
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let starts = review.pending_server_starts.get();
            if starts.is_empty() {
                return;
            }
            review.pending_server_starts.set(Vec::new());
            for kind in starts {
                backend.start_language_server_here(kind);
            }
        });
    }
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let ready = review.pending_ready.get();
            if ready.is_empty() {
                return;
            }
            review.pending_ready.set(Vec::new());
            // The asks read signals of their own; this effect is the
            // queue's alone.
            floem::reactive::Effect::untrack(|| {
                for kind in ready {
                    review.ask_again(&backend, kind);
                }
            });
        });
    }
    // An unread notification opened goes to what it is about.
    crate::review::notified::wire(review);
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
            state.new_work.open.get(),
        )
    });
    // A repository Add repository put in the config is chosen for new
    // work once the backend has it.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            if let Some(identity) = state.added_repo.get() {
                state.added_repo.set(None);
                new_work::choose_repo(state, &backend, &identity);
            }
        });
    }

    // The main area: the review view over the sidebar, or the empty
    // state, which on a fresh install with no token is what to do about
    // that. A memo, because a signal tells its readers about every write
    // and the container builds its child again on each: the status is
    // written on every poll, and a page built again is a scroll back at
    // the top.
    // The clock that moves the agents' elapsed times. Wired once, here,
    // since the rows are built again with the pane.
    agents::wire(state);
    let empty_backend = backend.clone();
    let pane = dyn_container(
        move || main_pane.get(),
        move |(open, no_token, new_work)| {
            if new_work {
                pane_with_bar(
                    state,
                    review,
                    empty_backend.clone(),
                    new_work::bar_title(),
                    new_work::close_button(state),
                    new_work::new_work_view(state, empty_backend.clone()),
                )
                .into_any()
            } else if open {
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
                        || {
                            "Pick a pull request, an issue, or a worktree from the sidebar."
                                .to_string()
                        },
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
            )
            // Over the main pane, for the footer's popovers, which hang
            // out over it.
            .style(|s| s.z_index(1)),
            // The drawer on the right, the open item's tree, is the pane's:
            // under Code's tabs, beside the file.
            pane,
        ))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_height(0.0)
                .width_full()
                .position(floem::style::Position::Relative)
        }),
        toast::toast(state, review, capture_backend.clone()),
        // An `@`'s list, over the composer it is typed in.
        crate::review::mention::popup(state, review),
        // The close button's menu, over the composer it hangs from.
        crate::review::close_menu::popup(state, review, backend.clone()),
        // Cmd-P's palette, over everything but a drag's cursor.
        crate::review::go_to::palette(state, review, backend.clone()),
        // Open…'s box, as modal as the palette.
        open_box::overlay(state),
        // Start work's confirmation, as modal.
        crate::review::start_work::overlay(state, backend.clone()),
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
        // A press outside the Agents dropdown or the usage popover shuts
        // it, and goes on to whatever it landed on. Caught on the way down,
        // as most controls stop their presses.
        .on_event_with_config(
            el::PointerDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |cx, _: &PointerButtonEvent| {
                dismiss_popovers(review, cx.hit_path.as_deref());
                EventPropagation::Continue
            },
        )
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
                if state.open_box.open.get() {
                    return open_box::key(state, key_backend, key);
                }
                if state.start_work_confirm.with(Option::is_some) {
                    return crate::review::start_work::key(state, key_backend, key);
                }
                // Cmd-N opens the new-work view, from anywhere.
                if key.code == Code::KeyN && key.modifiers == OS_MOD {
                    new_work::open(state, key_backend);
                    return EventPropagation::Stop;
                }
                // Cmd-[ shuts it, with what was typed kept, from its
                // message box too.
                if key.code == Code::BracketLeft
                    && key.modifiers == OS_MOD
                    && state.new_work.open.get_untracked()
                {
                    state.new_work.open.set(false);
                    return EventPropagation::Stop;
                }
                // Cmd-P puts it up, app-wide, as the tree's key is; Escape
                // gives the keyboard back to whatever had it.
                if key.code == Code::KeyP
                    && key.modifiers == OS_MOD
                    && crate::review::go_to::available(review)
                {
                    crate::review::go_to::open(review, focused_descendant(cx, root_id));
                    return EventPropagation::Stop;
                }
                // Cmd-Shift-A opens the Agents dropdown from the bar, and
                // shuts it, from anywhere; whatever had the keyboard gets it
                // back as it shuts.
                if key.modifiers == OS_MOD | Modifiers::SHIFT && key.code == Code::KeyA {
                    let opener = (!review.agents_dropdown_open.get_untracked())
                        .then(|| focused_descendant(cx, root_id))
                        .flatten();
                    agents::dropdown::toggle(review, key_backend, opener);
                    return EventPropagation::Stop;
                }
                // Cmd-Shift-E is the tree in the drawer on the right, from
                // the editor or a composer too. On Code it opens and shuts;
                // from Conversation it goes to Code with the tree open. An
                // item without files has none. Whatever had the keyboard
                // gets it back when the tree shuts, so it is noted as it
                // opens.
                if key.modifiers == OS_MOD | Modifiers::SHIFT
                    && key.code == Code::KeyE
                    && review.is_open()
                    && review.has_files()
                {
                    let opener = (!floem::reactive::Effect::untrack(|| state.tree_shown()))
                        .then(|| focused_descendant(cx, root_id))
                        .flatten();
                    if review.tab.get_untracked() == crate::review::model::ItemTab::Code {
                        drawer::toggle(state, key_backend, opener);
                    } else {
                        floem::reactive::Effect::batch(|| {
                            review.show_tab(crate::review::model::ItemTab::Code);
                            drawer::show(state, key_backend, opener);
                        });
                    }
                    return EventPropagation::Stop;
                }
                // Escape shuts the Agents dropdown wherever the keyboard is.
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && review.agents_dropdown_open.get_untracked()
                {
                    agents::dropdown::close(review, true);
                    return EventPropagation::Stop;
                }
                // And the usage popover.
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && review.usage_open.get_untracked()
                {
                    usage::close(review, true);
                    return EventPropagation::Stop;
                }
                // And Send Feedback's dropdown.
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && review.feedback_open.get_untracked()
                {
                    feedback::close(review, true);
                    return EventPropagation::Stop;
                }
                // And the invite popover.
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && review.invite_open.get_untracked()
                {
                    invite::close(review, true);
                    return EventPropagation::Stop;
                }
                // Page Up and Page Down move Conversation or the open file
                // by a page, Cmd-Up and Cmd-Down to its top and its end.
                // The page keys work from the code, the page or a composer:
                // a composer is a few lines, and the page is what there is
                // to page through. A composer, or any text box, keeps
                // Cmd-Up and Cmd-Down, the start and end of its text.
                // Caught here, as the editors
                // would take them for half a screen and for the caret alone.
                // A file with no editor, a picture or one too big to show,
                // has nowhere to go.
                let boxed = state.search_focused.get()
                    || review.review_open.get()
                    || review.find_focused.get();
                use crate::review::model::Paging;
                let paging = match key.code {
                    Code::PageUp if key.modifiers.is_empty() => Some(Paging::Up),
                    Code::PageDown if key.modifiers.is_empty() => Some(Paging::Down),
                    Code::ArrowUp if key.modifiers == OS_MOD => Some(Paging::Top),
                    Code::ArrowDown if key.modifiers == OS_MOD => Some(Paging::End),
                    _ => None,
                }
                .filter(|p| {
                    matches!(p, Paging::Up | Paging::Down)
                        || !(review.composer_focused.get() || text_box_focused(cx, root_id))
                })
                .filter(|_| !boxed && review.is_open());
                if let Some(paging) = paging {
                    let showing = match review.tab.get() {
                        crate::review::model::ItemTab::Conversation => true,
                        crate::review::model::ItemTab::Code => {
                            review.active_file().is_some_and(|f| !f.truncated)
                                && review.drawn_picture().is_none()
                        }
                    };
                    if showing {
                        review.paging.set(Some(paging));
                        return EventPropagation::Stop;
                    }
                }
                let typing = state.search_focused.get()
                    || review.composer_focused.get()
                    || review.review_open.get();
                // The find bar's keys, from the page and from the bar alike:
                // Cmd-F puts the bar up over the code or Conversation's
                // page, or gives it the keyboard again; Cmd-G and
                // Cmd-Shift-G walk the matches, putting the bar up without
                // the keyboard when it is down; Alt-Cmd-C, -W and -R are its
                // toggles. On Code, only with a file that has an editor to
                // find anything in; a text box elsewhere keeps its own keys.
                let finding = if typing || !review.is_open() {
                    None
                } else {
                    match review.tab.get() {
                        crate::review::model::ItemTab::Code => {
                            (review.active.with(Option::is_some)
                                && review.drawn_picture().is_none())
                            .then_some(crate::review::find::Pane::Code)
                        }
                        crate::review::model::ItemTab::Conversation => {
                            Some(crate::review::find::Pane::Conversation)
                        }
                    }
                };
                let find_open = match finding {
                    Some(crate::review::find::Pane::Code) => review.find_open.get(),
                    Some(crate::review::find::Pane::Conversation) => {
                        review.conversation_find_open.get()
                    }
                    None => false,
                };
                if let Some(pane) = finding {
                    let cmd = key.modifiers == OS_MOD;
                    let cmd_shift = key.modifiers == OS_MOD | Modifiers::SHIFT;
                    let alt_cmd = key.modifiers == OS_MOD | Modifiers::ALT && find_open;
                    match key.code {
                        Code::KeyF if cmd => {
                            crate::review::find::open(review, pane, true);
                            return EventPropagation::Stop;
                        }
                        Code::KeyG if cmd || cmd_shift => {
                            if !find_open {
                                crate::review::find::open(review, pane, false);
                            }
                            crate::review::find::walk(review, pane, cmd);
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
                // Any box that takes typing keeps the keys below: `a` typed
                // in the close-as-issue title is an `a`, not a composer.
                let typing = typing || review.find_focused.get() || text_box_focused(cx, root_id);
                if typing || !review.is_open() {
                    if key.code == Code::KeyW && key.modifiers == OS_MOD {
                        close_item_or_window(state, review, key_backend, root_id);
                        return EventPropagation::Stop;
                    }
                    return EventPropagation::Continue;
                }
                // Cmd-Shift-1 shows Conversation and Cmd-Shift-2 Code, as the
                // switch in the bar does. Here rather than with the other Cmd
                // keys because they are pressed from the code as often as
                // from anywhere.
                if key.modifiers == OS_MOD | Modifiers::SHIFT {
                    match key.code {
                        Code::Digit1 => {
                            review.show_tab(crate::review::model::ItemTab::Conversation);
                            return EventPropagation::Stop;
                        }
                        Code::Digit2 => {
                            if review.has_files() {
                                review.show_tab(crate::review::model::ItemTab::Code);
                            }
                            return EventPropagation::Stop;
                        }
                        _ => {}
                    }
                }
                // Cmd-} and Cmd-{ step through the tabs, wrapping at the
                // ends, as in Safari and Xcode. Cmd-[ stays the way back
                // to the sidebar.
                if key.modifiers == OS_MOD | Modifiers::SHIFT
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
                    crate::review::navigation::implementations_at_cursor(
                        review,
                        &references_backend,
                    );
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
                if key.code == Code::Minus && key.modifiers == Modifiers::CONTROL | Modifiers::SHIFT
                {
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
                // Then puts the find bar away, from the code or the page;
                // the bar takes its own Escape. A bar out of sight, on the
                // other tab, is not what Escape is about.
                if key.code == Code::Escape && key.modifiers.is_empty() && find_open {
                    if let Some(pane) = finding {
                        crate::review::find::close(review, pane);
                        return EventPropagation::Stop;
                    }
                }
                // Escape closes the panel under the editor, references or
                // the call hierarchy, when nothing else claims it: a
                // composer, or something open under a sidebar row.
                let claimed = review.composer.with(Option::is_some)
                    || state.renaming.with(Option::is_some)
                    || state.confirm_delete.with(Option::is_some)
                    || state.confirm_loss.with(Option::is_some);
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && !claimed
                    && review.panel_open()
                {
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
                        // Put the close menu away, or step back out of its search.
                        Code::Escape if review.close_menu.with(Option::is_some) => {
                            crate::review::close_menu::back(review);
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
                            if review
                                .composer
                                .with(|c| c.as_ref().is_some_and(|c| c.editing.is_some()))
                                || (review.composer.with(Option::is_some)
                                    && review.composer_draft.with(|d| d.trim().is_empty())) =>
                        {
                            review.close_composer();
                            return EventPropagation::Stop;
                        }
                        _ => {}
                    }
                }

                // Cmd-W closes the open file, as in any editor, then the
                // item, then the window; `v` checks it
                // as viewed, `]` and `[` go to the next and previous change,
                // `}` and `{` open the next and previous unviewed file. Files
                // only. The braces are shifted brackets: the physical codes
                // with Shift alone, so Cmd-{ steps through the tabs.
                if review.tab.get() == crate::review::model::ItemTab::Code {
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
                            if let (Some(item_id), Some(file)) = (item_id, file) {
                                if let Some(key) = file.viewed_key() {
                                    key_backend.set_viewed(
                                        &item_id,
                                        &file.path,
                                        key,
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
                // Cmd-W with no file to close closes the item, as the X does.
                if key.code == Code::KeyW && key.modifiers == OS_MOD {
                    close_item_or_window(state, review, key_backend, root_id);
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            },
        )
        // Window-level shortcuts. Key events bubble from whatever has focus, so
        // a handler on the root view catches them wherever the reader is.
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            // None of these while Cmd-P's palette, Open… or Start work's
            // confirmation is up: they are modal.
            if review.go_to.open.get()
                || state.open_box.open.get()
                || state.start_work_confirm.with(Option::is_some)
            {
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

            // `p` pins the sidebar row under the pointer, else the selected
            // row, or unpins it.
            if key.modifiers.is_empty()
                && !state.search_focused.get()
                && !review.composer_focused.get()
                && !review.review_open.get()
                && !review.find_focused.get()
                && key.code == Code::KeyP
            {
                let target = state
                    .hovered_item
                    .get()
                    .or_else(|| state.open_item.get())
                    .and_then(|id| state.item(&id));
                if let Some(item) = target {
                    key_backend.pin_item(&item.id, !item.is_pinned());
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
                // Back to the sidebar. Not Cmd-{, which steps through the
                // tabs and reaches here too when the root is its target.
                Code::BracketLeft if !key.modifiers.contains(Modifiers::SHIFT) => {
                    if review.is_showing() {
                        let navigation_id = key_backend.close_review();
                        state.begin_close(review, navigation_id);
                        return EventPropagation::Stop;
                    }
                    EventPropagation::Continue
                }
                // Cmd-1 is the sidebar's search. With the drawer shut it opens
                // the drawer, and the box takes the keyboard once it is built.
                // Not Cmd-Shift-1, which is Conversation's, and reaches here
                // too when the root is its target.
                Code::Digit1 if !key.modifiers.contains(Modifiers::SHIFT) => {
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
                .color(theme::fg())
                .background(theme::canvas())
                // A selection over a label, a fenced block's or a title's,
                // in the same blue as one over prose and in an editor.
                .custom_style_class(|s: floem::views::LabelCustomStyle| {
                    s.selection_color(theme::selection_bg())
                })
                // The caret in every field and editor. floem's own is 30%
                // black, which a dark window hides.
                .class(floem::views::TextInputClass, |s| {
                    s.cursor_color(theme::fg())
                })
                .class(floem::views::editor::view::EditorViewClass, |s| {
                    s.cursor_color(theme::fg())
                })
                // Every scroll's bars as macOS's own: when it overlays
                // them, there while the view scrolls and while the pointer
                // is on one, and fading out once it stops.
                .custom_style_class(|s: floem::views::scroll::ScrollCustomStyle| {
                    s.show_bars_when_idle(!crate::scrollers::overlaid())
                })
                .class(crate::scrollers::ToEdge, crate::scrollers::to_edge)
        })
}

/// The main pane with nothing open in it: a bar with the Agents button,
/// and the lights and the drawer control while the drawer is shut, then
/// the message.
fn empty_pane(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    message: impl IntoView + 'static,
) -> impl IntoView {
    let none = || Empty::new().style(|s| s.hide());
    pane_with_bar(state, review, backend, none(), none(), message)
}

/// The main pane under a bar like an item's with `title` where the item's
/// title goes and `close` where its X goes.
fn pane_with_bar(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    title: impl IntoView + 'static,
    close: impl IntoView + 'static,
    message: impl IntoView + 'static,
) -> impl IntoView {
    Stack::vertical((
        // A bar like an item's, with nothing open in it: the lights and
        // the drawer control while the drawer is shut, and the Agents
        // button where it sits on an item, so every item's runs are a
        // click away from here too.
        titlebar::title_bar(
            Stack::horizontal((
                Stack::horizontal((
                    sidebar::lights_lane(state, theme::REVIEW_HEADER_PADDING)
                        .style(move |s| s.apply_if(state.sidebar_open.get(), |s| s.hide())),
                    title,
                ))
                .style(|s| {
                    s.items_center()
                        .gap(8.0)
                        .flex_grow(1.0_f32)
                        .flex_basis(0.0)
                        .min_width(0.0)
                        .pointer_events_none()
                }),
                agents::dropdown::button(state, review, backend),
                close,
            ))
            .debug_name("empty bar"),
        )
        .style(|s| crate::review::header::bar_style(s).z_index(2)),
        Container::new(message)
            .debug_name("pane-body")
            .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
}

/// Cmd-W past the open file: close the item, as the X does, and with none
/// open, the window. All of it in the one handler, as a key whose target is
/// the root reaches the root's other handlers even once this one stops it.
/// The Dock icon and the Perch menu's New Window open another window.
fn close_item_or_window(
    state: AppState,
    review: ReviewState,
    backend: &Backend,
    root_id: floem::ViewId,
) {
    if review.is_showing() {
        let navigation_id = backend.close_review();
        state.begin_close(review, navigation_id);
    } else if let Some(window) = root_id.window_id() {
        floem::close_window(window);
    }
}

/// Shut the Agents dropdown and the usage popover when a press with
/// `hit_path` lands outside the button each hangs from.
fn dismiss_popovers(review: ReviewState, hit_path: Option<&[floem::ElementId]>) {
    let inside = |anchor: Option<floem::ViewId>| {
        let (Some(anchor), Some(leaf)) = (anchor, hit_path.and_then(|path| path.last())) else {
            return false;
        };
        std::iter::successors(Some(leaf.owning_id()), floem::ViewId::parent).any(|id| id == anchor)
    };
    if review.agents_dropdown_open.get_untracked()
        && !inside(review.agents_dropdown_anchor.get_untracked())
    {
        agents::dropdown::close(review, false);
    }
    if review.usage_open.get_untracked() && !inside(review.usage_anchor.get_untracked()) {
        usage::close(review, false);
    }
    if review.feedback_open.get_untracked() && !inside(review.feedback_anchor.get_untracked()) {
        feedback::close(review, false);
    }
    if review.invite_open.get_untracked() && !inside(review.invite_anchor.get_untracked()) {
        invite::close(review, false);
    }
}

/// Floem exposes focus membership rather than its private current-focus slot.
/// Walking the existing root is cheap for an app-wide shortcut, or a page
/// coming up, and lets the overlay restore the exact editor, composer, or
/// sidebar control.
pub(crate) fn focused_descendant(
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

/// Whether the keyboard is in a box that takes typing: a one-line box, or
/// an editor other than the code's, which is read-only and sits in a
/// `tinted editor`. Told apart by floem's names for its views, as every
/// form, composer and reply box is one of the two.
fn text_box_focused(cx: &floem::context::EventCx<'_>, root: floem::ViewId) -> bool {
    let Some(focused) = focused_descendant(cx, root) else {
        return false;
    };
    let name = focused.debug_name();
    if name.contains("TextInput") {
        return true;
    }
    name.contains("Editor View")
        && !std::iter::successors(focused.parent(), floem::ViewId::parent)
            .any(|id| id.debug_name().contains("tinted editor"))
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
            Container::new(sidebar_glyph(SidebarIcon::Warning, theme::fg_subtle)).style(|s| {
                s.size(48.0, 48.0)
                    .border_radius(24.0)
                    .background(theme::canvas_subtle())
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "No GitHub token found".to_string()).style(|s| {
                s.font_size(theme::TEXT_TITLE)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .margin_bottom(6.0)
            }),
            Label::derived(detail).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg_muted())
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
                    .color(theme::fg_subtle())
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
            .font_size(theme::PROSE_CODE_FONT_SIZE)
            .color(theme::fg())
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border_muted())
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

    /// The window with nothing open has a bar like an item's with the
    /// Agents button at its end; with the drawer shut, the lights and the
    /// drawer control sit in it too.
    #[test]
    fn with_nothing_open_the_bar_has_the_agents_and_with_the_drawer_shut_the_lights() {
        let state = AppState::new();
        let review = ReviewState::new();
        state.sidebar_open.set(false);
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();

        let bar = named(harness.root_id(), "empty bar").expect("no bar");
        let bounds = bar.get_visual_rect_no_clip();
        assert_eq!(
            (bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            (0.0, 0.0, 1400.0, theme::REVIEW_HEADER_HEIGHT)
        );
        let toggle = named(harness.root_id(), "drawer toggle")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(toggle.x0, theme::ACTIVITY_RAIL_WIDTH, "after the lights");
        let agents = named(harness.root_id(), "agents-button")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(
            agents.x1,
            1400.0 - theme::REVIEW_HEADER_PADDING,
            "at the bar's end"
        );

        // Beside the lights is the bar's own surface: the window drags
        // from it.
        let hit = harness
            .element_id_at(theme::ACTIVITY_RAIL_WIDTH / 2.0, bounds.center().y)
            .expect("nothing under the lane");
        assert_eq!(hit.owning_id(), bar, "the lane took the press");

        // The dropdown opens from the button with nothing open.
        harness.click(agents.center().x, agents.center().y);
        harness.rebuild();
        harness.paint();
        assert!(review.agents_dropdown_open.get());
        assert!(named(harness.root_id(), "\"No agent runs yet\"").is_some());
        harness.click(agents.center().x, agents.center().y);
        harness.rebuild();

        harness.click(toggle.center().x, toggle.center().y);
        harness.rebuild();
        harness.paint();
        assert!(
            state.sidebar_open.get(),
            "the control did not open the drawer"
        );
        let bar = named(harness.root_id(), "empty bar").unwrap();
        assert_eq!(
            bar.get_visual_rect_no_clip().x0,
            theme::SIDEBAR_WIDTH,
            "the bar starts at the drawer's edge"
        );
        assert_eq!(
            named(bar, "drawer toggle")
                .unwrap()
                .get_visual_rect_no_clip()
                .width(),
            0.0,
            "the drawer's strip has the lights now"
        );
    }

    /// New work is titled in the bar, where an item's title goes, and its
    /// X there shuts it, as Cmd-[ does.
    #[test]
    fn new_work_is_titled_in_the_bar_and_its_x_shuts_it() {
        let state = AppState::new();
        let review = ReviewState::new();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        state.new_work.open.set(true);
        harness.rebuild();
        harness.paint();

        let bar = named(harness.root_id(), "empty bar").expect("no bar");
        let title = named(bar, "new-work-title").expect("no title in the bar");
        let close = named(bar, "new-work-close")
            .expect("no X in the bar")
            .get_visual_rect_no_clip();
        let agents = named(bar, "agents-button")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(title.get_visual_rect_no_clip().x1 <= agents.x0);
        assert!(agents.x1 <= close.x0, "the X is last");
        assert_eq!(close.x1, 1400.0 - theme::REVIEW_HEADER_PADDING);

        harness.click(close.center().x, close.center().y);
        harness.rebuild();
        harness.paint();
        assert!(!state.new_work.open.get());
        assert!(named(harness.root_id(), "new-work-title").is_none());

        state.new_work.open.set(true);
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(key(Code::BracketLeft, Key::Character("[".into()), OS_MOD));
        harness.rebuild();
        assert!(!state.new_work.open.get(), "Cmd-[ shuts it too");
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
            Brush::Solid(theme::selection_bg())
        );
    }

    /// A pull request shown before its checkout: Code draws the
    /// checkout's progress where the files will be, then the editor once
    /// they are here, and the full-window card never comes up.
    #[test]
    fn code_shows_the_checkout_coming_under_a_shown_pull_request() {
        use crate::review::model::{ItemTab, Phase, Preparing};
        use perch_github::Item;
        use std::sync::Arc;

        let root = TestRoot::new();
        let state = AppState::new();
        let review = ReviewState::new();
        let mut item = Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = "pr".into();
        review.item.set(Some(Arc::new(item)));
        review.files_pending.set(Some(Preparing::Working {
            message: "Fetching the pull request".into(),
            percent: None,
        }));
        review.ready.set(true);
        review.set_phase(Phase::Ready);
        review.tab.set(ItemTab::Code);
        review.composer_autofocus.set(false);
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files preparing").is_some());
        assert!(named(harness.root_id(), "Label: \"Fetching the pull request\"").is_some());

        review.files_pending.set(Some(Preparing::Failed {
            message: "no such ref".into(),
        }));
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files preparing").is_none());
        assert!(named(harness.root_id(), "files failed").is_some());

        review.files_pending.set(None);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files failed").is_none());
        assert_eq!(review.phase.get(), Phase::Ready);
    }

    /// The window on Code with one changed file open in the editor.
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
            divergence: None,
            source: None,
        });
        review.item.set(Some(Arc::new(item)));
        review.ready.set(true);
        review.set_phase(Phase::Ready);
        review.tab.set(ItemTab::Code);
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

    /// Open…'s box comes up over the window with the keyboard in it, says
    /// what's wrong with an Enter it can't read, and goes on Escape.
    #[test]
    fn the_open_box_takes_the_keyboard_complains_and_goes_on_escape() {
        let state = AppState::new();
        let review = ReviewState::new();
        state.review.set(Some(review));
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "open-box-panel").is_none());

        open_box::show(state, None);
        harness.rebuild();
        harness.paint();
        harness.rebuild();
        harness.paint();
        let panel = named(harness.root_id(), "open-box-panel").expect("no box");
        assert!(panel.get_visual_rect_no_clip().width() > 0.0);
        let text = named(harness.root_id(), "open-box-text").unwrap();
        assert!(harness.is_focused(text), "the box has the keyboard");

        state.open_box.text.set("fence".into());
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert!(state.open_box.error.get().is_some());
        assert!(state.open_box.open.get());

        harness.dispatch_event(key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert!(!state.open_box.open.get());
        assert!(named(harness.root_id(), "open-box-panel").is_none());
    }

    /// Start work asks first: Escape cancels with nothing cut, and Enter
    /// cuts the branch.
    #[test]
    fn start_work_asks_before_cutting_a_worktree() {
        use crate::backend::Command;
        use crate::review::start_work::{ask, Pending};
        let state = AppState::new();
        let review = ReviewState::new();
        state.review.set(Some(review));
        let (backend, mut commands) = Backend::recording();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let settle = |harness: &mut HeadlessHarness| {
            harness.rebuild();
            harness.paint();
        };

        ask(state, Pending::Item("slatedb/slatedb#7".into()));
        settle(&mut harness);
        let panel = named(harness.root_id(), "start-work-confirm-panel").expect("no confirmation");
        assert!(panel.get_visual_rect_no_clip().width() > 0.0);
        let create = named(harness.root_id(), "start-work-create").unwrap();
        assert!(harness.is_focused(create), "Create has the keyboard");
        assert!(
            commands.try_recv().is_err(),
            "nothing cut before the answer"
        );

        harness.dispatch_event(key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert!(state.start_work_confirm.with(Option::is_none));
        assert!(named(harness.root_id(), "start-work-confirm-panel").is_none());
        assert!(commands.try_recv().is_err(), "cancelled, nothing cut");

        ask(state, Pending::Item("slatedb/slatedb#7".into()));
        settle(&mut harness);
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        settle(&mut harness);
        assert!(state.start_work_confirm.with(Option::is_none));
        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        let cut = sent.iter().any(
            |c| matches!(c, Command::StartWork { item_id, .. } if item_id == "slatedb/slatedb#7"),
        );
        assert!(cut, "Enter cuts the branch: {sent:?}");
    }

    /// Cmd-W as the window receives it.
    fn cmd_w() -> floem::event::Event {
        key(Code::KeyW, Key::Character("w".into()), OS_MOD)
    }

    /// Cmd-Shift-E opens the tree in the drawer on the right and shuts it,
    /// and Cmd-Shift-A opens the Agents dropdown and shuts it, both from
    /// the editor. The pane keeps the open file and its editor through all
    /// of it, and the editor gets the keyboard back each time.
    #[test]
    fn the_tree_and_agents_keys_work_around_the_editor() {
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
        assert_eq!(
            drawer.get_visual_rect_no_clip().width(),
            0.0,
            "shut, it takes no room"
        );

        let files = key(
            Code::KeyE,
            Key::Character("e".into()),
            OS_MOD | Modifiers::SHIFT,
        );
        let agents = key(
            Code::KeyA,
            Key::Character("a".into()),
            OS_MOD | Modifiers::SHIFT,
        );
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
        assert!(
            code.get_visual_rect_no_clip().x1 <= drawer.x0,
            "the editor is under the drawer"
        );
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));

        harness.dispatch_event(files);
        harness.rebuild();
        harness.paint();
        assert!(
            named(harness.root_id(), "files-panel").is_none(),
            "Cmd-Shift-E did not shut it"
        );
        assert_eq!(
            named(harness.root_id(), "right-drawer")
                .unwrap()
                .get_visual_rect_no_clip()
                .width(),
            0.0
        );
        assert_eq!(
            code.get_visual_rect_no_clip().x1,
            1400.0,
            "the editor has the window again"
        );
        assert!(
            harness.is_focused(editor),
            "the editor did not get the keyboard back"
        );
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));

        // Cmd-Shift-A opens the dropdown and Escape shuts it, giving the
        // keyboard back; so does Cmd-Shift-A again.
        harness.dispatch_event(agents.clone());
        harness.rebuild();
        harness.paint();
        assert!(
            review.agents_dropdown_open.get(),
            "Cmd-Shift-A did not open the dropdown"
        );
        assert!(named(harness.root_id(), "agents-dropdown").is_some());
        harness.dispatch_event(key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert!(!review.agents_dropdown_open.get());
        assert!(named(harness.root_id(), "agents-dropdown").is_none());
        assert!(
            harness.is_focused(editor),
            "Escape did not give the editor the keyboard back"
        );
        harness.dispatch_event(agents.clone());
        harness.rebuild();
        harness.dispatch_event(agents);
        harness.rebuild();
        assert!(!review.agents_dropdown_open.get());
        assert!(harness.is_focused(editor));
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));
    }

    /// Cmd-Shift-E from Conversation goes to Code with the tree open.
    #[test]
    fn the_tree_key_from_conversation_goes_to_code_with_the_tree() {
        use crate::review::model::ItemTab;
        let (mut harness, review) = window_with_a_file_open();
        review.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "files-panel").is_none());

        harness.dispatch_event(key(
            Code::KeyE,
            Key::Character("e".into()),
            OS_MOD | Modifiers::SHIFT,
        ));
        harness.rebuild();
        harness.paint();
        assert_eq!(review.tab.get(), ItemTab::Code);
        assert!(
            named(harness.root_id(), "files-panel").is_some(),
            "the tree did not open"
        );
        assert!(named(harness.root_id(), "tinted editor").is_some());
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
        let code = harness
            .element_id_at(x, y)
            .expect("nothing under the click");
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
        // Code stays, with nothing open, rather than going to Conversation.
        assert_eq!(review.tab.get(), crate::review::model::ItemTab::Code);
        assert_eq!(
            floem::take_close_window_event_count(harness.window_id()),
            0,
            "closing the file closed the window"
        );

        // With no file open, the key closes the item, as the X does, and
        // leaves the window.
        harness.dispatch_event(cmd_w());
        harness.rebuild();
        harness.paint();
        assert!(!review.is_showing(), "the item is still showing");
        assert_eq!(
            floem::take_close_window_event_count(harness.window_id()),
            0,
            "closing the item closed the window"
        );

        // With nothing open at all, it closes the window.
        harness.dispatch_event(cmd_w());
        harness.rebuild();
        assert_eq!(floem::take_close_window_event_count(harness.window_id()), 1);
    }

    /// Cmd-} and Cmd-{ step through Code's tabs in the strip's order,
    /// wrapping at both ends, from the code as from anywhere; from
    /// Conversation they go to the first or the last. Cmd-[ goes back to
    /// the sidebar, tabs or no.
    #[test]
    fn cmd_braces_step_through_the_tabs_and_wrap() {
        use crate::review::model::ItemTab;
        let (mut harness, review) = window_with_a_file_open();
        review.open_tab("src/y.rs");
        review.open_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        let next = || {
            key(
                Code::BracketRight,
                Key::Character("}".into()),
                OS_MOD | Modifiers::SHIFT,
            )
        };
        let previous = || {
            key(
                Code::BracketLeft,
                Key::Character("{".into()),
                OS_MOD | Modifiers::SHIFT,
            )
        };
        let showing = |review: &ReviewState| match review.tab.get() {
            ItemTab::Conversation => "conversation".to_string(),
            ItemTab::Code => review.active.get().unwrap_or_default(),
        };

        let mut seen = Vec::new();
        for _ in 0..4 {
            harness.dispatch_event(next());
            harness.rebuild();
            seen.push(showing(&review));
        }
        assert_eq!(seen, ["src/y.rs", "src/x.rs", "src/y.rs", "src/x.rs"]);

        let mut seen = Vec::new();
        for _ in 0..4 {
            harness.dispatch_event(previous());
            harness.rebuild();
            seen.push(showing(&review));
        }
        assert_eq!(seen, ["src/y.rs", "src/x.rs", "src/y.rs", "src/x.rs"]);

        // From Conversation, to the first tab, or the last; Conversation
        // is never stepped to.
        review.show_tab(ItemTab::Conversation);
        harness.dispatch_event(next());
        harness.rebuild();
        assert_eq!(showing(&review), "src/x.rs");
        review.show_tab(ItemTab::Conversation);
        harness.dispatch_event(previous());
        harness.rebuild();
        assert_eq!(showing(&review), "src/y.rs");

        // Cmd-[ leaves the item with its tabs still open.
        assert!(review.is_showing());
        harness.dispatch_event(key(Code::BracketLeft, Key::Character("[".into()), OS_MOD));
        harness.rebuild();
        assert!(!review.is_showing(), "Cmd-[ did not go back to the sidebar");
    }

    /// Cmd-Shift-1 and Cmd-Shift-2 pick Conversation and Code. Code with
    /// no file open opens none, and the item's first visit opens the tree
    /// beside the pane that says so; with no changes, the pane says that.
    #[test]
    fn the_mode_keys_switch_and_code_opens_no_file_with_the_tree() {
        use crate::review::model::ItemTab;
        use std::sync::Arc;
        let (mut harness, review) = window_with_a_file_open();
        let mode = |n: u8| {
            let (code, c) = if n == 1 {
                (Code::Digit1, "!")
            } else {
                (Code::Digit2, "@")
            };
            key(code, Key::Character(c.into()), OS_MOD | Modifiers::SHIFT)
        };
        harness.dispatch_event(mode(1));
        review.close_tab("src/x.rs");
        review
            .changed_files
            .set(Arc::new(vec![perch_git::ChangedFile {
                path: "src/x.rs".into(),
                old_path: None,
                status: perch_git::ChangeStatus::Modified,
                additions: 1,
                deletions: 1,
                binary: false,
                head_blob: None,
            }]));
        harness.rebuild();
        harness.paint();
        assert_eq!(review.tab.get(), ItemTab::Conversation);
        assert!(named(harness.root_id(), "conversation-title").is_some());

        harness.dispatch_event(mode(2));
        harness.rebuild();
        harness.paint();
        assert_eq!(review.tab.get(), ItemTab::Code);
        assert_eq!(review.active.get(), None, "a change opened by itself");
        assert!(
            named(harness.root_id(), "files-panel").is_some(),
            "the first visit had no tree"
        );
        assert!(
            named(harness.root_id(), "\"No file open\"").is_some(),
            "the pane does not say no file is open"
        );

        // With no changes and nothing open, the pane says there is
        // nothing changed.
        harness.dispatch_event(mode(1));
        review.close_tab("src/x.rs");
        review.changed_files.set(Arc::new(Vec::new()));
        harness.dispatch_event(mode(2));
        harness.rebuild();
        harness.paint();
        assert_eq!(review.tab.get(), ItemTab::Code);
        assert_eq!(review.active.get(), None);
        assert!(
            named(harness.root_id(), "\"No changes\"").is_some(),
            "the pane does not say there are no changes"
        );
    }

    /// With nothing focused the key's target is the root, whose handlers
    /// all hear it: Cmd-W still closes the file alone.
    #[test]
    fn cmd_w_with_nothing_focused_closes_only_the_file() {
        let (mut harness, review) = window_with_a_file_open();
        harness.dispatch_event(cmd_w());
        harness.rebuild();
        harness.paint();
        assert_eq!(review.active.get(), None, "the file is still open");
        assert!(review.is_showing(), "the item closed with the file");
        assert_eq!(floem::take_close_window_event_count(harness.window_id()), 0);
    }

    /// On Conversation, Cmd-W closes the item, not the file's tab behind
    /// it: Conversation cannot be closed. The window stays.
    #[test]
    fn cmd_w_on_conversation_closes_the_item() {
        use crate::review::model::ItemTab;
        let (mut harness, review) = window_with_a_file_open();
        review.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(cmd_w());
        harness.rebuild();
        harness.paint();
        assert!(!review.is_showing(), "the item is still showing");
        assert_eq!(floem::take_close_window_event_count(harness.window_id()), 0);
    }

    /// The agents' instructions in Review are a text box: `a` typed there
    /// is an `a`, not the key that opens a composer and takes the keyboard.
    #[test]
    fn the_review_launchers_box_keeps_the_keys_the_window_would_take() {
        let (mut harness, review) = window_with_a_file_open();
        let control = named(harness.root_id(), "review-control")
            .expect("no Review in the bar")
            .get_visual_rect_no_clip();
        harness.click(control.center().x, control.center().y);
        harness.rebuild();
        harness.paint();
        assert!(review.review_open.get(), "the click did not open it");

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
            let modifiers = if ch == "P" {
                Modifiers::SHIFT
            } else {
                Modifiers::empty()
            };
            harness.dispatch_event(key(code, Key::Character(ch.into()), modifiers));
            harness.rebuild();
            harness.paint();
        }
        assert!(
            review.composer.with(Option::is_none),
            "`a` opened a composer"
        );
        assert!(harness.is_focused(editor), "the box lost the keyboard");
        assert_eq!(review.review_launcher_instructions.get(), "Please");
    }
}
