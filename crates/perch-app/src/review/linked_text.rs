//! Wrapped rich text whose links can be followed, one click at a time.
//!
//! The text comes from an agent or from GitHub, so a link's words say
//! nothing reliable about where it goes. Nothing happens on hover but a
//! tooltip with the whole destination, and a click opens it in the
//! browser, HTTP(S) only. floem's rich text view keeps its layout to
//! itself, so this wraps one and lays the same text out again for
//! hit-testing, at the width the wrapped view was given.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use floem::action::{add_overlay, exec_after, remove_overlay, TimerToken};
use floem::context::{EventCx, UpdateCx};
use floem::event::{Event, EventPropagation, Phase};
use floem::kurbo::{Point, Rect};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::style::CursorStyle;
use floem::text::{AttrsList, OverflowWrap, TextLayout, TextWrapMode};
use floem::ui_events::pointer::{PointerButton, PointerButtonEvent, PointerEvent};
use floem::views::{rich_text, Decorators};
use floem::{IntoView, View, ViewId};

use crate::review::cards::tip_body;
use crate::ui::widgets::open_in_browser;

/// How long the pointer rests on a link before its destination shows.
const TIP_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

struct Link {
    range: Range<usize>,
    url: String,
}

/// A rich text view with links, see the module.
pub struct LinkedText {
    id: ViewId,
    text: String,
    attrs: AttrsList,
    links: Vec<Link>,
    /// The layout used for hit-testing, and the width it was made for.
    shadow: RefCell<Option<(f32, TextLayout)>>,
    /// The link under the pointer, as an index into `links`. A signal so
    /// the cursor style can follow it.
    hovered: RwSignal<Option<usize>>,
    /// The link the primary button went down on, until it comes up.
    pressed: Option<usize>,
    /// A timer that will show the tooltip for the link it names, unless
    /// the pointer moves first, and where the pointer was.
    pending_tip: Option<(usize, Point, TimerToken)>,
    tip: Rc<RefCell<Option<ViewId>>>,
}

/// `text` with `attrs`, where each of `links` is a byte range of the text
/// and its destination.
pub fn linked_text(
    text: String,
    attrs: AttrsList,
    links: Vec<(Range<usize>, String)>,
) -> impl IntoView {
    let id = ViewId::new();
    let again = (text.clone(), attrs.clone());
    let child =
        rich_text(text.clone(), attrs.clone(), move || again.clone()).style(|s| s.width_full());
    id.set_children([child]);
    let hovered = RwSignal::new(None);
    let tip = Rc::new(RefCell::new(None));
    let view = LinkedText {
        id,
        text,
        attrs,
        links: links
            .into_iter()
            .map(|(range, url)| Link { range, url })
            .collect(),
        shadow: RefCell::new(None),
        hovered,
        pressed: None,
        pending_tip: None,
        tip: tip.clone(),
    };
    view.style(move |s| {
        if hovered.get().is_some() {
            s.cursor(CursorStyle::Pointer)
        } else {
            s
        }
    })
    .on_cleanup(move || {
        if let Some(overlay) = tip.borrow_mut().take() {
            remove_overlay(overlay);
        }
    })
}

impl LinkedText {
    /// The link under `point`, in this view's coordinates.
    fn link_at(&self, point: Point) -> Option<usize> {
        let child = *self.id.children().first()?;
        let width = text_width(child)?;
        if width <= 0.0 || width.is_nan() {
            return None;
        }
        let mut shadow = self.shadow.borrow_mut();
        let stale = shadow.as_ref().is_none_or(|(w, _)| *w != width);
        if stale {
            *shadow = Some((width, self.layout_for(width)));
        }
        let (_, layout) = shadow.as_ref().expect("just built");
        // The wrapped view fills this one, so its text starts at the
        // child's own origin.
        let origin = child.get_layout_rect().origin();
        link_under(layout, &self.links, point - origin.to_vec2())
    }

    /// The text laid out as the wrapped view lays it out at `width`.
    fn layout_for(&self, width: f32) -> TextLayout {
        layout_at(&self.text, self.attrs.clone(), width)
    }
}

/// The width the rich text view laid its text out for: that of the leaf
/// taffy node it measures the text in, which is narrower than the view
/// when a single word is wider than the column, since a flex item's
/// minimum is its content. The painted lines break at this width, so
/// this is the width the shadow must break at.
fn text_width(rich_text: ViewId) -> Option<f32> {
    let taffy = rich_text.taffy();
    let taffy = taffy.borrow();
    let leaf = *taffy.children(rich_text.taffy_node()).ok()?.first()?;
    Some(taffy.layout(leaf).ok()?.content_box_width())
}

