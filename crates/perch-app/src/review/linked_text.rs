//! Wrapped rich text whose links can be followed, one click at a time,
//! whose struck spans are ruled through, and whose words can be dragged
//! over and copied.
//!
//! The text comes from an agent or from GitHub, so a link's words say
//! nothing reliable about where it goes. Nothing happens on hover but a
//! tooltip with the whole destination, and a click opens an address in
//! the browser, HTTP(S) only, or a place in the worktree in Files. floem's
//! rich text view keeps its layout to itself, so this wraps one and lays
//! the same text out again for hit-testing, at the width the wrapped view
//! was given.
//!
//! floem's text attributes have no strikethrough either, and the rich
//! text view draws only glyphs. So the same second layout says where a
//! struck range's glyphs sit, and a rule is drawn through them where the
//! font puts its strikeout, in the glyphs' colour. The rule goes down
//! before the child's glyphs, since floem reaches no `post_paint`
//! through a boxed view, and a rule under glyphs of its own colour reads
//! the same as one over them.
//!
//! Nor does the rich text view select, where a label does. So the second
//! layout also says which glyph the primary button went down on and
//! which the pointer has been dragged to, and a selection runs between
//! them while the button is held, then stays until a click or the focus
//! leaving clears it, for Cmd-C to copy. A double click takes the word
//! under the pointer and a triple the whole run; a drag from a word grows
//! by words. The selection is painted from the shadow, under the glyphs
//! as a browser paints one. It is this view's alone, as a label's is the
//! label's: a drag does not run on into the next paragraph.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use floem::action::{add_overlay, exec_after, remove_overlay, TimerToken};
use floem::context::{EventCx, PaintCx, UpdateCx};
use floem::event::{Event, EventPropagation, FocusEvent, Phase};
use floem::kurbo::{Point, Rect};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::style::{CursorStyle, Selectable};
use floem::text::{
    AttrsList, Cursor, OverflowWrap, Selection, TextLayout, TextSelection, TextWrapMode,
};
use floem::ui_events::keyboard::{Code, KeyState, KeyboardEvent};
use floem::ui_events::pointer::{PointerButton, PointerButtonEvent, PointerEvent, PointerId};
use floem::views::{rich_text, Decorators};
use floem::{IntoView, Renderer, View, ViewId};

use crate::review::cards::tip_body;
use crate::theme;
use crate::ui::widgets::{copy_to_clipboard, open_in_browser};
use crate::ui::OS_MOD;

/// How long the pointer rests on a link before its destination shows.
const TIP_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// How far the pointer moves with the button down before that is a drag
/// and not a click, in logical pixels.
const DRAG_SLOP: f64 = 2.0;

struct Link {
    range: Range<usize>,
    target: Target,
}

/// Where a link goes.
#[derive(Clone)]
pub enum Target {
    /// An address, opened in the browser when it is HTTP(S).
    Url(String),
    /// A file, a line of one, or a symbol's definitions in the worktree,
    /// which `open` goes to in Files. `tip` says where.
    Local { tip: String, open: Rc<dyn Fn()> },
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Url(url) => f.debug_tuple("Url").field(url).finish(),
            Target::Local { tip, .. } => f.debug_struct("Local").field("tip", tip).finish(),
        }
    }
}

