//! The title bar, drawn by the app.
//!
//! The window has no title bar of its own (see `main.rs`): the content runs
//! to the top and the app draws its own strip, with the traffic lights in
//! it. So the presses that the system would have taken on its bar land on
//! the app's, and the app has to do with them what the system would have.

use floem::action::{drag_window, toggle_window_maximized};
use floem::event::EventPropagation;
use floem::prelude::*;
use floem::ui_events::pointer::PointerButton;
use floem::views::Decorators;

/// Let a bar stand in for the window's title bar.
///
/// A press on the bar's own surface drags the window, and the second press
/// of a double-click zooms it, the way the system's bar does. The controls
/// in the bar keep their clicks: only a press whose deepest hit is the bar
/// itself counts. A label, an icon or a spacer that should read as part of
/// the bar rather than as a control opts in with `pointer_events_none()`,
/// which takes it out of hit testing so the press lands on the bar behind.
///
/// The zoom is keyed off the press rather than floem's `DoubleClick`, which
/// is minted on release. AppKit takes the pointer for the drag from the
/// first press, and the release that ends the drag never reaches the view;
/// the second press does, with its count.
pub fn title_bar<V: IntoView + 'static>(bar: V) -> impl IntoView {
    title_bar_with(bar, drag_window, toggle_window_maximized)
}

fn title_bar_with<V: IntoView + 'static>(
    bar: V,
    drag: impl Fn() + 'static,
    zoom: impl Fn() + 'static,
) -> impl IntoView {
    let bar = bar.into_intermediate();
    let id = bar.view_id();
    bar.on_event(el::PointerDown, move |cx, press: &PointerButtonEvent| {
        if press.button != Some(PointerButton::Primary) {
            return EventPropagation::Continue;
        }
        // A press on a control in the bar bubbles through here too. Not ours.
        let on_bar =
            cx.hit_path.as_deref().and_then(|path| path.last()) == Some(&id.get_element_id());
        if !on_bar {
            return EventPropagation::Continue;
        }
        match press.state.count {
            1 => drag(),
            2 => zoom(),
            // A third press in the same spot is neither: the second one
            // zoomed, and nothing needs dragging or zooming straight back.
            _ => {}
        }
        EventPropagation::Stop
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::views::{Container, Empty, Label, Stack};

    /// What the bar did, counted.
    #[derive(Clone, Default)]
    struct Counts {
        drags: Rc<Cell<usize>>,
        zooms: Rc<Cell<usize>>,
        clicks: Rc<Cell<usize>>,
    }

    /// A bar like the review header: a button at the left, a label that is
    /// part of the bar, a spacer, and a label that is left pickable, as a
    /// selectable one would be. All on one 60px strip 800px wide.
    fn harness() -> (HeadlessHarness, Counts) {
        let counts = Counts::default();
        let root = TestRoot::new();
        let clicks = counts.clicks.clone();
        let bar = Stack::horizontal((
            Container::new(Empty::new())
                .on_event_stop(el::Click, move |_, _| clicks.set(clicks.get() + 1))
                .style(|s| s.size(40.0, 28.0)),
            Label::derived(|| "part of the bar".to_string())
                .style(|s| s.width(150.0).pointer_events_none()),
            Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
            Label::derived(|| "a control".to_string()).style(|s| s.width(100.0)),
        ))
        .style(|s| s.width_full().height(60.0).items_center());
        let drags = counts.drags.clone();
        let zooms = counts.zooms.clone();
        let bar = title_bar_with(
            bar,
            move || drags.set(drags.get() + 1),
            move || zooms.set(zooms.get() + 1),
        );
        let view = Stack::vertical((
            bar,
            Empty::new().style(|s| s.width_full().flex_grow(1.0_f32)),
        ))
        .style(|s| s.size_full().flex_col());
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        (harness, counts)
    }

    // The button spans x 0..40 at y 16..44; the label 40..190; the spacer
    // 190..700; the pickable label 700..800.
    const BUTTON: (f64, f64) = (20.0, 30.0);
    const LABEL: (f64, f64) = (100.0, 30.0);
    const SPACER: (f64, f64) = (400.0, 30.0);
    const ABOVE_BUTTON: (f64, f64) = (20.0, 5.0);
    const PICKABLE: (f64, f64) = (750.0, 30.0);
    const BELOW_BAR: (f64, f64) = (400.0, 300.0);

    #[test]
    fn a_press_on_the_bar_drags_and_a_second_zooms() {
        let (mut harness, counts) = harness();

        harness.pointer_down(SPACER.0, SPACER.1);
        assert_eq!((counts.drags.get(), counts.zooms.get()), (1, 0));

        // The release may never come, so the second press is what counts.
        harness.double_click(SPACER.0, SPACER.1);
        assert_eq!((counts.drags.get(), counts.zooms.get()), (1, 1));
    }

    #[test]
    fn the_bar_is_wherever_nothing_in_it_is_a_control() {
        let (mut harness, counts) = harness();

        harness.pointer_down(LABEL.0, LABEL.1);
        harness.pointer_up(LABEL.0, LABEL.1);
        harness.pointer_down(ABOVE_BUTTON.0, ABOVE_BUTTON.1);
        harness.pointer_up(ABOVE_BUTTON.0, ABOVE_BUTTON.1);
        assert_eq!(counts.drags.get(), 2);
        assert_eq!(counts.clicks.get(), 0);
    }

    #[test]
    fn a_control_keeps_its_click() {
        let (mut harness, counts) = harness();

        harness.click(BUTTON.0, BUTTON.1);
        assert_eq!(counts.clicks.get(), 1);
        harness.double_click(BUTTON.0, BUTTON.1);
        assert_eq!(counts.clicks.get(), 2);
        assert_eq!((counts.drags.get(), counts.zooms.get()), (0, 0));

        // A child that was left pickable is a control as far as the bar
        // knows, whatever it looks like.
        harness.pointer_down(PICKABLE.0, PICKABLE.1);
        assert_eq!(counts.drags.get(), 0);
    }

    #[test]
    fn a_press_below_the_bar_is_not_its_business() {
        let (mut harness, counts) = harness();

        harness.pointer_down(BELOW_BAR.0, BELOW_BAR.1);
        harness.double_click(BELOW_BAR.0, BELOW_BAR.1);
        assert_eq!((counts.drags.get(), counts.zooms.get()), (0, 0));
    }

    #[test]
    fn only_the_primary_button() {
        let (mut harness, counts) = harness();

        harness.secondary_click(SPACER.0, SPACER.1);
        assert_eq!((counts.drags.get(), counts.zooms.get()), (0, 0));
    }
}