/// `text` wrapped at `width` the way the rich text view wraps it: only
/// when it does not fit.
fn layout_at(text: &str, attrs: AttrsList, width: f32) -> TextLayout {
    let mut layout = TextLayout::new();
    layout.set_text_wrap_mode(TextWrapMode::Wrap);
    layout.set_overflow_wrap(OverflowWrap::Normal);
    layout.set_text(text, attrs, None);
    if layout.size().width as f32 > width {
        layout.set_size(width, f32::MAX);
    }
    layout
}

/// Which of `links` is drawn under `point`, in the layout's coordinates.
/// A link that wraps is several rectangles; the point must be inside one,
/// not merely nearest to it.
fn link_under(layout: &TextLayout, links: &[Link], point: Point) -> Option<usize> {
    links.iter().position(|link| {
        let selection = layout.selection_from_byte_range(link.range.start, link.range.end);
        let mut hit = false;
        layout.selection_geometry_with_line_metrics(&selection, |x0, y0, x1, y1| {
            hit |= Rect::new(x0, y0, x1, y1).contains(point);
        });
        hit
    })
}

impl LinkedText {
    fn set_hovered(&mut self, link: Option<usize>) {
        if self.hovered.get() != link {
            self.hovered.set(link);
        }
    }

    fn dismiss_tip(&mut self) {
        self.pending_tip = None;
        if let Some(overlay) = self.tip.borrow_mut().take() {
            remove_overlay(overlay);
        }
    }

    fn handle_pointer(&mut self, event: &PointerEvent, cx: &mut EventCx) -> EventPropagation {
        match event {
            PointerEvent::Move(update) => {
                let point = update.current.logical_point();
                let hit = self.link_at(point);
                if hit != self.hovered.get() {
                    self.dismiss_tip();
                    self.set_hovered(hit);
                    cx.window_state.request_paint(self.id);
                }
                if let Some(link) = hit {
                    if self.tip.borrow().is_none() {
                        // The tip appears once the pointer has rested on
                        // the link; every move starts the wait again.
                        let id = self.id;
                        let token = exec_after(TIP_DELAY, move |token| id.update_state(token));
                        self.pending_tip = Some((link, point, token));
                    }
                }
                EventPropagation::Continue
            }
            PointerEvent::Down(PointerButtonEvent { button, state, .. }) => {
                self.dismiss_tip();
                if *button != Some(PointerButton::Primary) {
                    return EventPropagation::Continue;
                }
                self.pressed = self.link_at(state.logical_point());
                if self.pressed.is_some() {
                    EventPropagation::Stop
                } else {
                    EventPropagation::Continue
                }
            }
            PointerEvent::Up(PointerButtonEvent { button, state, .. }) => {
                let pressed = self.pressed.take();
                if *button != Some(PointerButton::Primary) {
                    return EventPropagation::Continue;
                }
                match pressed {
                    Some(link) if self.link_at(state.logical_point()) == Some(link) => {
                        open_in_browser(&self.links[link].url);
                        EventPropagation::Stop
                    }
                    _ => EventPropagation::Continue,
                }
            }
            PointerEvent::Leave(_) | PointerEvent::Cancel(_) => {
                self.dismiss_tip();
                self.pressed = None;
                self.set_hovered(None);
                EventPropagation::Continue
            }
            PointerEvent::Scroll(_) | PointerEvent::Gesture(_) => {
                self.dismiss_tip();
                EventPropagation::Continue
            }
            PointerEvent::Enter(_) => EventPropagation::Continue,
        }
    }
}