/// Two links are the same when they go to the same address, or say they
/// go to the same place.
impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Target::Url(a), Target::Url(b)) => a == b,
            (Target::Local { tip: a, .. }, Target::Local { tip: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// A byte range to strike through, and the colour of its text.
pub type Strike = (Range<usize>, Color);

floem::prop_extractor! {
    SelectProps {
        selectable: Selectable,
    }
}

/// A selection, from the primary button going down to the focus leaving.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Selecting {
    None,
    /// The button went down at `origin`, off any link, by the glyph edge
    /// `from`. A drag past the slop starts a selection there; a release
    /// first was a click.
    Armed {
        origin: Point,
        from: Cursor,
    },
    /// The button is down and the selection's far end follows the pointer.
    Dragging(TextSelection),
    /// The button is up and the selection stays.
    Done(TextSelection),
}

/// A rich text view with links, see the module.
pub struct LinkedText {
    id: ViewId,
    text: String,
    attrs: AttrsList,
    links: Vec<Link>,
    strikes: Vec<Strike>,
    /// The layout used for hit-testing and for the rules, and the width
    /// it was made for.
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
    selecting: Selecting,
    /// Whether the text may be selected: inherited, and off inside a row
    /// that is a button.
    props: SelectProps,
}

/// `text` with `attrs`, where each of `links` is a byte range of the text
/// and where it goes, and each of `strikes` a byte range to rule through
/// in its colour.
pub fn linked_text(
    text: String,
    attrs: AttrsList,
    links: Vec<(Range<usize>, Target)>,
    strikes: Vec<Strike>,
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
            .map(|(range, target)| Link { range, target })
            .collect(),
        strikes,
        shadow: RefCell::new(None),
        hovered,
        pressed: None,
        pending_tip: None,
        tip: tip.clone(),
        selecting: Selecting::None,
        props: SelectProps::default(),
    };
    view.style(move |s| {
        // Focusable, so a drag can take the focus and keep its selection
        // until the focus goes, and so Cmd-C reaches it. Under the
        // pointer the text cursor, as over a label, unless the text is
        // not selectable or a link is under it.
        let s = s.focusable().with::<Selectable>(|s, selectable| {
            s.set_context_opt(
                floem::style::Cursor,
                selectable.def(|selectable| selectable.then_some(CursorStyle::Text)),
            )
        });
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
    /// The shadow layout at the width the wrapped view's text has now,
    /// built or rebuilt when that width is new, run through `with`.
    fn with_shadow<T>(&self, with: impl FnOnce(&TextLayout, Point) -> T) -> Option<T> {
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
        Some(with(layout, origin))
    }

    /// The link under `point`, in this view's coordinates.
    fn link_at(&self, point: Point) -> Option<usize> {
        self.with_shadow(|layout, origin| link_under(layout, &self.links, point - origin.to_vec2()))
            .flatten()
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

/// floem's selection around a parley one. floem makes its own from a
/// pair of cursors alone, which forgets that a selection began as a word
/// and grows by words; a collapsed one is made and the parley selection
/// put in it.
fn wrap(layout: &TextLayout, selection: Selection) -> TextSelection {
    let mut wrapped = layout.collapsed_selection(selection.anchor());
    *wrapped = selection;
    wrapped
}

/// Whether `key` is the copy shortcut.
fn is_copy(key: &KeyboardEvent) -> bool {
    key.state == KeyState::Down && key.modifiers == OS_MOD && key.code == Code::KeyC
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

    /// The selection, while there is one with something in it.
    fn selection(&self) -> Option<TextSelection> {
        match self.selecting {
            Selecting::Dragging(selection) | Selecting::Done(selection)
                if !selection.is_collapsed() =>
            {
                Some(selection)
            }
            _ => None,
        }
    }

    /// The selected text.
    fn selected_text(&self) -> Option<String> {
        let selection = self.selection()?;
        self.with_shadow(|layout, _| self.text[layout.selection_text_range(&selection)].to_string())
    }

    /// Whether the primary button is down for a selection, armed or drawn.
    fn dragging(&self) -> bool {
        matches!(
            self.selecting,
            Selecting::Armed { .. } | Selecting::Dragging(_)
        )
    }

    /// The primary button down at `point`, off any link, for the `count`th
    /// click in a row: one arms a drag from the nearest glyph edge, two
    /// takes the word there, and three or more the whole text. Whatever
    /// it begins, the pointer is captured so the drag and the release
    /// reach here wherever they land.
    fn press(&mut self, point: Point, count: u8, pointer: Option<PointerId>) {
        let len = self.text.len();
        let next = self
            .with_shadow(|layout, child| {
                let at = point - child.to_vec2();
                match count {
                    0 | 1 => layout.hit_test(at).map(|from| Selecting::Armed {
                        origin: point,
                        from,
                    }),
                    2 => {
                        TextLayout::clear_selection_geometry_cache();
                        let word = Selection::word_from_point(
                            layout.parley_layout(),
                            at.x as f32,
                            at.y as f32,
                        );
                        Some(Selecting::Dragging(wrap(layout, word)))
                    }
                    _ => {
                        TextLayout::clear_selection_geometry_cache();
                        Some(Selecting::Done(layout.selection_from_byte_range(0, len)))
                    }
                }
            })
            .flatten();
        let Some(next) = next else {
            return;
        };
        self.selecting = next;
        if let Some(pointer) = pointer {
            self.id.set_pointer_capture(pointer);
        }
        if self.selection().is_some() {
            self.id.request_focus();
        }
    }

    /// The pointer at `point` with the button down: from armed, past the
    /// slop, a selection from the pressed glyph edge to the one under the
    /// pointer; from there on, the far end follows.
    fn drag_to(&mut self, point: Point) -> bool {
        let next = match self.selecting {
            Selecting::Armed { origin, from } => {
                if origin.distance(point) <= DRAG_SLOP {
                    return false;
                }
                self.with_shadow(|layout, child| {
                    let to = layout.hit_test(point - child.to_vec2())?;
                    Some(layout.begin_selection(from, to))
                })
                .flatten()
            }
            Selecting::Dragging(selection) => self.with_shadow(|layout, child| {
                let at = point - child.to_vec2();
                wrap(
                    layout,
                    selection.extend_to_point(layout.parley_layout(), at.x as f32, at.y as f32),
                )
            }),
            Selecting::None | Selecting::Done(_) => None,
        };
        let Some(next) = next else {
            return false;
        };
        self.selecting = Selecting::Dragging(next);
        true
    }

    fn handle_pointer(&mut self, event: &PointerEvent, cx: &mut EventCx) -> EventPropagation {
        match event {
            PointerEvent::Move(update) => {
                let point = update.current.logical_point();
                if self.dragging() {
                    // A drag is a selection, not a hover: no link lights
                    // up or offers its tip under it.
                    let began = matches!(self.selecting, Selecting::Armed { .. });
                    if self.drag_to(point) {
                        if began {
                            self.id.request_focus();
                        }
                        cx.window_state.request_paint(self.id);
                    }
                    return EventPropagation::Continue;
                }
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
            PointerEvent::Down(PointerButtonEvent {
                button,
                state,
                pointer,
            }) => {
                self.dismiss_tip();
                if *button != Some(PointerButton::Primary) {
                    return EventPropagation::Continue;
                }
                // Any press clears what was selected, as in a browser.
                self.selecting = Selecting::None;
                cx.window_state.request_paint(self.id);
                let point = state.logical_point();
                self.pressed = self.link_at(point);
                if self.pressed.is_some() {
                    return EventPropagation::Stop;
                }
                if self.props.selectable() {
                    self.press(point, state.count, pointer.pointer_id);
                }
                EventPropagation::Continue
            }
            PointerEvent::Up(PointerButtonEvent { button, state, .. }) => {
                let pressed = self.pressed.take();
                if *button != Some(PointerButton::Primary) {
                    return EventPropagation::Continue;
                }
                self.selecting = match self.selecting {
                    Selecting::Dragging(selection) if !selection.is_collapsed() => {
                        Selecting::Done(selection)
                    }
                    Selecting::Done(selection) => Selecting::Done(selection),
                    Selecting::None | Selecting::Armed { .. } | Selecting::Dragging(_) => {
                        Selecting::None
                    }
                };
                cx.window_state.request_paint(self.id);
                match pressed {
                    Some(link) if self.link_at(state.logical_point()) == Some(link) => {
                        match &self.links[link].target {
                            Target::Url(url) => open_in_browser(url),
                            Target::Local { open, .. } => open(),
                        }
                        EventPropagation::Stop
                    }
                    _ => EventPropagation::Continue,
                }
            }
            PointerEvent::Leave(_) => {
                self.dismiss_tip();
                self.pressed = None;
                self.set_hovered(None);
                EventPropagation::Continue
            }
            PointerEvent::Cancel(_) => {
                self.dismiss_tip();
                self.pressed = None;
                self.set_hovered(None);
                if self.dragging() {
                    self.selecting = Selecting::None;
                    cx.window_state.request_paint(self.id);
                }
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

    fn paint(&mut self, cx: &mut PaintCx) {
        // The selection goes down first, under the rules and the glyphs.
        if let Some(selection) = self.selection() {
            if cx.window_state.is_focused(self.id) {
                let rects = self
                    .with_shadow(|layout, origin| {
                        let mut rects = Vec::new();
                        layout.selection_geometry_with_line_metrics(
                            &selection,
                            |x0, y0, x1, y1| {
                                rects.push(Rect::new(x0, y0, x1, y1) + origin.to_vec2());
                            },
                        );
                        rects
                    })
                    .unwrap_or_default();
                for rect in rects {
                    cx.fill(&rect, theme::SELECTION_BG, 0.0);
                }
            }
        }
        if self.strikes.is_empty() {
            return;
        }
        let len = self.text.len();
        let rules = self
            .with_shadow(|layout, origin| {
                rules(layout, len, &self.strikes)
                    .into_iter()
                    .map(|(rule, color)| (rule + origin.to_vec2(), color))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (rule, color) in rules {
            cx.fill(&rule, color, 0.0);
        }
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
        let text = match &self.links[link].target {
            Target::Url(url) if url.starts_with("https://") || url.starts_with("http://") => {
                url.clone()
            }
            Target::Url(url) => format!("{url} (not opened: only http and https links open)"),
            Target::Local { tip, .. } => tip.clone(),
        };
        let at = self.id.get_visual_origin() + point.to_vec2() + (0.0, 14.0);
        let overlay = add_overlay(
            tip_body(move || text.clone()).style(move |s| s.inset_left(at.x).inset_top(at.y)),
        );
        overlay.set_style_parent(self.id);
        *self.tip.borrow_mut() = Some(overlay);
    }

    fn style_pass(&mut self, cx: &mut floem::context::StyleCx<'_>) {
        if self.props.read(cx) && !self.props.selectable() && self.selecting != Selecting::None {
            self.selecting = Selecting::None;
            cx.window_state.request_paint(self.id);
        }
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
    /// gutter is seen too, and for the pointer while this view holds it
    /// for a drag. The focus and the keys come here as well: a selection
    /// lasts while this view has the focus, and Cmd-C copies it.
    fn event(&mut self, cx: &mut EventCx) -> EventPropagation {
        match &cx.event {
            Event::Pointer(pointer) => {
                let own = cx.phase == Phase::Target;
                if own || matches!(pointer, PointerEvent::Leave(_) | PointerEvent::Cancel(_)) {
                    let pointer = pointer.clone();
                    return self.handle_pointer(&pointer, cx);
                }
            }
            Event::Focus(FocusEvent::Lost) => {
                if self.selecting != Selecting::None {
                    self.selecting = Selecting::None;
                    cx.window_state.request_paint(self.id);
                }
            }
            Event::Key(key) if cx.phase == Phase::Target && is_copy(key) => {
                if let Some(text) = self.selected_text() {
                    copy_to_clipboard(text);
                    return EventPropagation::Stop;
                }
            }
            _ => {}
        }
        EventPropagation::Continue
    }
}

/// One rule per run of glyphs a strike crosses, at that run's strikeout
/// position and thickness, so a code span keeps its own rule and a line
/// that wraps gets one rule per visual line. A break's own cluster and
/// the whitespace hanging off a line's end carry no rule.
fn rules(layout: &TextLayout, len: usize, strikes: &[Strike]) -> Vec<(Rect, Color)> {
    let mut out = Vec::new();
    for line in layout.parley_layout().lines() {
        let baseline = line.metrics().baseline;
        // Runs sit one after another along the line from its alignment
        // offset; nothing here lays out an inline box between them.
        let mut next = line.metrics().offset;
        for run in line.runs() {
            let start = next;
            next += run.advance();
            let covered = run.text_range();
            let metrics = run.metrics();
            let top = baseline - metrics.strikethrough_offset;
            let thickness = metrics.strikethrough_size.max(1.0) as f64;
            for (range, color) in strikes {
                let lo = range.start.max(covered.start);
                let hi = range.end.min(covered.end).min(len);
                if lo >= hi {
                    continue;
                }
                // The clusters of this run inside the strike, by their
                // drawn positions.
                let mut x = start;
                let mut extent: Option<(f32, f32)> = None;
                for cluster in run.visual_clusters() {
                    let advance = cluster.advance();
                    let text = cluster.text_range();
                    let inside = text.start < hi && text.end > lo;
                    let hanging = cluster.is_hard_line_break()
                        || (cluster.is_space_or_nbsp() && cluster.is_end_of_line());
                    if inside && !hanging && advance > 0.0 {
                        extent = Some(match extent {
                            Some((x0, x1)) => (x0.min(x), x1.max(x + advance)),
                            None => (x, x + advance),
                        });
                    }
                    x += advance;
                }
                if let Some((x0, x1)) = extent {
                    out.push((
                        Rect::new(x0 as f64, top as f64, x1 as f64, top as f64 + thickness),
                        *color,
                    ));
                }
            }
        }
    }
    out
}

/// A point on each link of the linked text `id`, left to right along its
/// first line, found as a reader finds one: where the pointer turns into
/// a hand. The pointer is left off the text.
#[cfg(test)]
pub(crate) fn links_on_first_line(
    harness: &mut floem::headless::HeadlessHarness,
    id: ViewId,
) -> Vec<Point> {
    use floem::style::Cursor as CursorProp;
    let run = id.get_visual_rect();
    let y = run.y0 + 8.0;
    let mut out = Vec::new();
    let mut on = false;
    let mut x = run.x0 + 1.0;
    while x < run.x1 {
        harness.pointer_move(x, y);
        harness.recompute_styles();
        let hand = harness.get_computed_style(id).get(CursorProp) == Some(CursorStyle::Pointer);
        if hand && !on {
            out.push(Point::new(x + 3.0, y));
        }
        on = hand;
        x += 2.0;
    }
    harness.pointer_move(run.x1 + 10.0, run.y1 + 10.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::text::{Attrs, FamilyOwned, LineHeightValue};

    fn links(text: &str, needle: &str, url: &str) -> Vec<Link> {
        let start = text.find(needle).unwrap();
        vec![Link {
            range: start..start + needle.len(),
            target: Target::Url(url.into()),
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

    const BLACK: Color = Color::from_rgb8(0, 0, 0);
    const RED: Color = Color::from_rgb8(0xff, 0, 0);

    fn family(stack: &str) -> Vec<FamilyOwned> {
        FamilyOwned::parse_list(stack).collect()
    }

    fn prose(size: f32) -> AttrsList {
        let family = family(crate::theme::FONT_STACK);
        AttrsList::new(
            Attrs::new()
                .font_size(size)
                .weight(crate::theme::weight_body())
                .family(&family)
                .line_height(LineHeightValue::Normal(crate::theme::PROSE_LINE_HEIGHT)),
        )
    }

    fn one(range: Range<usize>) -> Vec<Strike> {
        vec![(range, BLACK)]
    }

    fn strike_lines(
        text: &str,
        attrs: &AttrsList,
        width: f32,
        strikes: &[Strike],
    ) -> Vec<(Rect, Color)> {
        rules(&layout_at(text, attrs.clone(), width), text.len(), strikes)
    }

    fn natural_width(text: &str) -> f32 {
        layout_at(text, prose(15.0), 10_000.0).size().width as f32
    }

    #[test]
    fn a_strike_runs_along_its_span_and_sits_inside_the_line() {
        let text = "keep gone keep";
        let lines = strike_lines(text, &prose(15.0), 1000.0, &one(5..9));
        assert_eq!(lines.len(), 1, "{lines:?}");
        let (line, color) = lines[0];
        assert_eq!(color, BLACK);
        // Starts after "keep " and ends before " keep": the words either
        // side are the same width, so the rule is centred in the run.
        let width = natural_width(text) as f64;
        assert!(line.x0 > 0.0 && line.x1 < width, "{line:?} in {width}");
        assert!(
            (line.x0 - (width - line.x1)).abs() < 1.0,
            "{line:?} in {width}"
        );
        // Somewhere in the line's 21px, not on its edge.
        assert!(line.y0 > 0.0 && line.y1 < 21.0, "{line:?}");
        assert!(line.height() >= 1.0, "{line:?}");
    }

    #[test]
    fn a_strike_that_wraps_is_one_rule_per_visual_line() {
        let text = "one two three four five six seven eight nine ten";
        let all = 0..text.len();
        let wide = strike_lines(text, &prose(15.0), 10_000.0, &one(all.clone()));
        assert_eq!(wide.len(), 1, "{wide:?}");
        let narrow = strike_lines(text, &prose(15.0), 120.0, &one(all));
        assert!(narrow.len() > 1, "{narrow:?}");
        // Each rule sits lower than the one before, and none hangs into
        // the margin past the words.
        for pair in narrow.windows(2) {
            assert!(pair[1].0.y0 > pair[0].0.y0, "{narrow:?}");
        }
        for (rule, _) in &narrow {
            assert!(rule.x1 <= 120.0, "{narrow:?}");
        }
    }

    #[test]
    fn a_strike_across_a_line_break_stops_at_the_last_letter() {
        // The break is a character of its own; the rule must not run on
        // past "alpha" to where the newline would be.
        let alone = strike_lines("alpha", &prose(15.0), 1000.0, &one(0..5));
        let broken = strike_lines("alpha\nbeta", &prose(15.0), 1000.0, &one(0..10));
        assert_eq!(broken.len(), 2, "{broken:?}");
        assert!(
            (broken[0].0.x1 - alone[0].0.x1).abs() < 0.01,
            "{broken:?} vs {alone:?}"
        );
    }

    #[test]
    fn a_code_span_in_a_struck_run_keeps_its_own_rule_and_colour() {
        let text = "before code after";
        let mut list = prose(15.0);
        let code = family(crate::theme::CODE_FONT_STACK);
        list.add_span(7..11, Attrs::new().font_size(14.0).family(&code).color(RED));
        let strikes = vec![(0..7, BLACK), (7..11, RED), (11..17, BLACK)];
        let lines = strike_lines(text, &list, 1000.0, &strikes);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[1].1, RED);
        // The three rules abut along the line, and the code font's sits
        // at its own height.
        assert!((lines[0].0.x1 - lines[1].0.x0).abs() < 0.01, "{lines:?}");
        assert!((lines[1].0.x1 - lines[2].0.x0).abs() < 0.01, "{lines:?}");
        assert_eq!(lines[0].0.y0, lines[2].0.y0, "{lines:?}");
        assert_ne!(lines[0].0.y0, lines[1].0.y0, "{lines:?}");
    }

    #[test]
    fn a_word_too_wide_for_the_column_is_laid_out_at_its_own_width() {
        // As the rich text view does: the text node grows to the word,
        // so the rules follow the lines the word forces, not the column.
        let word = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let text = format!("{word} bbbbbbbb cccccccc");
        let all = 0..text.len();
        let column = strike_lines(&text, &prose(15.0), 120.0, &one(all.clone()));
        let node = strike_lines(&text, &prose(15.0), natural_width(word) + 1.0, &one(all));
        assert!(column.len() > node.len(), "{column:?} vs {node:?}");
        assert_eq!(node.len(), 2, "{node:?}");
    }

    #[test]
    fn an_empty_strike_draws_nothing_and_one_past_the_end_stops_at_it() {
        let text = "short";
        assert!(strike_lines(text, &prose(15.0), 1000.0, &one(2..2)).is_empty());
        let over = strike_lines(text, &prose(15.0), 1000.0, &one(0..99));
        let whole = strike_lines(text, &prose(15.0), 1000.0, &one(0..5));
        assert_eq!(over, whole);
    }

    mod selecting {
        use super::*;
        use crate::ui::widgets::{copied, opened};
        use crate::ui::OS_MOD;
        use floem::headless::{HeadlessHarness, TestRoot};
        use floem::style::{Cursor as CursorProp, CursorStyle};
        use floem::ui_events::keyboard::{Key, KeyState, KeyboardEvent};
        use floem::ui_events::pointer::{PointerId, PointerInfo, PointerState, PointerType};
        use floem::views::Container;

        const WIDTH: f64 = 1000.0;
        const TEXT: &str = "The quick brown fox jumps over the lazy dog.";

        /// `TEXT` on one line in a column `WIDTH` wide, at the window's
        /// top left, with `links` in it, and the view that carries it.
        /// The column is a button when `selectable` is off.
        fn harness(
            links: Vec<(Range<usize>, Target)>,
            selectable: bool,
        ) -> (HeadlessHarness, ViewId) {
            let root = TestRoot::new();
            let view = Container::new(linked_text(TEXT.into(), prose(15.0), links, Vec::new()))
                .style(move |s| s.width(WIDTH).padding(0.0).selectable(selectable));
            let mut harness = HeadlessHarness::new_with_size(root, view, WIDTH, 400.0);
            harness.rebuild();
            harness.paint();
            let id = find(harness.root_id(), "LinkedText").expect("no linked text");
            assert_eq!(id.get_visual_rect().origin(), Point::ZERO);
            copied();
            opened();
            (harness, id)
        }

        fn find(id: ViewId, name: &str) -> Option<ViewId> {
            if id.debug_name() == name {
                return Some(id);
            }
            id.children()
                .into_iter()
                .find_map(|child| find(child, name))
        }

        /// Where the glyphs of `needle` sit, as the shadow lays them out
        /// and so as the window shows them.
        fn glyphs(needle: &str) -> Rect {
            let start = TEXT.find(needle).unwrap();
            let layout = layout_at(TEXT, prose(15.0), WIDTH as f32);
            let link = Link {
                range: start..start + needle.len(),
                target: Target::Url(String::new()),
            };
            let rects = line_rects(&layout, &link);
            assert_eq!(rects.len(), 1, "{needle:?} on one line: {rects:?}");
            rects[0]
        }

        /// The primary button going down or up at a point, for the
        /// `count`th click in a row.
        fn button(harness: &mut HeadlessHarness, down: bool, x: f64, y: f64, count: u8) {
            let mut state = PointerState {
                count,
                ..PointerState::default()
            };
            state.position.x = x;
            state.position.y = y;
            let event = PointerButtonEvent {
                state,
                button: Some(PointerButton::Primary),
                pointer: PointerInfo {
                    pointer_id: Some(PointerId::PRIMARY),
                    persistent_device_id: None,
                    pointer_type: PointerType::Mouse,
                },
            };
            harness.dispatch_event(Event::Pointer(if down {
                PointerEvent::Down(event)
            } else {
                PointerEvent::Up(event)
            }));
        }

        /// A drag with the primary button from one point to another: down,
        /// a first move past the slop, the move to the end, and up.
        fn drag(harness: &mut HeadlessHarness, from: Point, to: Point) {
            harness.pointer_down(from.x, from.y);
            harness.pointer_move(from.x + DRAG_SLOP + 1.0, from.y);
            harness.pointer_move(to.x, to.y);
            harness.pointer_up(to.x, to.y);
        }

        /// Cmd-C pressed, and what it copied.
        fn copy(harness: &mut HeadlessHarness) -> Vec<String> {
            harness.dispatch_event(Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Character("c".into()),
                code: Code::KeyC,
                location: Default::default(),
                modifiers: OS_MOD,
                repeat: false,
                is_composing: false,
            }));
            copied()
        }

        fn cursor(harness: &mut HeadlessHarness, id: ViewId, at: Point) -> Option<CursorStyle> {
            harness.pointer_move(at.x, at.y);
            harness.recompute_styles();
            harness.get_computed_style(id).get(CursorProp)
        }

        #[test]
        fn a_drag_selects_between_its_ends_and_cmd_c_copies_it() {
            let (mut h, id) = harness(Vec::new(), true);
            let (quick, fox) = (glyphs("quick"), glyphs("fox"));
            let y = quick.center().y;
            drag(
                &mut h,
                Point::new(quick.x0 + 2.0, y),
                Point::new(fox.x1 - 2.0, y),
            );
            assert!(h.is_focused(id), "the drag took the focus");
            assert_eq!(copy(&mut h), vec!["quick brown fox".to_string()]);
            // The selection stays for a second copy.
            assert_eq!(copy(&mut h), vec!["quick brown fox".to_string()]);
            // Backwards is the same words.
            drag(
                &mut h,
                Point::new(fox.x1 - 2.0, y),
                Point::new(quick.x0 + 2.0, y),
            );
            assert_eq!(copy(&mut h), vec!["quick brown fox".to_string()]);
        }

        #[test]
        fn a_click_selects_nothing_and_clears_what_was_selected() {
            let (mut h, id) = harness(Vec::new(), true);
            let (quick, fox) = (glyphs("quick"), glyphs("fox"));
            let y = quick.center().y;
            h.click(quick.center().x, y);
            assert!(h.is_focused(id));
            assert_eq!(copy(&mut h), Vec::<String>::new());
            drag(
                &mut h,
                Point::new(quick.x0 + 2.0, y),
                Point::new(fox.x1 - 2.0, y),
            );
            assert_eq!(copy(&mut h), vec!["quick brown fox".to_string()]);
            h.click(fox.center().x, y);
            assert_eq!(copy(&mut h), Vec::<String>::new());
        }

        /// macOS sends a pointer move between a double click's press and
        /// its release, without the pointer having moved: the sequence
        /// here is the window's, and the word stays whole through it.
        #[test]
        fn a_double_click_takes_the_word_and_a_triple_the_whole_text() {
            let (mut h, _) = harness(Vec::new(), true);
            let brown = glyphs("brown");
            let at = brown.center();
            button(&mut h, true, at.x, at.y, 2);
            h.pointer_move(at.x, at.y);
            button(&mut h, false, at.x, at.y, 2);
            assert_eq!(copy(&mut h), vec!["brown".to_string()]);
            // Dragging on from the word grows the selection by words.
            let quick = glyphs("quick");
            button(&mut h, true, at.x, at.y, 2);
            h.pointer_move(quick.center().x, at.y);
            h.pointer_up(quick.center().x, at.y);
            assert_eq!(copy(&mut h), vec!["quick brown".to_string()]);
            button(&mut h, true, at.x, at.y, 3);
            h.pointer_move(at.x, at.y);
            button(&mut h, false, at.x, at.y, 3);
            assert_eq!(copy(&mut h), vec![TEXT.to_string()]);
        }

        #[test]
        fn a_drag_that_leaves_the_text_still_reaches_its_end() {
            let (mut h, _) = harness(Vec::new(), true);
            let quick = glyphs("quick");
            let y = quick.center().y;
            // Well below and to the right of the text, inside the window.
            drag(
                &mut h,
                Point::new(quick.x0 + 2.0, y),
                Point::new(WIDTH - 1.0, y + 300.0),
            );
            assert_eq!(
                copy(&mut h),
                vec!["quick brown fox jumps over the lazy dog.".to_string()]
            );
            let dog = glyphs("dog.");
            drag(
                &mut h,
                Point::new(dog.x1 - 2.0, y),
                Point::new(0.0, y - 100.0),
            );
            assert_eq!(copy(&mut h), vec![TEXT.to_string()]);
        }

        #[test]
        fn a_press_on_a_link_is_the_links_and_selects_nothing() {
            let start = TEXT.find("fox").unwrap();
            let link = (start..start + 3, Target::Url("https://example.com/fox".into()));
            let (mut h, id) = harness(vec![link], true);
            let (fox, lazy) = (glyphs("fox"), glyphs("lazy"));
            let y = fox.center().y;
            drag(&mut h, fox.center(), lazy.center());
            assert_eq!(
                opened(),
                Vec::<String>::new(),
                "a drag off the link opened it"
            );
            assert_eq!(copy(&mut h), Vec::<String>::new());
            h.click(fox.center().x, y);
            assert_eq!(opened(), vec!["https://example.com/fox".to_string()]);
            // And a drag from beside it into it is a selection.
            let quick = glyphs("quick");
            drag(
                &mut h,
                Point::new(quick.x0 + 2.0, y),
                Point::new(fox.x1 - 2.0, y),
            );
            assert_eq!(copy(&mut h), vec!["quick brown fox".to_string()]);
            assert_eq!(opened(), Vec::<String>::new());
            assert_eq!(cursor(&mut h, id, quick.center()), Some(CursorStyle::Text));
            assert_eq!(cursor(&mut h, id, fox.center()), Some(CursorStyle::Pointer));
        }

        #[test]
        fn a_click_on_a_local_link_goes_there_and_not_to_the_browser() {
            let start = TEXT.find("fox").unwrap();
            let went = Rc::new(std::cell::Cell::new(0));
            let counted = went.clone();
            let target = Target::Local {
                tip: "src/fox.rs:3".into(),
                open: Rc::new(move || counted.set(counted.get() + 1)),
            };
            let (mut h, id) = harness(vec![(start..start + 3, target)], true);
            let (fox, lazy) = (glyphs("fox"), glyphs("lazy"));
            assert_eq!(cursor(&mut h, id, fox.center()), Some(CursorStyle::Pointer));
            h.click(fox.center().x, fox.center().y);
            assert_eq!(went.get(), 1);
            assert_eq!(opened(), Vec::<String>::new());
            // A drag off it goes nowhere, as off any link.
            drag(&mut h, fox.center(), lazy.center());
            assert_eq!(went.get(), 1);
        }

        #[test]
        fn the_selection_goes_with_the_focus() {
            let (mut h, id) = harness(Vec::new(), true);
            let (quick, fox) = (glyphs("quick"), glyphs("fox"));
            let y = quick.center().y;
            drag(
                &mut h,
                Point::new(quick.x0 + 2.0, y),
                Point::new(fox.x1 - 2.0, y),
            );
            assert_eq!(copy(&mut h), vec!["quick brown fox".to_string()]);
            h.root_id().request_focus();
            h.rebuild();
            assert!(!h.is_focused(id));
            id.request_focus();
            h.rebuild();
            assert!(h.is_focused(id));
            assert_eq!(copy(&mut h), Vec::<String>::new());
        }

        #[test]
        fn text_that_is_not_selectable_is_left_alone() {
            let (mut h, id) = harness(Vec::new(), false);
            let (quick, fox) = (glyphs("quick"), glyphs("fox"));
            let y = quick.center().y;
            assert_eq!(cursor(&mut h, id, quick.center()), None);
            drag(
                &mut h,
                Point::new(quick.x0 + 2.0, y),
                Point::new(fox.x1 - 2.0, y),
            );
            assert_eq!(copy(&mut h), Vec::<String>::new());
        }
    }
}
