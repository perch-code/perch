//! The drawer on the right: the open item's files, as a tree, on Code.
//!
//! It is part of the main pane, under Code's tabs and beside the file,
//! so opening it moves the body over rather than covering what is being
//! read; only when the window has no room for both does it lie over the
//! body's edge instead. The body is keyed elsewhere, so the open file,
//! its scroll and a composer survive the drawer opening and shutting.
//!
//! The tile at the right end of Code's tabs opens it, where Codex has
//! its folder, and Cmd-Shift-E does from anywhere, going to Code first
//! from Conversation. Both are toggles on Code. On Conversation the tree
//! is hidden, and comes back with Code.

use std::rc::Rc;
use std::time::Duration;

use floem::event::DragConfig;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate};
use floem::style::{CursorStyle, Position};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;

/// The word `meta` keeps under `drawer.view` for an open tree. Anything
/// else, `closed` or the `agents` an older Perch wrote, is a shut one.
const OPEN: &str = "files";

/// Whether the saved word is an open tree.
pub fn parse(value: &str) -> bool {
    value == OPEN
}

/// Where the drawer goes, and how wide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// Beside the main pane, taking its room from it.
    Docked(f64),
    /// Over the pane's right edge, at the width the reader set.
    Overlay(f64),
}

impl Placement {
    pub fn is_docked(self) -> bool {
        matches!(self, Placement::Docked(_))
    }
}

/// The room the window has for the drawer once the drawer on the left,
/// open `sidebar` wide or shut, and the main pane's minimum have theirs.
fn room(window_width: f64, sidebar: Option<f64>) -> f64 {
    window_width - sidebar.unwrap_or(0.0) - theme::MAIN_MIN_WIDTH
}

/// The drawer is docked while the room is at least its minimum, narrower
/// than the reader set it if that is what fits. With less room than that
/// it lies over the pane at the reader's width, since a pane squeezed
/// under its minimum has no room for its controls.
pub fn placement(window_width: f64, sidebar: Option<f64>, preferred: f64) -> Placement {
    let room = room(window_width, sidebar);
    if room >= theme::DRAWER_MIN_WIDTH {
        Placement::Docked(preferred.min(room))
    } else {
        Placement::Overlay(preferred)
    }
}

/// The placement as the window and the drawers are now.
fn current_placement(state: AppState) -> Placement {
    placement(
        state.window_width.get(),
        state.sidebar_open.get().then(|| state.sidebar_width.get()),
        state.drawer_width.get(),
    )
}

/// Open the tree. `opener` is what to give the keyboard back to when it
/// shuts; with none given, whatever was recorded when it opened stands.
pub fn show(state: AppState, backend: &Backend, opener: Option<floem::ViewId>) {
    if let Some(opener) = opener {
        state.drawer_opener.set(Some(opener));
    }
    if !state.tree_open.get_untracked() {
        state.tree_open.set(true);
    }
    backend.set_meta("drawer.view", OPEN);
}

/// A control's press: shut the tree when it shows, else open it.
pub fn toggle(state: AppState, backend: &Backend, opener: Option<floem::ViewId>) {
    if floem::reactive::Effect::untrack(|| state.tree_shown()) {
        close(state, backend, true);
    } else {
        show(state, backend, opener);
    }
}

/// Shut the tree.
pub fn close(state: AppState, backend: &Backend, restore_focus: bool) {
    state.tree_open.set(false);
    backend.set_meta("drawer.view", "closed");
    if restore_focus {
        restore_opener(state);
    } else {
        state.drawer_opener.set(None);
    }
}

fn restore_opener(state: AppState) {
    let opener = state.drawer_opener.get_untracked();
    state.drawer_opener.set(None);
    if let Some(opener) = opener {
        opener.request_focus();
    }
}