impl View for LinkedText {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        "LinkedText".into()
    }

    fn update(&mut self, _cx: &mut UpdateCx, state: Box<dyn std::any::Any>) {
        let Ok(token) = state.downcast::<TimerToken>() else {
            return;
        };
        let Some((link, point, pending)) = self.pending_tip else {
            return;
        };
        if pending != *token {
            return;
        }
        self.pending_tip = None;
        let url = self.links[link].url.clone();
        let text = if url.starts_with("https://") || url.starts_with("http://") {
            url
        } else {
            format!("{url} (not opened: only http and https links open)")
        };
        let at = self.id.get_visual_origin() + point.to_vec2() + (0.0, 14.0);
        let overlay = add_overlay(
            tip_body(move || text.clone()).style(move |s| s.inset_left(at.x).inset_top(at.y)),
        );
        overlay.set_style_parent(self.id);
        *self.tip.borrow_mut() = Some(overlay);
    }

    fn style_pass(&mut self, _cx: &mut floem::context::StyleCx<'_>) {
        if self.tip.borrow().is_some() && self.id.is_hidden() {
            self.dismiss_tip();
        }
    }

    /// Pointer events are for the wrapped text; they are read here on the
    /// way down so the text itself need know nothing about links.
    fn event_capture(&mut self, cx: &mut EventCx) -> EventPropagation {
        match &cx.event {
            Event::Pointer(pointer) => {
                let pointer = pointer.clone();
                self.handle_pointer(&pointer, cx)
            }
            Event::Key(_) => {
                self.dismiss_tip();
                EventPropagation::Continue
            }
            _ => EventPropagation::Continue,
        }
    }

    /// Leaving is told to each view the pointer was over, this one
    /// included, and only to it: there is no capture pass for it. The
    /// same goes for a pointer over this view's own padding or border,
    /// where it is the target, so a move off the text onto a quote's
    /// gutter is seen too.
    fn event(&mut self, cx: &mut EventCx) -> EventPropagation {
        if let Event::Pointer(pointer) = &cx.event {
            let own = cx.phase == Phase::Target;
            if own || matches!(pointer, PointerEvent::Leave(_) | PointerEvent::Cancel(_)) {
                let pointer = pointer.clone();
                return self.handle_pointer(&pointer, cx);
            }
        }
        EventPropagation::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::text::Attrs;

    fn links(text: &str, needle: &str, url: &str) -> Vec<Link> {
        let start = text.find(needle).unwrap();
        vec![Link {
            range: start..start + needle.len(),
            url: url.into(),
        }]
    }

    fn line_rects(layout: &TextLayout, link: &Link) -> Vec<Rect> {
        let selection = layout.selection_from_byte_range(link.range.start, link.range.end);
        let mut rects = Vec::new();
        layout.selection_geometry_with_line_metrics(&selection, |x0, y0, x1, y1| {
            rects.push(Rect::new(x0, y0, x1, y1));
        });
        rects
    }

    #[test]
    fn only_a_point_on_the_link_hits_it() {
        let text = "Read the docs at example before deciding.";
        let links = links(text, "docs", "https://example.com/docs");
        let attrs = AttrsList::new(Attrs::new().font_size(14.0));
        let layout = layout_at(text, attrs, 1000.0);
        let rects = line_rects(&layout, &links[0]);
        assert_eq!(rects.len(), 1, "{rects:?}");
        let rect = rects[0];
        assert!(rect.x0 > 0.0 && rect.width() > 0.0, "{rect:?}");
        assert_eq!(link_under(&layout, &links, rect.center()), Some(0));
        // Just left of the link is the word before it, and past the end
        // of the line is nothing, however near the link's last glyph.
        assert_eq!(
            link_under(&layout, &links, Point::new(rect.x0 - 3.0, rect.center().y)),
            None
        );
        assert_eq!(
            link_under(&layout, &links, Point::new(rect.x1 + 3.0, rect.center().y)),
            None
        );
        assert_eq!(
            link_under(&layout, &links, Point::new(900.0, rect.center().y)),
            None
        );
        assert_eq!(
            link_under(&layout, &links, Point::new(rect.center().x, rect.y1 + 40.0)),
            None
        );
    }

    #[test]
    fn a_link_that_wraps_is_hit_on_both_lines() {
        let text = "A sentence that ends in a fairly long link to the documentation site indeed.";
        let links = links(
            text,
            "fairly long link to the documentation site",
            "https://example.com",
        );
        let attrs = AttrsList::new(Attrs::new().font_size(14.0));
        let narrow = layout_at(text, attrs.clone(), 220.0);
        assert!(narrow.visual_line_count() > 1, "the text wraps at 220px");
        let rects = line_rects(&narrow, &links[0]);
        assert!(rects.len() >= 2, "the link spans lines: {rects:?}");
        for rect in &rects {
            assert_eq!(
                link_under(&narrow, &links, rect.center()),
                Some(0),
                "{rect:?}"
            );
        }
        let wide = layout_at(text, attrs, 5000.0);
        assert_eq!(wide.visual_line_count(), 1, "and not when there is room");
    }
}
