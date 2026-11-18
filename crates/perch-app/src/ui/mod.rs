//! Window layout.

pub mod agents;
pub mod browse;
pub mod drawer;
pub mod feedback;
pub mod history;
pub mod items;
pub mod link;
pub mod menu;
pub mod nav_row;
pub mod new_work;
pub mod notifications;
pub mod open_box;
pub mod profile_card;
pub mod settings;
pub mod sidebar;
pub mod titlebar;
pub mod toast;
pub mod update;
pub mod usage;
pub mod widgets;

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::SignalUpdate;
#[cfg(not(target_os = "macos"))]
use floem::ui_events::keyboard::Location;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::{Backend, Status};
use crate::icons::SidebarIcon;
use crate::keymap::{self, Action};
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

/// Open an item as a click on its row does. An archived row is restored
/// on the way, which the backend treats as the same act.
pub(crate) fn open_item(
    state: AppState,
    review: ReviewState,
    backend: &Rc<Backend>,
    item: &perch_github::Item,
) {
    // An item open in another window is looked at there.
    if crate::windows::raise_holding(state, &item.id, backend.window()) {
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
        // Its checkout is made again first: nothing of it shows till then.
        review.forget_page(&item.id);
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

pub fn app_view(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    state.review.set(Some(review));

    let review_backend = backend.clone();
    // Opening from the sidebar.
    let open_item: items::OpenItem = Rc::new({
        let backend = backend.clone();
        move |item: &perch_github::Item| open_item(state, review, &backend, item)
    });
    let panel_backend = backend.clone();
    let panel_open = open_item.clone();
    let key_backend = backend.clone();
    let capture_backend = backend.clone();
    let pointer_backend = backend.clone();
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
            // Under a kept page the backend still holds the item just
            // left, whose worktree would answer: asked once this one's
            // open lands.
            if asks.is_empty() || review.refreshing.get() {
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
            state.repos_known.get() && state.repos.with(Vec::is_empty),
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
    // The clock that moves the agents' elapsed times, and the run asked
    // for from elsewhere brought into view. Wired once, here, since the
    // rows are built again with the pane.
    agents::wire(state, review, backend.clone());
    let empty_backend = backend.clone();
    let pane = dyn_container(
        move || main_pane.get(),
        move |(open, no_token, new_work, no_repos)| {
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
                crate::review::review_view(state, review, review_backend.clone()).into_any()
            } else if no_token {
                empty_pane(state, review, empty_backend.clone(), token_help(state)).into_any()
            } else {
                empty_pane(
                    state,
                    review,
                    empty_backend.clone(),
                    nothing_open(state, empty_backend.clone(), no_repos),
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

    let sidebar_present = floem::reactive::Memo::new(move |_| state.sidebar_present());
    let root = Stack::vertical((
        Stack::horizontal((
            dyn_container(
                // Sliding shut it is still there, so a Memo: the drawer is
                // built once however many frames the slide takes.
                move || sidebar_present.get(),
                move |present| {
                    if present {
                        // The handle is absolutely placed over the drawer's
                        // right edge, so the drawer is its positioning box.
                        floem::views::Container::new(
                            Stack::horizontal((
                                sidebar::sidebar(state, panel_backend.clone(), panel_open.clone()),
                                sidebar::resize_handle(state),
                            ))
                            // Sliding, the drawer keeps its width and its
                            // right edge stays on the box's: the rest of it
                            // is off the window's left.
                            .style(move |s| {
                                let hidden =
                                    state.sidebar_width.get() - state.sidebar_shown_width();
                                s.height_full().flex_shrink(0.0_f32).margin_left(-hidden)
                            }),
                        )
                        .debug_name("left drawer")
                        .style(move |s| {
                            // Only mid-slide is the box narrower than the
                            // drawer, and only then does it clip: the
                            // footer's popovers hang out of it at rest.
                            let sliding = state.sidebar_slide.get().is_some();
                            s.height_full()
                                .flex_shrink(0.0_f32)
                                .position(floem::style::Position::Relative)
                                .apply_if(sliding, |s| {
                                    s.width(state.sidebar_shown_width())
                                        .overflow_x(floem::taffy::Overflow::Hidden)
                                })
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
        // While the engine on a host can't be reached, along the foot.
        link::banner(state, backend.clone()),
        toast::toast(state, review, capture_backend.clone()),
        // An `@`'s list, over the composer it is typed in.
        crate::review::mention::popup(state, review),
        // The close button's menu, over the composer it hangs from.
        crate::review::close_menu::popup(state, review, backend.clone()),
        // Cmd-P's palette, over everything but a drag's cursor.
        crate::review::go_to::palette(state, review, backend.clone()),
        // Open…'s box, as modal as the palette.
        open_box::overlay(state),
        // Settings, over the whole window but its toasts.
        settings::overlay(state, backend.clone()),
        // Start work's confirmation, as modal.
        crate::review::start_work::overlay(state, backend.clone()),
        // A run's raw output, over the Review popover it was opened from.
        agents::detail::sheet(state, review, backend.clone()),
        // Over everything, and only while a drawer's edge is being dragged.
        sidebar::resize_cursor_sheet(state),
        drawer::resize_cursor_sheet(state),
    ));
    let root_id = root.view_id();
    #[cfg(not(target_os = "macos"))]
    let ctrl_sides = RwSignal::new([false; 2]);
    let hide_shortcuts = move || {
        #[cfg(not(target_os = "macos"))]
        ctrl_sides.set([false; 2]);
        state.shortcut_modifier_changed(false);
    };
    let focus_backend = backend.clone();
    let blur_backend = backend.clone();
    // macOS's scroll bar setting, read again whenever it changes.
    let scroller_style = floem::ext_event::create_trigger();
    crate::scrollers::watch(scroller_style);
    // The view with the keyboard. A key that types goes only to the view
    // with the keyboard, so with none it reaches nothing, not even the
    // window's keys: `j` would do nothing as a window opens, or once Open…
    // or Go to File has gone and let the keyboard go. floem also leaves
    // the keyboard with a view it takes away: `j` that opened the next
    // item, taking the code editor the reader had clicked in with the
    // last one, would leave the next `j` doing nothing. So with nobody
    // holding the keyboard, or the view that held it gone, the window has
    // it, unless a box has taken it meanwhile.
    let keyboard: Rc<Cell<Option<floem::ViewId>>> = Rc::default();
    let (gained, lost, gone) = (keyboard.clone(), keyboard.clone(), keyboard);
    // Nobody was found holding the keyboard at the last layout. The window
    // takes it at the next, by when a view that asked for it in the same
    // layout, as a box made with the window does, has it.
    let unheld: Rc<Cell<bool>> = Rc::default();
    root
        // The window's width, for the drawer on the right to know whether
        // it can be docked. Read off the root's own layout, which is the
        // window's size, after every layout; written only when it changes,
        // and a change restyles the drawer within the same update.
        .on_event_cont(el::UpdatePhaseLayout, move |cx, _| {
            if gone.get().is_none_or(|id| !id.is_valid()) {
                // A menu or a picker is laid over the window rather than
                // in it, so its view with the keyboard is found by looking,
                // not heard of on the way down.
                match focused_descendant(cx, root_id.root()) {
                    Some(holder) => {
                        gone.set(Some(holder));
                        unheld.set(false);
                    }
                    None if unheld.get() => {
                        gone.set(Some(root_id));
                        unheld.set(false);
                        root_id.request_focus();
                    }
                    None => {
                        unheld.set(true);
                        root_id.request_layout();
                    }
                }
            }
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
        // Which view has the keyboard, for the window to have it back once
        // that view has gone. Caught on the way down, as a text box stops
        // its focus events.
        .on_event_with_config(
            el::FocusGained,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE_AND_TARGET,
            },
            move |cx, _| {
                gained.set(focus_target(cx));
                EventPropagation::Continue
            },
        )
        .on_event_with_config(
            el::FocusLost,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE_AND_TARGET,
            },
            move |cx, _| {
                // A menu overlay can receive the release outside this root.
                if !cx.window_state.is_focus_within(root_id) {
                    hide_shortcuts();
                }
                if lost.get() == focus_target(cx) {
                    lost.set(None);
                    // Looked at again after the next layout, by when a
                    // view the keyboard went on to has said so.
                    root_id.request_layout();
                }
                EventPropagation::Continue
            },
        )
        // Whether the reader can see the window: a run that ends on the
        // open item isn't news while they can.
        .on_event_cont(el::WindowGainedFocus, move |_, _| {
            #[cfg(target_os = "macos")]
            crate::windows::sync_command_hints(
                state,
                objc2_app_kit::NSEvent::modifierFlags_class(),
            );
            focus_backend.set_window_key(true)
        })
        .on_event_cont(el::WindowLostFocus, move |_, _| {
            // A key given to a shortcut, still down, comes up elsewhere.
            hide_shortcuts();
            state.settings.let_go();
            blur_backend.set_window_key(false)
        })
        // A key given to a shortcut coming up, wherever the keyboard is
        // now: the Perch menu has its keys back.
        .on_event_with_config(
            el::KeyUp,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE_AND_TARGET,
            },
            move |_, key: &KeyboardEvent| {
                #[cfg(not(target_os = "macos"))]
                track_ctrl(state, ctrl_sides, key);
                if state.settings.held.get_untracked() == Some(key.code) {
                    state.settings.let_go();
                }
                EventPropagation::Continue
            },
        )
        // A press outside one of the bar's popovers shuts
        // it, and goes on to whatever it landed on. Caught on the way down,
        // as most controls stop their presses.
        .on_event_with_config(
            el::PointerDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |cx, press: &PointerButtonEvent| {
                // The mouse's back and forward buttons, from anywhere in
                // the window.
                use floem::ui_events::pointer::PointerButton;
                if let Some(back) = match press.button {
                    Some(PointerButton::X1) => Some(true),
                    Some(PointerButton::X2) => Some(false),
                    _ => None,
                } {
                    // Settings covers the window: back shuts it, as Cmd-[
                    // does, rather than moving what is under it unseen.
                    if state.settings.open.get_untracked() {
                        if back {
                            settings::close(state, true);
                        }
                    } else {
                        history::travel(state, review, &pointer_backend, back);
                    }
                    return EventPropagation::Stop;
                }
                dismiss_popovers(review, cx.hit_path.as_deref());
                EventPropagation::Continue
            },
        )
        // The keys the code editor would otherwise keep. The editor stops
        // every key once it has focus, and these are pressed right after
        // clicking in it, so they are caught on the way down to it rather
        // than on the way back up. Target as well, for when nothing has focus
        // and floem hands the key straight to whoever listens. Nothing here
        // runs while a text box has the keyboard. Which keys are which
        // action's is the keymap's to say.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE_AND_TARGET,
            },
            alone(move |cx, key: &KeyboardEvent| {
                #[cfg(not(target_os = "macos"))]
                track_ctrl(state, ctrl_sides, key);
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
                // A key that types is a box's while one has the keyboard,
                // whatever action it is bound to.
                let in_box = typing_key(state, review, cx, root_id, key);
                // Settings covers the window: none of the window's keys
                // reach what is under it.
                if state.settings.open.get() {
                    return settings::key(state, cx, key, in_box);
                }
                // Escape puts a run's raw output away, from wherever the
                // keyboard is, before it reaches a popover under it.
                if key.code == Code::Escape
                    && key.modifiers.is_empty()
                    && review.agent_output.with_untracked(Option::is_some)
                {
                    agents::detail::close(review);
                    return EventPropagation::Stop;
                }
                // Cmd-N opens the new-work view, from anywhere.
                if keymap::pressed(Action::NewWork, key, in_box) {
                    new_work::open(state, key_backend);
                    return EventPropagation::Stop;
                }
                // New Window, Open… and Settings… are the Perch menu's on
                // macOS, which holds a key for each. Off it there is no
                // menu to hold them, so their keys are here, as are the
                // keys the menu can't hold.
                if crate::app_menu::pressed_in_window(Action::NewWindow, key, in_box) {
                    crate::windows::open_new();
                    return EventPropagation::Stop;
                }
                if crate::app_menu::pressed_in_window(Action::Open, key, in_box) {
                    open_box::show(state, focused_descendant(cx, root_id));
                    return EventPropagation::Stop;
                }
                if crate::app_menu::pressed_in_window(Action::Settings, key, in_box) {
                    settings::show(state, focused_descendant(cx, root_id));
                    return EventPropagation::Stop;
                }
                // Cmd-[ shuts it, with what was typed kept, from its
                // message box too.
                if keymap::pressed(Action::Back, key, in_box) && state.new_work.open.get_untracked()
                {
                    state.new_work.open.set(false);
                    return EventPropagation::Stop;
                }
                // Cmd-P puts it up, app-wide, as the tree's key is; Escape
                // gives the keyboard back to whatever had it.
                if keymap::pressed(Action::GoToFile, key, in_box)
                    && crate::review::go_to::available(review)
                {
                    crate::review::go_to::open(review, focused_descendant(cx, root_id));
                    return EventPropagation::Stop;
                }
                // Cmd-Shift-A opens the Agents dropdown from the bar, and
                // shuts it, from anywhere; whatever had the keyboard gets it
                // back as it shuts.
                if keymap::pressed(Action::ToggleAgents, key, in_box) {
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
                if keymap::pressed(Action::ToggleFiles, key, in_box)
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
                let paging = [
                    (Action::PageUp, Paging::Up),
                    (Action::PageDown, Paging::Down),
                    (Action::ScrollToTop, Paging::Top),
                    (Action::ScrollToEnd, Paging::End),
                ]
                .into_iter()
                .find(|(action, _)| keymap::pressed(*action, key, in_box))
                .map(|(_, paging)| paging)
                .filter(|p| {
                    matches!(p, Paging::Up | Paging::Down)
                        || !(review.composer_focused.get() || text_box_focused(cx, root_id))
                })
                .filter(|_| !boxed && review.is_open());
                if let Some(paging) = paging {
                    let showing = match review.tab.get() {
                        crate::review::model::ItemTab::Conversation => true,
                        crate::review::model::ItemTab::Code => {
                            review.active_file().is_some_and(|f| !f.truncated) && !review.drawn()
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
                            (review.active.with(Option::is_some) && !review.drawn())
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
                    let pressed = |action| keymap::pressed(action, key, in_box);
                    if pressed(Action::Find) {
                        crate::review::find::open(review, pane, true);
                        return EventPropagation::Stop;
                    }
                    let next = pressed(Action::FindNext);
                    if next || pressed(Action::FindPrevious) {
                        if !find_open {
                            crate::review::find::open(review, pane, false);
                        }
                        crate::review::find::walk(review, pane, next);
                        return EventPropagation::Stop;
                    }
                    if find_open {
                        type Flag = fn(&mut crate::review::find::Options) -> &mut bool;
                        let toggles: [(Action, Flag); 3] = [
                            (Action::FindMatchCase, |o| &mut o.match_case),
                            (Action::FindWholeWord, |o| &mut o.whole_word),
                            (Action::FindRegex, |o| &mut o.regex),
                        ];
                        for (action, flag) in toggles {
                            if pressed(action) {
                                review.find_options.update(|o| {
                                    let flag = flag(o);
                                    *flag = !*flag;
                                });
                                return EventPropagation::Stop;
                            }
                        }
                    }
                }
                // Ctrl-- goes back and Ctrl-Shift-- forward, as in VS Code:
                // from anywhere, a box being typed in or the empty pane too,
                // since neither key types anything.
                let back = keymap::pressed(Action::HistoryBack, key, in_box);
                if back || keymap::pressed(Action::HistoryForward, key, in_box) {
                    history::travel(state, review, key_backend, back);
                    return EventPropagation::Stop;
                }
                // The sidebar's keys, from anywhere, the code and the empty
                // pane too: Done and Mark Unread on the row under the
                // pointer, else the open item, the next and previous rows,
                // the tabs, and Open on GitHub. Each only where its button
                // or menu row would do something; elsewhere the key goes on.
                let pressed = |action| keymap::pressed(action, key, in_box);
                if pressed(Action::MarkDone) && notifications::done_by_key(state, key_backend) {
                    return EventPropagation::Stop;
                }
                if pressed(Action::MarkUnread)
                    && notifications::mark_unread_by_key(state, key_backend)
                {
                    return EventPropagation::Stop;
                }
                let next_item = pressed(Action::NextItem);
                if (next_item || pressed(Action::PreviousItem))
                    && sidebar::step(state, review, key_backend, next_item)
                {
                    return EventPropagation::Stop;
                }
                use crate::items::SidebarTab;
                for tab in SidebarTab::ALL {
                    if pressed(sidebar::tab_action(tab)) {
                        sidebar::show_tab(state, tab);
                        return EventPropagation::Stop;
                    }
                }
                if pressed(Action::OpenOnGithub) {
                    if let Some(url) = notifications::github_url_by_key(state) {
                        widgets::open_in_browser(&url);
                        return EventPropagation::Stop;
                    }
                }
                // Review, Merge and Send Feedback open and shut from
                // anywhere, a box in one of them too, where the button is
                // showing, not under new work; whatever had the keyboard
                // gets it back as each shuts.
                let bar_shown = review.is_open() && !state.new_work.open.get_untracked();
                if pressed(Action::ToggleReview)
                    && bar_shown
                    && crate::review::header::offers_review(review)
                {
                    if review.review_open.get_untracked() {
                        crate::review::review_menu::cancel(review, true);
                    } else {
                        crate::review::review_menu::open(review, focused_descendant(cx, root_id));
                    }
                    return EventPropagation::Stop;
                }
                if pressed(Action::ToggleMerge)
                    && bar_shown
                    && crate::review::merge_button::toggle_by_key(
                        review,
                        focused_descendant(cx, root_id),
                    )
                {
                    return EventPropagation::Stop;
                }
                if pressed(Action::SendFeedback) {
                    let opener = (!review.feedback_open.get_untracked())
                        .then(|| focused_descendant(cx, root_id))
                        .flatten();
                    feedback::toggle(state, review, opener);
                    return EventPropagation::Stop;
                }
                // Any box that takes typing keeps the keys below: `a` typed
                // in the close-as-issue title is an `a`, not a composer.
                let typing = typing || review.find_focused.get() || text_box_focused(cx, root_id);
                if typing || !review.is_open() {
                    if keymap::pressed(Action::Close, key, in_box) {
                        close_item_or_window(state, review, key_backend, root_id);
                        return EventPropagation::Stop;
                    }
                    return EventPropagation::Continue;
                }
                // Cmd-Shift-1 shows Conversation and Cmd-Shift-2 Code, as the
                // switch in the bar does. Here rather than with the other Cmd
                // keys because they are pressed from the code as often as
                // from anywhere.
                if keymap::pressed(Action::ShowConversation, key, in_box) {
                    review.show_tab(crate::review::model::ItemTab::Conversation);
                    return EventPropagation::Stop;
                }
                if keymap::pressed(Action::ShowCode, key, in_box) {
                    if review.has_files() {
                        review.show_tab(crate::review::model::ItemTab::Code);
                    }
                    return EventPropagation::Stop;
                }
                // Cmd-} and Cmd-{ step through the tabs, wrapping at the
                // ends, as in Safari and Xcode. Cmd-[ stays the way back
                // to the sidebar.
                let next_tab = keymap::pressed(Action::NextTab, key, in_box);
                if (next_tab || keymap::pressed(Action::PreviousTab, key, in_box))
                    && crate::review::tabs::step_tab(review, key_backend, next_tab)
                {
                    return EventPropagation::Stop;
                }
                // The item's keys, as the news pill, the bar's menu and the
                // view menu have them.
                if pressed(Action::Refresh) && crate::review::refresh(state, review, key_backend) {
                    return EventPropagation::Stop;
                }
                if pressed(Action::CopyLink) {
                    if let Some(url) = crate::review::header::page(review) {
                        widgets::copy_to_clipboard(url);
                        return EventPropagation::Stop;
                    }
                }
                if pressed(Action::CopyBranchName) {
                    if let Some(branch) = crate::review::header::open_branch_name(review) {
                        widgets::copy_to_clipboard(branch);
                        return EventPropagation::Stop;
                    }
                }
                if pressed(Action::StartWork) && crate::review::header::start_work(state, review) {
                    return EventPropagation::Stop;
                }
                if pressed(Action::ToggleWrap) && crate::review::tabs::toggle_wrap(state, review) {
                    return EventPropagation::Stop;
                }
                // The navigation keys, VS Code's: F12 goes to the definition
                // of the caret's symbol, Cmd-F12 to its implementations,
                // Shift-F12 asks for its references and Shift-Alt-H for its
                // call hierarchy. Its type definition has a key only when
                // the keymap gives it one.
                if keymap::pressed(Action::GoToDefinition, key, in_box) {
                    crate::review::navigation::definition_at_cursor(review, &references_backend);
                    return EventPropagation::Stop;
                }
                if keymap::pressed(Action::GoToTypeDefinition, key, in_box) {
                    crate::review::navigation::type_definition_at_cursor(
                        review,
                        &references_backend,
                    );
                    return EventPropagation::Stop;
                }
                if keymap::pressed(Action::GoToImplementations, key, in_box) {
                    crate::review::navigation::implementations_at_cursor(
                        review,
                        &references_backend,
                    );
                    return EventPropagation::Stop;
                }
                if keymap::pressed(Action::FindReferences, key, in_box) {
                    crate::review::references::find_at_cursor(review, &references_backend);
                    return EventPropagation::Stop;
                }
                if keymap::pressed(Action::CallHierarchy, key, in_box) {
                    crate::review::hierarchy::show_at_cursor(review, &references_backend);
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
                if keymap::pressed(Action::Comment, key, in_box) {
                    // On Conversation the composer is laid in place over
                    // the timeline, about the item as a whole: `a` opens
                    // it, or takes the keyboard back to it.
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
                            // A composer open in the code is gone back
                            // to rather than replaced.
                            None => review.ask_on_page(crate::review::threads::defaults(state)),
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
                if key.modifiers.is_empty() {
                    match key.code {
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
                // and `}` and `{` open the next and previous unviewed file.
                // Files only. The braces are shifted brackets: the physical codes with Shift alone, so Cmd-{
                // steps through the tabs.
                if review.tab.get() == crate::review::model::ItemTab::Code {
                    let pressed = |action| keymap::pressed(action, key, in_box);
                    if pressed(Action::Close) {
                        if let Some(path) = review.active.get() {
                            review.close_tab(&path);
                            return EventPropagation::Stop;
                        }
                    }
                    let next_change = pressed(Action::NextChange);
                    if next_change || pressed(Action::PreviousChange) {
                        crate::review::tabs::jump_to_change(review, next_change);
                        return EventPropagation::Stop;
                    }
                    if pressed(Action::ToggleViewed) {
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
                    let forward = pressed(Action::NextUnviewed);
                    if forward || pressed(Action::PreviousUnviewed) {
                        let active = review.active.get();
                        if let Some(path) = review.next_unviewed(active.as_deref(), forward) {
                            // The same path a click in the rail takes.
                            if review.open_tab(&path) {
                                key_backend.open_file(&path);
                            }
                            review.show_in_tree(&path);
                        }
                        return EventPropagation::Stop;
                    }
                }
                // Cmd-W with no file to close closes the item, as the X does.
                if keymap::pressed(Action::Close, key, in_box) {
                    close_item_or_window(state, review, key_backend, root_id);
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            }),
        )
        // Window-level shortcuts. Key events bubble from whatever has focus, so
        // a handler on the root view catches them wherever the reader is.
        .on_event(el::KeyDown, move |cx, key: &KeyboardEvent| {
            // None of these while Cmd-P's palette, Open…, Start work's
            // confirmation or Settings is up: they are modal.
            if review.go_to.open.get()
                || state.open_box.open.get()
                || state.start_work_confirm.with(Option::is_some)
                || state.settings.open.get()
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

            let in_box = typing_key(state, review, cx, root_id, key);
            // `p` pins the sidebar row under the pointer, else the selected
            // row, or unpins it.
            if keymap::pressed(Action::Pin, key, in_box) {
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

            // Show or hide the sidebar drawer.
            if keymap::pressed(Action::ToggleSidebar, key, in_box) {
                state.toggle_sidebar();
                return EventPropagation::Stop;
            }
            // Back to the sidebar.
            if keymap::pressed(Action::Back, key, in_box) && review.is_showing() {
                let navigation_id = key_backend.close_review();
                state.begin_close(review, navigation_id);
                return EventPropagation::Stop;
            }
            // Cmd-1 is the sidebar's search. With the drawer shut it opens
            // the drawer, and the box takes the keyboard once it is built.
            if keymap::pressed(Action::FocusSearch, key, in_box) {
                if state.sidebar_open.get() {
                    if let Some(id) = state.search_box.get() {
                        id.request_focus();
                    }
                } else {
                    state.focus_search.set(true);
                    state.show_sidebar(true);
                }
                return EventPropagation::Stop;
            }
            EventPropagation::Continue
        })
        .style(move |s| {
            scroller_style.track();
            s.size_full()
                .apply_if(
                    state.shortcut_hints.get() != crate::state::ShortcutHints::Visible,
                    |s| s.class(widgets::ShortcutHint, |s| s.hide()),
                )
                // The binding of an icon-only control takes its glyph's place.
                .apply_if(
                    state.shortcut_hints.get() == crate::state::ShortcutHints::Visible,
                    |s| s.class(widgets::ShortcutIcon, |s| s.hide()),
                )
                // A press on anything that takes no keyboard of its own,
                // a sidebar row or the page, gives the window the
                // keyboard, so a key that types, `j` or `e`, reaches the
                // window's keys rather than nothing: floem hands such a
                // key only to whatever has the keyboard. Focusable only,
                // so Tab never stops on the window.
                .focusable()
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
                // Every scroll's bars as macOS's own: a pill of a slider,
                // which floem draws only on macOS and squares off
                // elsewhere, and when macOS overlays them, there while the
                // view scrolls and while the pointer is on one, and fading
                // out once it stops.
                .custom_style_class(|s: floem::views::scroll::ScrollCustomStyle| {
                    s.handle_rounded(true)
                        .show_bars_when_idle(!crate::scrollers::overlaid())
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
                    sidebar::pane_lights_lane(state),
                    history::buttons(state, review, backend.clone()),
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
        crate::windows::close_window(window);
    }
}

/// Shut the bar's popovers (Agents, usage, feedback, Review, Merge) when a
/// press with `hit_path` lands outside the button each hangs from.
fn dismiss_popovers(review: ReviewState, hit_path: Option<&[floem::ElementId]>) {
    // A menu opened from a popover is not under it in the tree: a press
    // on one of its rows, or the press that puts it away, leaves the
    // popover up.
    if menu::is_open() {
        return;
    }
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
    // The Review popover stays under the sheet a reviewer's output is read
    // in, and while a review is on its way to GitHub. Its draft is kept.
    if review.review_open.get_untracked()
        && review.agent_output.with_untracked(Option::is_none)
        && !inside(review.review_anchor.get_untracked())
    {
        crate::review::review_menu::cancel(review, false);
    }
    // The merge box stays while it says why a merge failed: shutting it
    // forgets the error, and it may have come up while the reader was busy
    // elsewhere.
    if review.merge_open.get_untracked()
        && review.merge_error.with_untracked(Option::is_none)
        && !inside(review.merge_anchor.get_untracked())
    {
        crate::review::merge_button::close(review);
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

/// The view a focus event is for, from any phase of it.
fn focus_target(cx: &floem::context::EventCx<'_>) -> Option<floem::ViewId> {
    cx.dispatch
        .as_deref()?
        .iter()
        .find(|step| step.phase == floem::event::Phase::Target)
        .map(|step| step.owning_id)
}

#[cfg(not(target_os = "macos"))]
fn track_ctrl(state: AppState, sides: RwSignal<[bool; 2]>, event: &KeyboardEvent) {
    if event.key != Key::Named(NamedKey::Control) {
        return;
    }
    let side = match event.location {
        Location::Left => 0,
        Location::Right => 1,
        _ => return,
    };
    sides.update(|held| held[side] = event.state == KeyState::Down);
    state.shortcut_modifier_changed(sides.get_untracked().contains(&true));
}

/// A window key handler that, once it stops a key, stops it for the
/// root's other handlers too. With nothing focused the root is the key's
/// target, and every handler on it hears the key even once one has
/// stopped it, so Cmd-[ that shut new work would close the item under it
/// as well.
fn alone(
    mut handler: impl FnMut(&mut floem::context::EventCx, &KeyboardEvent) -> EventPropagation + 'static,
) -> impl FnMut(&mut floem::context::EventCx, &KeyboardEvent) -> EventPropagation + 'static {
    move |cx, key| {
        let result = handler(cx, key);
        if result.is_stop() {
            cx.stop_immediate_propagation();
        }
        result
    }
}

/// Whether `key` types and a box has the keyboard to type it in: the
/// sidebar's search, a composer, Review's form, a find bar or any other
/// text box. A key that types is the box's then, whatever it is bound to.
/// The focused view is looked for only for such a key, as that walks the
/// window.
fn typing_key(
    state: AppState,
    review: ReviewState,
    cx: &floem::context::EventCx<'_>,
    root: floem::ViewId,
    key: &KeyboardEvent,
) -> bool {
    let shortcut = crate::ui::menu::Shortcut {
        modifiers: key.modifiers,
        code: key.code,
    };
    shortcut.is_bare()
        && (state.search_focused.get_untracked()
            || review.composer_focused.get_untracked()
            || review.review_open.get_untracked()
            || review.find_focused.get_untracked()
            || text_box_focused(cx, root))
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

/// The main area with nothing open. With no repository in the config,
/// Add repository under the words: the inbox needs none, but new work
/// does, and this is the first screen a new install shows.
fn nothing_open(state: AppState, backend: Rc<Backend>, no_repos: bool) -> impl IntoView {
    let under = if no_repos {
        Stack::vertical((
            Label::derived(|| "Add a repository to start new work.".to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg_muted())
                    .margin_top(2.0)
                    .margin_bottom(16.0)
            }),
            widgets::action_button(
                || "Add repository…".to_string(),
                "empty-add-repository",
                false,
                || true,
                move || new_work::add_repository(state, backend.clone()),
            ),
        ))
        .style(|s| s.items_center())
        .into_any()
    } else {
        Empty::new().into_any()
    };
    widgets::centered_message_with(
        SidebarIcon::Branch,
        || "Nothing open".to_string(),
        || "Pick a pull request, an issue, or a worktree from the sidebar.".to_string(),
        under,
    )
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
    /// A config with no repositories puts Add repository under "Nothing
    /// open"; one with a repository, or a window the backend has not told
    /// yet, has the words alone.
    #[test]
    fn with_no_repository_nothing_open_offers_to_add_one() {
        let state = AppState::new();
        let review = ReviewState::new();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let offered =
            |harness: &HeadlessHarness| named(harness.root_id(), "empty-add-repository").is_some();
        assert!(!offered(&harness), "not before the backend says");

        state.repos_known.set(true);
        harness.rebuild();
        harness.paint();
        assert!(offered(&harness), "none in the config");

        state.repos.set(vec![perch_github::DeclaredRepo {
            identity: "slatedb/slatedb".into(),
            name: "slatedb".into(),
        }]);
        harness.rebuild();
        harness.paint();
        assert!(!offered(&harness), "one in the config");
    }

    #[test]
    fn sliding_the_drawer_keeps_its_width_and_the_pane_follows_its_edge() {
        let state = AppState::new();
        let review = ReviewState::new();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        let root_id = harness.root_id();
        let drawer = named(root_id, "left drawer").expect("no drawer");
        let strip = || {
            named(root_id, "drawer strip")
                .unwrap()
                .get_visual_rect_no_clip()
        };
        let pane = || {
            named(root_id, "main-pane")
                .unwrap()
                .get_visual_rect_no_clip()
        };
        assert_eq!((strip().x0, pane().x0), (0.0, theme::SIDEBAR_WIDTH));

        // Half way shut: half of it is off the window's left, at its full
        // width, and the pane has the half it left.
        state.sidebar_slide.set(Some(0.5));
        state.sidebar_open.set(false);
        harness.rebuild();
        let half = theme::SIDEBAR_WIDTH / 2.0;
        assert_eq!(
            named(harness.root_id(), "left drawer"),
            Some(drawer),
            "the drawer was rebuilt for the slide"
        );
        // The strip ends at the drawer's border, a pixel short of its edge.
        assert_eq!((strip().x0, strip().x1), (-half, half - 1.0));
        assert_eq!(pane().x0, half);
        assert!(
            named(harness.root_id(), "empty bar")
                .and_then(|bar| named(bar, "drawer toggle"))
                .is_some_and(|toggle| toggle.get_visual_rect_no_clip().is_zero_area()),
            "the pane's control is up while the drawer covers the lights"
        );

        // Nearly shut, the pane's control is up, where it rests. Off macOS
        // there are no lights and no lane past the bar's padding, so the
        // control waits for the drawer to be gone.
        state.sidebar_slide.set(Some(10.0 / theme::SIDEBAR_WIDTH));
        harness.rebuild();
        let toggle = named(harness.root_id(), "empty bar")
            .and_then(|bar| named(bar, "drawer toggle"))
            .unwrap();
        assert!((pane().x0 - 10.0).abs() < 1e-6);
        if theme::ACTIVITY_RAIL_WIDTH - theme::REVIEW_HEADER_PADDING >= 10.0 {
            assert!(
                (toggle.get_visual_rect_no_clip().x0 - theme::ACTIVITY_RAIL_WIDTH).abs() < 1e-6
            );
        } else {
            assert!(toggle.get_visual_rect_no_clip().is_zero_area());
        }

        // There: the drawer is gone.
        state.sidebar_slide.set(None);
        harness.rebuild();
        assert!(named(harness.root_id(), "left drawer").is_none());
        assert_eq!(pane().x0, 0.0);
    }

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

    /// The mouse's back button and Ctrl-- go back from anywhere, the empty
    /// pane too, opening the item the place was on; its forward button
    /// goes forward again.
    #[test]
    fn the_mouse_back_button_and_ctrl_minus_go_back_across_items() {
        use crate::review::model::{Place, Spot};
        use floem::ui_events::pointer::{
            PointerButton, PointerButtonEvent, PointerEvent, PointerId, PointerInfo, PointerType,
        };
        let state = AppState::new();
        let review = ReviewState::new();
        let item = |id: &str| {
            let mut item =
                perch_github::Item::new("slatedb/slatedb", "2026-09-18T10:00:00Z".parse().unwrap());
            item.id = id.into();
            item
        };
        state.items.set(vec![item("a"), item("b")]);
        let place = |id: &str| Place {
            item: Some(id.into()),
            spot: Spot::Conversation,
        };
        review.history.update(|h| {
            h.push(place("a"));
            h.push(place("b"));
        });
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let press = |button: PointerButton, at: floem::kurbo::Point, down: bool| {
            let mut pointer_state = floem::ui_events::pointer::PointerState::default();
            pointer_state.position.x = at.x;
            pointer_state.position.y = at.y;
            pointer_state.count = 1;
            let event = PointerButtonEvent {
                state: pointer_state,
                button: Some(button),
                pointer: PointerInfo {
                    pointer_id: Some(PointerId::PRIMARY),
                    persistent_device_id: None,
                    pointer_type: PointerType::Mouse,
                },
            };
            floem::event::Event::Pointer(if down {
                PointerEvent::Down(event)
            } else {
                PointerEvent::Up(event)
            })
        };

        // Pressed over the drawer control, Back goes back and does not
        // also press the control.
        let toggle = named(harness.root_id(), "drawer toggle")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.dispatch_event(press(PointerButton::X1, toggle, true));
        harness.dispatch_event(press(PointerButton::X1, toggle, false));
        harness.rebuild();
        assert!(state.sidebar_open.get(), "the drawer control was pressed");
        assert_eq!(state.open_item.get().as_deref(), Some("b"));
        assert!(review.opening.get());

        harness.dispatch_event(key(
            Code::Minus,
            Key::Character("-".into()),
            Modifiers::CONTROL,
        ));
        assert_eq!(state.open_item.get().as_deref(), Some("a"));
        assert!(!review.history.with(|h| h.can_go_back()));

        // Nothing was open to come back to: Forward has nowhere to go.
        harness.dispatch_event(press(PointerButton::X2, toggle, true));
        assert_eq!(state.open_item.get().as_deref(), Some("a"));
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
        let (harness, _, review) = window_and_state_with_a_file_open();
        (harness, review)
    }

    /// The same, with the window's state.
    fn window_and_state_with_a_file_open() -> (HeadlessHarness, AppState, ReviewState) {
        window_with_a_file_open_on(Rc::new(Backend::detached()))
    }

    /// The same, on `backend`.
    fn window_with_a_file_open_on(
        backend: Rc<Backend>,
    ) -> (HeadlessHarness, AppState, ReviewState) {
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

        let view = app_view(state, review, backend);
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, state, review)
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

    #[test]
    fn toolbar_shows_current_bindings_inline_after_a_modifier_hold() {
        let _keymap = keymap::install(keymap::with_bindings("worktrees = 'alt+x'\nbrowse = []"));
        let (mut harness, app, _) = window_and_state_with_a_file_open();
        let root = harness.root_id();
        let shortcut = |control| named(named(root, control).unwrap(), "shortcut hint").unwrap();
        let hint = shortcut("tab:Worktrees");
        let toggle_glyph = shortcut("drawer toggle").parent().unwrap().children()[0];
        assert!(hint.is_hidden());
        assert!(!toggle_glyph.is_hidden());
        let press = || {
            app.shortcut_modifier_changed(true);
            let crate::state::ShortcutHints::Waiting(timer) = app.shortcut_hints.get_untracked()
            else {
                panic!("expected a pending shortcut reveal");
            };
            timer
        };
        let tap = press();
        harness.rebuild();
        assert!(hint.is_hidden());
        app.shortcut_modifier_changed(false);
        let hold = press();
        app.reveal_shortcut_hints(tap);
        harness.rebuild();
        assert!(hint.is_hidden(), "the previous tap revealed a later hold");
        app.reveal_shortcut_hints(hold);
        harness.rebuild();
        let painted = harness.paint_and_get_order();
        for (control, action) in [
            ("tab:Inbox", Action::Inbox),
            ("tab:Worktrees", Action::Worktrees),
            ("Conversation pill", Action::ShowConversation),
            ("Code pill", Action::ShowCode),
        ] {
            let hint = shortcut(control);
            assert!(
                painted.contains(&hint),
                "{control} shortcut was not painted"
            );
            assert!(hint.debug_name().contains(&keymap::label(action).unwrap()));
            let rect = hint.get_visual_rect();
            assert!(
                rect.width() > 8.0 && rect.height() > 2.0,
                "{control} {rect:?}"
            );
            assert_eq!(rect, hint.get_visual_rect_no_clip(), "{control}");
            let content = hint.parent().unwrap().children()[0].get_visual_rect();
            assert!(
                rect.x0 >= content.x1 && rect.y0 < content.y1 && content.y0 < rect.y1,
                "{control}: hint {rect:?} should sit beside {content:?}"
            );
        }
        // An icon-only control shows its binding where the icon was.
        for (control, action) in [
            ("drawer toggle", Action::ToggleSidebar),
            ("new work", Action::NewWork),
            ("back tip", Action::HistoryBack),
            ("forward tip", Action::HistoryForward),
        ] {
            let hint = shortcut(control);
            assert!(
                painted.contains(&hint),
                "{control} shortcut was not painted"
            );
            assert!(hint.debug_name().contains(&keymap::label(action).unwrap()));
            let glyph = hint.parent().unwrap().children()[0];
            assert!(
                glyph.is_hidden(),
                "{control} keeps its icon beside its keys"
            );
            let rect = hint.get_visual_rect();
            let control_rect = named(root, control).unwrap().get_visual_rect();
            assert!(
                rect.width() > 8.0
                    && rect.x0 >= control_rect.x0
                    && rect.x1 <= control_rect.x1
                    && rect.y0 >= control_rect.y0
                    && rect.y1 <= control_rect.y1,
                "{control}: {rect:?} should sit inside {control_rect:?}"
            );
        }
        let back = shortcut("back tip").get_visual_rect();
        let forward = shortcut("forward tip").get_visual_rect();
        assert!(
            back.x1 < forward.x0,
            "history shortcuts overlap: {back:?} {forward:?}"
        );
        assert!(shortcut("tab:Browse").is_hidden());
        app.shortcut_modifier_changed(false);
        harness.rebuild();
        assert!(hint.is_hidden());
        assert!(!toggle_glyph.is_hidden(), "the icon comes back");

        let pending = press();
        harness.dispatch_event(floem::event::Event::Window(
            floem::event::WindowEvent::FocusLost,
        ));
        app.reveal_shortcut_hints(pending);
        harness.rebuild();
        assert!(hint.is_hidden(), "window blur must cancel a pending reveal");
    }

    /// A toolbar too narrow for its hints keeps its icons, and so does a
    /// control whose binding was taken away.
    #[test]
    fn an_icon_gives_way_to_its_binding_only_where_there_is_one_and_room() {
        let _keymap = keymap::install(keymap::with_bindings("new_work = []"));
        let (mut harness, app, _) = window_and_state_with_a_file_open();
        let root = harness.root_id();
        let glyph = |control| {
            let hint = named(named(root, control).unwrap(), "shortcut hint").unwrap();
            (hint, hint.parent().unwrap().children()[0])
        };
        let hold = || {
            app.shortcut_modifier_changed(true);
            let crate::state::ShortcutHints::Waiting(timer) = app.shortcut_hints.get_untracked()
            else {
                panic!("expected a pending shortcut reveal");
            };
            app.reveal_shortcut_hints(timer);
        };
        hold();
        harness.rebuild();
        let (hint, icon) = glyph("drawer toggle");
        assert!(!hint.is_hidden() && icon.is_hidden());
        let (hint, icon) = glyph("new work");
        assert!(
            hint.is_hidden() && !icon.is_hidden(),
            "nothing to show for it"
        );

        app.sidebar_width.set(theme::lights_lane_width() + 310.0);
        harness.rebuild();
        assert!(glyph("tab:Inbox").0.is_hidden());
        let (hint, icon) = glyph("drawer toggle");
        assert!(
            !hint.is_hidden() && icon.is_hidden(),
            "the toggle still fits"
        );

        app.sidebar_width.set(theme::lights_lane_width() + 100.0);
        harness.rebuild();
        let (hint, icon) = glyph("drawer toggle");
        assert!(
            hint.is_hidden() && !icon.is_hidden(),
            "no room for the keys"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn either_ctrl_key_keeps_the_same_shortcut_hold() {
        use crate::state::ShortcutHints;
        let (mut harness, app, _) = window_and_state_with_a_file_open();
        let mut ctrl = |location, state| {
            let floem::event::Event::Key(mut event) =
                key(Code::Unidentified, Key::Named(NamedKey::Control), OS_MOD)
            else {
                unreachable!()
            };
            event.location = location;
            event.state = state;
            harness.dispatch_event(floem::event::Event::Key(event));
        };
        ctrl(Location::Left, KeyState::Down);
        let ShortcutHints::Waiting(timer) = app.shortcut_hints.get_untracked() else {
            panic!("pressing Ctrl must start the hold timer");
        };
        ctrl(Location::Right, KeyState::Down);
        ctrl(Location::Left, KeyState::Up);
        app.reveal_shortcut_hints(timer);
        assert!(matches!(
            app.shortcut_hints.get_untracked(),
            ShortcutHints::Visible
        ));
        ctrl(Location::Right, KeyState::Up);
        assert!(matches!(
            app.shortcut_hints.get_untracked(),
            ShortcutHints::Hidden
        ));
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

    /// Off macOS, with no Perch menu, Ctrl-O puts Open…'s box up and
    /// Ctrl-Shift-N is New Window's, not Ctrl-N's new work.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn ctrl_o_puts_the_open_box_up_and_ctrl_shift_n_is_not_new_work() {
        let state = AppState::new();
        let review = ReviewState::new();
        state.review.set(Some(review));
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(key(
            Code::KeyN,
            Key::Character("N".into()),
            OS_MOD | Modifiers::SHIFT,
        ));
        harness.rebuild();
        assert!(!state.new_work.open.get(), "New Window is not new work");

        harness.dispatch_event(key(Code::KeyO, Key::Character("o".into()), OS_MOD));
        harness.rebuild();
        harness.paint();
        harness.rebuild();
        harness.paint();
        assert!(state.open_box.open.get());
        let text = named(harness.root_id(), "open-box-text").expect("no box");
        assert!(harness.is_focused(text), "the box has the keyboard");
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
        assert!(named(harness.root_id(), "conversation column").is_some());

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

    /// A window key a user binds to a letter is the window's from the
    /// code, and a letter in a box that has the keyboard; the key it had
    /// before does nothing.
    #[test]
    fn a_window_key_bound_to_a_bare_letter_is_typed_in_a_box_that_has_the_keyboard() {
        let _keymap = crate::keymap::install(crate::keymap::with_bindings("toggle_agents = \"t\""));
        let (mut harness, review) = window_with_a_file_open();
        // A key that types reaches the window only through a view with
        // the keyboard: the code, here.
        let code = named(harness.root_id(), "tinted editor")
            .expect("no editor in the window")
            .get_visual_rect_no_clip();
        harness.click(code.x0 + 200.0, code.y0 + 40.0);
        harness.rebuild();
        harness.paint();
        let press = |harness: &mut HeadlessHarness, code: Code, ch: &str, modifiers: Modifiers| {
            harness.dispatch_event(key(code, Key::Character(ch.into()), modifiers));
            harness.rebuild();
            harness.paint();
        };
        press(&mut harness, Code::KeyT, "t", Modifiers::empty());
        assert!(review.agents_dropdown_open.get(), "`t` is the Agents key");
        press(&mut harness, Code::KeyT, "t", Modifiers::empty());
        assert!(!review.agents_dropdown_open.get(), "`t` again shuts it");
        press(&mut harness, Code::KeyA, "a", OS_MOD | Modifiers::SHIFT);
        assert!(!review.agents_dropdown_open.get(), "Cmd-Shift-A is not");

        let control = named(harness.root_id(), "review-control")
            .expect("no Review in the bar")
            .get_visual_rect_no_clip();
        harness.click(control.center().x, control.center().y);
        harness.rebuild();
        harness.paint();
        let rect = named(harness.root_id(), "review-instructions")
            .expect("no instructions box")
            .get_visual_rect_no_clip();
        harness.click(rect.center().x, rect.center().y);
        harness.rebuild();
        harness.paint();
        press(&mut harness, Code::KeyT, "t", Modifiers::empty());
        assert!(
            !review.agents_dropdown_open.get(),
            "`t` in the box was taken"
        );
        assert_eq!(review.review_launcher_instructions.get(), "t");
    }

    /// Cmd-[ with new work over an item, and nothing focused so the root
    /// is the key's target, shuts new work and leaves the item showing.
    #[test]
    fn cmd_left_bracket_over_an_item_shuts_new_work_alone() {
        let (mut harness, state, review) = window_and_state_with_a_file_open();
        state.new_work.open.set(true);
        harness.rebuild();
        harness.paint();
        // New work's box takes the keyboard; give it up.
        harness.root_id().clear_focus();
        harness.rebuild();
        harness.paint();
        harness.dispatch_event(key(Code::BracketLeft, Key::Character("[".into()), OS_MOD));
        harness.rebuild();
        assert!(!state.new_work.open.get(), "new work is still up");
        assert!(review.is_showing(), "the item closed with new work");
    }

    /// A notification about issue `number` of org/repo.
    fn issue_thread(id: &str, number: u64, unread: bool) -> perch_github::Thread {
        use perch_github::{Reason, Repository, Subject, SubjectKind};
        perch_github::Thread {
            id: id.into(),
            unread,
            local_unread: false,
            local_read: false,
            restored_at: None,
            reason: Reason::Mention,
            updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            last_read_at: (!unread).then(|| "2026-09-19T11:00:00Z".parse().unwrap()),
            subject: Subject {
                title: format!("Issue {number}"),
                kind: SubjectKind::Issue,
                url: Some(format!(
                    "https://api.github.com/repos/org/repo/issues/{number}"
                )),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "org/repo".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        }
    }

    /// Issue `number` of org/repo as an item.
    fn issue_item(number: u64) -> perch_github::Item {
        let at = "2026-09-19T12:00:00Z".parse().unwrap();
        let mut item = perch_github::Item::new("org/repo", at);
        item.id = format!("org/repo#{number}");
        item.subject = Some(perch_github::ItemSubject::Issue(perch_github::Issue {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            number,
            title: format!("Issue {number}"),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "u".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            repo_full_name: "org/repo".into(),
            comments: 0,
            created_at: Some(at),
            updated_at: at,
        }));
        item
    }

    /// The commands sent since the last call.
    fn sent(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>,
    ) -> Vec<crate::backend::Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    /// A window with nothing open on a recording backend, the window with
    /// the keyboard after a press on the empty pane, as a key that types
    /// needs a view that has it.
    fn empty_window(
        state: AppState,
        review: ReviewState,
    ) -> (
        HeadlessHarness,
        tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>,
    ) {
        let (backend, commands) = Backend::recording();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let pane = named(harness.root_id(), "pane-body")
            .expect("no empty pane")
            .get_visual_rect_no_clip()
            .center();
        harness.click(pane.x, pane.y);
        harness.rebuild();
        harness.paint();
        (harness, commands)
    }

    fn press(harness: &mut HeadlessHarness, code: Code, ch: &str, modifiers: Modifiers) {
        harness.dispatch_event(key(code, Key::Character(ch.into()), modifiers));
        harness.rebuild();
        harness.paint();
    }

    /// `e` marks the inbox row under the pointer Done, and Shift-U marks
    /// it unread, but not one unread already; with the pointer elsewhere
    /// they are for the open item's notification. Typed in the sidebar's
    /// search, `e` is an `e`.
    #[test]
    fn e_and_shift_u_mark_the_hovered_row_or_the_open_item() {
        use crate::backend::Command;
        let state = AppState::new();
        let review = ReviewState::new();
        state.notifications.set(vec![
            issue_thread("1", 1, false),
            issue_thread("2", 2, true),
        ]);
        let (mut harness, mut commands) = empty_window(state, review);

        state.hovered_row.set(Some("thread:1".into()));
        press(&mut harness, Code::KeyE, "e", Modifiers::empty());
        let done = sent(&mut commands);
        assert!(
            matches!(&done[..], [Command::DoneNotification { thread_id, .. }] if thread_id == "1"),
            "{done:?}"
        );
        press(&mut harness, Code::KeyU, "U", Modifiers::SHIFT);
        let unread = sent(&mut commands);
        assert!(
            matches!(&unread[..], [Command::MarkNotificationUnread { thread_id }] if thread_id == "1"),
            "{unread:?}"
        );
        state.hovered_row.set(Some("thread:2".into()));
        press(&mut harness, Code::KeyU, "U", Modifiers::SHIFT);
        assert!(sent(&mut commands).is_empty(), "unread already");

        // The pointer off the list: the open item's notification.
        state.hovered_row.set(None);
        press(&mut harness, Code::KeyE, "e", Modifiers::empty());
        assert!(sent(&mut commands).is_empty(), "nothing open, nothing done");
        review.item.set(Some(std::sync::Arc::new(issue_item(2))));
        press(&mut harness, Code::KeyE, "e", Modifiers::empty());
        let done = sent(&mut commands);
        assert!(
            done.iter().any(
                |c| matches!(c, Command::DoneNotification { thread_id, .. } if thread_id == "2")
            ),
            "{done:?}"
        );

        let search = state.search_box.get().expect("no sidebar search");
        search.request_focus();
        harness.rebuild();
        harness.paint();
        press(&mut harness, Code::KeyE, "e", Modifiers::empty());
        assert!(sent(&mut commands).is_empty(), "`e` was typed");
        assert_eq!(state.item_query.get(), "e");
    }

    /// `j` opens the first row with none open, then the row after the open
    /// one, and `k` the one before; past either end they do nothing.
    #[test]
    fn j_and_k_open_the_next_and_previous_rows() {
        use crate::backend::Command;
        use crate::items::SidebarTab;
        let state = AppState::new();
        let review = ReviewState::new();
        let item = |id: &str, minute: u32| {
            let at = format!("2026-09-18T10:{minute:02}:00Z").parse().unwrap();
            let mut item = perch_github::Item::new("slatedb/slatedb", at);
            item.id = id.into();
            item.checkout = Some(perch_github::ItemCheckout {
                branch: id.into(),
                base_ref: "main".into(),
                head_sha: "h".into(),
                path: std::path::PathBuf::from(format!("/tmp/{id}")),
                owned: true,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: None,
                source: None,
            });
            item
        };
        state.items.set(vec![item("a", 2), item("b", 1)]);
        crate::ui::sidebar::set_filters(state, |f| f.tab = SidebarTab::Worktrees);
        let order: Vec<String> = state.visible_items().into_iter().map(|i| i.id).collect();
        assert_eq!(order.len(), 2);
        let (mut harness, mut commands) = empty_window(state, review);
        let opened = |commands: &mut _| -> Vec<String> {
            sent(commands)
                .into_iter()
                .filter_map(|c| match c {
                    Command::OpenItem { id, .. } => Some(id),
                    _ => None,
                })
                .collect()
        };

        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[0].clone()]);
        assert_eq!(state.open_item.get().as_ref(), Some(&order[0]));
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[1].clone()]);
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert!(opened(&mut commands).is_empty(), "past the last row");
        press(&mut harness, Code::KeyK, "k", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[0].clone()]);
        press(&mut harness, Code::KeyK, "k", Modifiers::empty());
        assert!(opened(&mut commands).is_empty(), "past the first row");
    }

    /// `j` works as a window opens, before any click, and after the
    /// keyboard has been let go, as Open… and Go to File let it go as they
    /// shut: the window has it whenever nobody does.
    #[test]
    fn j_works_with_nothing_holding_the_keyboard() {
        use crate::backend::Command;
        use crate::items::SidebarTab;
        let state = AppState::new();
        let review = ReviewState::new();
        let item = |id: &str, minute: u32| {
            let at = format!("2026-09-18T10:{minute:02}:00Z").parse().unwrap();
            let mut item = perch_github::Item::new("slatedb/slatedb", at);
            item.id = id.into();
            item.checkout = Some(perch_github::ItemCheckout {
                branch: id.into(),
                base_ref: "main".into(),
                head_sha: "h".into(),
                path: std::path::PathBuf::from(format!("/tmp/{id}")),
                owned: true,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: None,
                source: None,
            });
            item
        };
        state.items.set(vec![item("a", 2), item("b", 1)]);
        crate::ui::sidebar::set_filters(state, |f| f.tab = SidebarTab::Worktrees);
        let order: Vec<String> = state.visible_items().into_iter().map(|i| i.id).collect();
        let (backend, mut commands) = Backend::recording();
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        let opened = |commands: &mut _| -> Vec<String> {
            sent(commands)
                .into_iter()
                .filter_map(|c| match c {
                    Command::OpenItem { id, .. } => Some(id),
                    _ => None,
                })
                .collect()
        };

        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[0].clone()]);
        harness.root_id().clear_focus();
        harness.rebuild();
        harness.paint();
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[1].clone()]);
    }

    /// Open… up as a window opens, as the Perch menu's Open… with no window
    /// has it, keeps the keyboard: the window takes it only when nobody
    /// has asked for it.
    #[test]
    fn a_box_made_with_the_window_keeps_the_keyboard() {
        let state = AppState::new();
        let review = ReviewState::new();
        open_box::show(state, None);
        let root = TestRoot::new();
        let view = app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
        let text = named(harness.root_id(), "open-box-text").expect("no box");
        assert!(harness.is_focused(text), "the window took the keyboard");
    }

    /// The view with the keyboard under `id`, if any.
    fn focused_under(harness: &HeadlessHarness, id: floem::ViewId) -> Option<floem::ViewId> {
        if harness.is_focused(id) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| focused_under(harness, child))
    }

    /// `j` from a control on the item that Tab moved the keyboard to opens
    /// the next row, taking the control with the item, and the next `j`
    /// goes on: the window takes the keyboard after Tab as after a click.
    #[test]
    fn j_goes_on_after_tab_left_the_keyboard_on_the_item() {
        use crate::backend::Command;
        use crate::items::SidebarTab;
        let (backend, mut commands) = Backend::recording();
        let (mut harness, state, review) = window_with_a_file_open_on(Rc::new(backend));
        let item = |id: &str, minute: u32| {
            let at = format!("2026-09-18T10:{minute:02}:00Z").parse().unwrap();
            let mut item = perch_github::Item::new("slatedb/slatedb", at);
            item.id = id.into();
            item.checkout = Some(perch_github::ItemCheckout {
                branch: id.into(),
                base_ref: "main".into(),
                head_sha: "h".into(),
                path: std::path::PathBuf::from(format!("/tmp/{id}")),
                owned: true,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: None,
                source: None,
            });
            item
        };
        state.items.set(vec![item("a", 2), item("b", 1)]);
        crate::ui::sidebar::set_filters(state, |f| f.tab = SidebarTab::Worktrees);
        let order: Vec<String> = state.visible_items().into_iter().map(|i| i.id).collect();
        // Conversation, as the code editor keeps Tab for itself.
        review.show_tab(crate::review::model::ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        let pane = harness.root_id();
        let mut on = None;
        for _ in 0..200 {
            harness.dispatch_event(key(
                Code::Tab,
                Key::Named(NamedKey::Tab),
                Modifiers::empty(),
            ));
            harness.rebuild();
            harness.paint();
            // A control on the item, which goes with it: not the
            // sidebar's, nor a box.
            on = focused_under(&harness, pane).filter(|id| {
                let names: Vec<String> = std::iter::successors(Some(*id), floem::ViewId::parent)
                    .map(|v| v.debug_name())
                    .collect();
                !names.iter().any(|n| {
                    n.contains("TextInput") || n.contains("Editor View") || n.contains("drawer")
                }) && *id != harness.root_id()
            });
            if on.is_some() {
                break;
            }
        }
        let on = on.expect("Tab never reached a control on the item");
        sent(&mut commands);

        let opened = |commands: &mut _| -> Vec<String> {
            sent(commands)
                .into_iter()
                .filter_map(|c| match c {
                    Command::OpenItem { id, .. } => Some(id),
                    _ => None,
                })
                .collect()
        };
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[0].clone()]);
        assert!(!on.is_valid(), "the control stayed");
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[1].clone()]);
    }

    /// `j` from the code editor opens the next row, taking the editor that
    /// had the keyboard with the item, and the window has the keyboard
    /// then, so the next `j` goes on to the row after.
    #[test]
    fn j_goes_on_after_the_view_with_the_keyboard_has_gone() {
        use crate::backend::Command;
        use crate::items::SidebarTab;
        let (backend, mut commands) = Backend::recording();
        let (mut harness, state, _review) = window_with_a_file_open_on(Rc::new(backend));
        let item = |id: &str, minute: u32| {
            let at = format!("2026-09-18T10:{minute:02}:00Z").parse().unwrap();
            let mut item = perch_github::Item::new("slatedb/slatedb", at);
            item.id = id.into();
            item.checkout = Some(perch_github::ItemCheckout {
                branch: id.into(),
                base_ref: "main".into(),
                head_sha: "h".into(),
                path: std::path::PathBuf::from(format!("/tmp/{id}")),
                owned: true,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: None,
                source: None,
            });
            item
        };
        state.items.set(vec![item("a", 2), item("b", 1)]);
        crate::ui::sidebar::set_filters(state, |f| f.tab = SidebarTab::Worktrees);
        let order: Vec<String> = state.visible_items().into_iter().map(|i| i.id).collect();
        assert_eq!(order.len(), 2);
        harness.rebuild();
        harness.paint();

        let code = named(harness.root_id(), "tinted editor").expect("no editor in the window");
        let rect = code.get_visual_rect_no_clip();
        let (x, y) = (rect.x0 + 200.0, rect.y0 + 40.0);
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        let editor = harness
            .element_id_at(x, y)
            .expect("nothing under the click");
        assert!(
            harness.is_focused(editor.owning_id()),
            "the editor has no keyboard"
        );
        sent(&mut commands);

        let opened = |commands: &mut _| -> Vec<String> {
            sent(commands)
                .into_iter()
                .filter_map(|c| match c {
                    Command::OpenItem { id, .. } => Some(id),
                    _ => None,
                })
                .collect()
        };
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[0].clone()]);
        assert!(!editor.owning_id().is_valid(), "the editor stayed");
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec![order[1].clone()]);
    }

    /// `j` in the inbox opens notifications as their rows do, passing over
    /// one that opens in the browser, and goes on from the one opening.
    #[test]
    fn j_in_the_inbox_passes_over_a_row_that_opens_in_the_browser() {
        use crate::backend::Command;
        let state = AppState::new();
        let review = ReviewState::new();
        let mut release = issue_thread("r", 9, true);
        release.subject.kind = perch_github::SubjectKind::Release;
        state.notifications.set(vec![
            issue_thread("1", 1, true),
            release,
            issue_thread("2", 2, true),
        ]);
        let (mut harness, mut commands) = empty_window(state, review);
        let opened = |commands: &mut _| -> Vec<String> {
            sent(commands)
                .into_iter()
                .filter_map(|c| match c {
                    Command::OpenNotification { thread, .. } => Some(thread.id),
                    _ => None,
                })
                .collect()
        };
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec!["1".to_string()]);
        // Still opening: the pane has no item to say which row it is.
        assert!(review.opening.get());
        press(&mut harness, Code::KeyJ, "j", Modifiers::empty());
        assert_eq!(opened(&mut commands), vec!["2".to_string()]);
        assert!(crate::ui::widgets::opened().is_empty(), "no browser");
    }

    /// Cmd-R fetches the open item again; Cmd-Shift-C copies its link and
    /// Cmd-Shift-O opens it, or the inbox row under the pointer; Cmd-Shift-R
    /// opens and shuts Review.
    #[test]
    fn the_item_keys_refresh_copy_open_and_review() {
        use crate::backend::Command;
        let (backend, mut commands) = Backend::recording();
        let (mut harness, state, review) = window_with_a_file_open_on(Rc::new(backend));
        sent(&mut commands);

        press(&mut harness, Code::KeyR, "r", OS_MOD);
        let refreshed = sent(&mut commands);
        assert!(
            matches!(&refreshed[..], [Command::RefreshOpenItem { item_id }] if item_id == "wt"),
            "{refreshed:?}"
        );

        // A worktree of the reader's own is nowhere on GitHub.
        crate::ui::widgets::copied();
        crate::ui::widgets::opened();
        press(&mut harness, Code::KeyC, "c", OS_MOD | Modifiers::SHIFT);
        assert!(crate::ui::widgets::copied().is_empty());
        state.notifications.set(vec![issue_thread("1", 1, true)]);
        state.hovered_row.set(Some("thread:1".into()));
        press(&mut harness, Code::KeyO, "o", OS_MOD | Modifiers::SHIFT);
        assert_eq!(
            crate::ui::widgets::opened(),
            vec!["https://github.com/org/repo/issues/1".to_string()]
        );
        state.hovered_row.set(None);
        let mut item = (*review.item.get().unwrap()).clone();
        item.subject = issue_item(5).subject;
        review.item.set(Some(std::sync::Arc::new(item)));
        press(&mut harness, Code::KeyC, "c", OS_MOD | Modifiers::SHIFT);
        assert_eq!(
            crate::ui::widgets::copied(),
            vec!["https://github.com/org/repo/issues/5".to_string()]
        );
        press(&mut harness, Code::KeyO, "o", OS_MOD | Modifiers::SHIFT);
        assert_eq!(
            crate::ui::widgets::opened(),
            vec!["https://github.com/org/repo/issues/5".to_string()]
        );

        press(&mut harness, Code::KeyR, "r", OS_MOD | Modifiers::SHIFT);
        assert!(review.review_open.get(), "Cmd-Shift-R opens Review");
        press(&mut harness, Code::KeyR, "r", OS_MOD | Modifiers::SHIFT);
        assert!(!review.review_open.get(), "and shuts it");
    }

    /// Custom bindings run the same actions as the buttons and menu rows.
    #[test]
    fn custom_bindings_run_their_actions() {
        use crate::items::SidebarTab;
        let _keymap = crate::keymap::install(crate::keymap::with_bindings(
            r#"
            toggle_wrap = "alt+z"
            copy_branch_name = "super+alt+b"
            send_feedback = "super+alt+f"
            worktrees = "ctrl+2"
            browse = "ctrl+3"
            inbox = "ctrl+1"
            start_work = "super+alt+s"
            "#,
        ));
        let (mut harness, state, review) = window_and_state_with_a_file_open();

        let wrap = review.wrap.get();
        press(&mut harness, Code::KeyZ, "z", Modifiers::ALT);
        assert_eq!(review.wrap.get(), !wrap);

        crate::ui::widgets::copied();
        press(&mut harness, Code::KeyB, "b", OS_MOD | Modifiers::ALT);
        assert_eq!(
            crate::ui::widgets::copied(),
            vec!["perch/pr-812".to_string()]
        );

        let tab = || state.item_filters.with(|f| f.tab);
        press(&mut harness, Code::Digit2, "2", Modifiers::CONTROL);
        assert_eq!(tab(), SidebarTab::Worktrees);
        press(&mut harness, Code::Digit3, "3", Modifiers::CONTROL);
        assert_eq!(tab(), SidebarTab::Browse);
        press(&mut harness, Code::Digit1, "1", Modifiers::CONTROL);
        assert_eq!(tab(), SidebarTab::Inbox);

        press(&mut harness, Code::KeyF, "f", OS_MOD | Modifiers::ALT);
        assert!(review.feedback_open.get(), "the feedback dropdown is up");
        press(&mut harness, Code::KeyF, "f", OS_MOD | Modifiers::ALT);
        assert!(!review.feedback_open.get());

        press(&mut harness, Code::KeyS, "s", OS_MOD | Modifiers::ALT);
        assert!(
            state.start_work_confirm.with(Option::is_some),
            "Start work asks"
        );
    }
}
