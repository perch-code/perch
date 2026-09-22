//! Window layout.

pub mod agents;
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

    let main_pane = floem::reactive::Memo::new(move |_| {
        (
            review.is_showing(),
            state.status.with(|s| matches!(s, Status::NoToken { .. })),
        )
    });

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
            // The main area: the review view over the sidebar, or the
            // empty state, which on a fresh install with no token is what
            // to do about that. A memo, because a signal tells its readers
            // about every write and the container builds its child again
            // on each: the status is written on every poll, and a page
            // built again is a scroll back at the top.
            dyn_container(
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
                        empty_pane(state, token_help(state)).into_any()
                    } else {
                        empty_pane(
                            state,
                            widgets::centered_message(
                                SidebarIcon::Branch,
                                || "Nothing open".to_string(),
                                || {
                                    "Pick a pull request, an issue, or a branch from the sidebar."
                                        .to_string()
                                },
                            ),
                        )
                        .into_any()
                    }
                },
            )
            // A zero basis: the pane takes what the drawer leaves, and
            // taffy never measures the whole item view unconstrained to
            // find out how wide it would like to be.
            .style(|s| {
                s.flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .min_width(0.0)
                    .height_full()
            }),
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
                // Escape closes the panel under the editor, references or
                // the call hierarchy, when nothing else claims it: a
                // composer, or something open under a sidebar row.
                let claimed = review.composer.with(Option::is_some)
                    || state.renaming.with(Option::is_some)
                    || state.confirm_delete.with(Option::is_some);
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

                // Cmd-W closes the open file, as in any editor; `v` checks it
                // as viewed, `]` and `[` go to the next and previous change,
                // `}` and `{` open the next and previous unviewed file. Files
                // only. The braces are shifted brackets: the physical codes
                // with Shift alone, so Cmd-[ stays back.
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
                .font_weight(theme::weight_body())
                .color(theme::FG)
                .background(theme::CANVAS)
                // A selection over a label, a fenced block's or a title's,
                // in the same blue as one over prose and in an editor.
                .custom_style_class(|s: floem::views::LabelCustomStyle| {
                    s.selection_color(theme::SELECTION_BG)
                })
        })
}

/// The main pane with nothing open in it. While the drawer is shut the
/// lights and the drawer control have nowhere else to be, so a bare bar
/// carries them; while it is open the drawer's strip has them and the
/// message runs to the top.
fn empty_pane(state: AppState, message: impl IntoView + 'static) -> impl IntoView {
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
        Container::new(message).style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
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

    /// Cmd-W as the window receives it.
    fn cmd_w() -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character("w".into()),
            code: Code::KeyW,
            location: Default::default(),
            modifiers: OS_MOD,
            repeat: false,
            is_composing: false,
        })
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

        // With nothing open, the key is left for whoever else wants it.
        harness.dispatch_event(cmd_w());
        harness.rebuild();
        assert_eq!(review.active.get(), None);
    }

    /// On Conversation the file's tab is out of sight, and Cmd-W leaves
    /// it be.
    #[test]
    fn cmd_w_on_conversation_leaves_the_files_tab_alone() {
        let (mut harness, review) = window_with_a_file_open();
        review.show_tab(crate::review::model::ItemTab::Conversation);
        harness.rebuild();
        harness.paint();

        harness.dispatch_event(cmd_w());
        harness.rebuild();
        assert_eq!(review.active.get().as_deref(), Some("src/x.rs"));
        assert_eq!(review.tabs.with(Vec::len), 1);
    }
}