/// The drawer, in the main pane's row after its body. Shut, it takes no
/// room at all. The tree is built as it comes and dropped as it goes, and
/// no more: a memo, since the signals under it are written more often
/// than they change.
pub fn panel(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let shown = Memo::new(move |_| state.tree_shown());
    let placement = Memo::new(move |_| current_placement(state));

    let content_backend = backend.clone();
    let content = dyn_container(
        move || shown.get(),
        move |shown| {
            if shown {
                crate::review::rail::drawer_view(state, review, content_backend.clone()).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.size_full().flex_col().min_width(0.0));

    Container::new(
        Stack::horizontal((resize_handle(state, backend), content)).style(|s| s.size_full()),
    )
    .style(move |s| {
        if !shown.get() {
            return s.hide();
        }
        let s = s
            .height_full()
            .flex_shrink(0.0_f32)
            .background(theme::canvas())
            .border_left(1.0)
            .border_color(theme::border())
            .position(Position::Relative);
        match placement.get() {
            Placement::Docked(width) => s.width(width),
            // Over the body's right edge, from under the tabs to the
            // window's bottom, with a shadow to say so.
            Placement::Overlay(width) => s
                .width(width)
                .max_width_full()
                .position(Position::Absolute)
                .inset_top(0.0)
                .inset_right(0.0)
                .inset_bottom(0.0)
                .z_index(60)
                .box_shadow_blur(18.0)
                .box_shadow_h_offset(-5.0)
                .box_shadow_color(theme::shadow()),
        }
    })
    .debug_name("right-drawer")
}

/// The bar at the top of the drawer: what the drawer holds, and the way
/// to shut it.
pub fn header_bar(
    leading: impl IntoView + 'static,
    title: impl Fn() -> String + 'static,
    trailing: impl IntoView + 'static,
    close: impl IntoView + 'static,
) -> impl IntoView {
    Stack::horizontal((
        leading,
        Label::derived(title).style(|s| {
            s.font_size(theme::TEXT_TITLE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
        }),
        trailing,
        Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
        close,
    ))
    .style(|s| {
        s.height(theme::DRAWER_HEADER_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .gap(8.0)
            .padding_left(12.0)
            .padding_right(8.0)
            .background(theme::canvas())
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    })
}

/// The control at the bar's right end that shuts the drawer.
pub fn close_button(state: AppState, backend: Rc<Backend>, debug: &'static str) -> impl View {
    crate::ui::widgets::icon_action(
        SidebarIcon::Close,
        debug,
        || true,
        move || close(state, &backend, true),
    )
}

/// The strip along the drawer's left edge that drags it wider or narrower.
///
/// Docked, the drag stops where the main pane would go under its minimum:
/// the width the reader sets is one the window can show.
fn resize_handle(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let start_width = RwSignal::new(theme::DRAWER_WIDTH);
    Empty::new()
        .on_event_stop(el::PointerDown, |cx, event: &PointerButtonEvent| {
            if let Some(pointer) = event
                .pointer
                .pointer_id
                .filter(|pointer| pointer.is_primary_pointer())
            {
                cx.request_pointer_capture(pointer);
            }
        })
        .on_event_stop(el::GainedPointerCapture, |cx, token| {
            cx.start_drag(
                *token,
                DragConfig {
                    threshold: 1.0,
                    animation_duration: Duration::ZERO,
                    easing: Rc::new(floem::easing::Linear),
                    custom_data: None,
                    track_targets: false,
                },
                false,
            );
        })
        .on_event_stop(el::DragStart, move |_, _: &floem::event::DragStartEvent| {
            start_width.set(state.drawer_width.get_untracked());
            state.drawer_resizing.set(true);
        })
        .on_event_stop(
            el::DragMove,
            move |cx, event: &floem::event::DragMoveEvent| {
                let inverse = cx.world_transform.inverse();
                let start = inverse * event.start_state.logical_point();
                let current = inverse * event.current_state.logical_point();
                let placement = floem::reactive::Effect::untrack(|| current_placement(state));
                let most = if placement.is_docked() {
                    let sidebar = state
                        .sidebar_open
                        .get_untracked()
                        .then(|| state.sidebar_width.get_untracked());
                    room(state.window_width.get_untracked(), sidebar).min(theme::DRAWER_MAX_WIDTH)
                } else {
                    theme::DRAWER_MAX_WIDTH
                };
                let width = (start_width.get_untracked() + start.x - current.x)
                    .clamp(theme::DRAWER_MIN_WIDTH, most);
                state.drawer_width.set(width);
            },
        )
        .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
            state.drawer_resizing.set(false);
            backend.set_meta(
                "drawer.width",
                &state.drawer_width.get_untracked().round().to_string(),
            );
        })
        .on_event_stop(
            el::DragCancel,
            move |_, _: &floem::event::DragCancelEvent| {
                state.drawer_resizing.set(false);
            },
        )
        .debug_name("drawer resize handle")
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(0.0)
                .inset_bottom(0.0)
                .inset_left(-4.0)
                .width(8.0)
                .z_index(3)
                .hover(|s| s.cursor(CursorStyle::ColResize))
        })
}

/// Keep the resize cursor while capture carries the pointer over the pane.
pub fn resize_cursor_sheet(state: AppState) -> impl IntoView {
    Empty::new().style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(101)
            .cursor(CursorStyle::ColResize)
            .apply_if(!state.drawer_resizing.get(), |s| s.hide())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drawer docks while the main pane keeps its minimum beside it,
    /// as wide as the reader set it or as wide as fits; with less room
    /// than its own minimum it lies over the pane instead.
    #[test]
    fn the_drawer_docks_while_the_pane_keeps_its_minimum() {
        let preferred = theme::DRAWER_WIDTH;
        // The default window with the sidebar shut: room to spare.
        assert_eq!(
            placement(1280.0, None, preferred),
            Placement::Docked(preferred)
        );
        // With the sidebar open at its default there is not: over the pane.
        assert_eq!(
            placement(1280.0, Some(theme::SIDEBAR_WIDTH), preferred),
            Placement::Overlay(preferred)
        );
        // A wider window with the sidebar open: docked, but narrower than
        // set, so the pane keeps its minimum.
        let window = theme::SIDEBAR_WIDTH + theme::MAIN_MIN_WIDTH + theme::DRAWER_MIN_WIDTH + 20.0;
        assert_eq!(
            placement(window, Some(theme::SIDEBAR_WIDTH), preferred),
            Placement::Docked(theme::DRAWER_MIN_WIDTH + 20.0)
        );
        // A pixel less room than the minimum, and it is over the pane.
        let window = theme::SIDEBAR_WIDTH + theme::MAIN_MIN_WIDTH + theme::DRAWER_MIN_WIDTH - 1.0;
        assert_eq!(
            placement(window, Some(theme::SIDEBAR_WIDTH), preferred),
            Placement::Overlay(preferred)
        );
        // Before the first layout the window has no width to speak of.
        assert_eq!(
            placement(0.0, None, preferred),
            Placement::Overlay(preferred)
        );
    }

    /// The tree open is saved as `files`, as the drawer on it was. The
    /// `agents` an older Perch saved for its inspector is a shut drawer.
    #[test]
    fn the_saved_word_is_an_open_tree_and_anything_else_is_shut() {
        assert!(parse(OPEN));
        for shut in ["closed", "agents", ""] {
            assert!(!parse(shut), "{shut} opened the tree");
        }
    }
}
