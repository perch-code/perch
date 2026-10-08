//! A diagram Changescape laid out, drawn as the SVG it shipped. Perch
//! adds no layout and edits nothing in the tree: it draws the picture,
//! finds which `hot` node is under the pointer from the nodes' bounding
//! boxes, rings it, and opens its element on a click.
//!
//! The picture fits the block's width until the reader zooms it, and
//! then a picture wider than the block scrolls. Expand shows it over
//! the section, fitted to the width and the height there. Changescape
//! colours it for a light page, so it sits on a light card in a dark
//! theme too.

use std::borrow::Cow;
use std::sync::Arc;

use floem::context::{LayoutChanged, PaintCx};
use floem::kurbo::{Point, Rect, Size, Stroke};
use floem::peniko::{Brush, Color};
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::ui_events::pointer::PointerButtonEvent;
use floem::views::{Container, Decorators, Label, Stack};
use floem::{AnyView, RendererSvg, View, ViewId};
use perch_changescape::page::Svg;

use crate::review::changescape::blocks::Ctx;
use crate::review::model::ReviewState;
use crate::review::picture::{assumed_scale, note_scale, Drawing, MAX_RASTER};
use crate::theme;
use crate::ui::widgets::clickable;

/// The card under every diagram, whatever the theme.
const CARD: Color = Color::WHITE;
/// How much of a node outside the facet option the card covers.
const DIM_COVER: f32 = 0.7;
/// How much each zoom step scales by.
const ZOOM_STEP: f64 = 1.25;
const ZOOM_MIN: f64 = 0.25;
const ZOOM_MAX: f64 = 4.0;
/// The most a picture is scaled up to fit its block when it opens.
const FIT_MAX: f64 = 1.5;
/// The most a picture of `natural` size zooms to at `scale`, so that its
/// raster stays within `MAX_RASTER`.
fn zoom_cap(natural: Size, scale: f64) -> f64 {
    let longest = natural.width.max(natural.height) * scale;
    if longest > 0.0 {
        ZOOM_MAX.min(MAX_RASTER / longest)
    } else {
        ZOOM_MAX
    }
}

/// The zoom a picture draws at: `zoom`, or one until the room is known,
/// never past the cap.
fn drawn_zoom(zoom: Option<f64>, cap: f64) -> f64 {
    zoom.unwrap_or(1.0).min(cap)
}

/// `z` held to the zoom range and to the cap.
fn clamp_zoom(z: f64, cap: f64) -> f64 {
    z.clamp(ZOOM_MIN, ZOOM_MAX).min(cap)
}

/// The tallest a diagram's box gets before it scrolls down too.
const MAX_HEIGHT: f64 = 560.0;

/// Where a diagram is drawn.
#[derive(Debug, Clone)]
pub enum Frame {
    /// In its section, as block `at` (view id, index): the column's
    /// width, and at most `MAX_HEIGHT` tall.
    Block((String, usize)),
    /// Over the section, all of it.
    Expanded,
}

/// The zoom that fits a picture of `natural` size into `room`.
fn fit(frame: &Frame, natural: Size, room: Size) -> f64 {
    let across = room.width / natural.width;
    match frame {
        Frame::Block(_) => across.min(FIT_MAX),
        Frame::Expanded if natural.height > 0.0 => across.min(room.height / natural.height),
        Frame::Expanded => across,
    }
}

/// A hot node: where it is in the picture's own units, and its element.
#[derive(Debug, Clone)]
struct Hot {
    rect: Rect,
    el: String,
}

pub fn svg_block(cx: Ctx, svg: Svg, frame: Frame) -> AnyView {
    let drawing = match Drawing::parse(&svg.svg) {
        Ok(drawing) => drawing,
        Err(e) => {
            tracing::warn!(error = %e, "could not parse a changescape diagram");
            return Label::new("This diagram could not be drawn.")
                .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_subtle()))
                .into_any();
        }
    };
    let hot = Arc::new(hot_nodes(&drawing, &svg));
    let natural = drawing.size();
    // The hot nodes outside the facet option the tab applies, by index.
    let outside = {
        let (hot, state, page) = (hot.clone(), cx.state, cx.page.clone());
        Memo::new(move |_| {
            super::with_facet(state, &page, |option| match option {
                Some(option) => (0..hot.len())
                    .filter(|&i| !option.els.contains(&hot[i].el))
                    .collect(),
                None => Vec::new(),
            })
        })
    };
    // The reader's zoom; `None` fits the room, whatever its size.
    let zoom: RwSignal<Option<f64>> = RwSignal::new(None);
    // The room inside the card, from its last layout.
    let room: RwSignal<Option<Size>> = RwSignal::new(None);
    let hovered: RwSignal<Option<usize>> = RwSignal::new(None);
    let scale = RwSignal::new(assumed_scale());
    let cap = move || zoom_cap(natural, scale.get());
    // A screen or display zoom with more pixels a point lowers the cap.
    floem::reactive::Effect::new(move |_| {
        let cap = cap();
        if zoom.get_untracked().is_some_and(|z| z > cap) {
            zoom.set(Some(cap));
        }
    });
    // The zoom the picture draws at.
    let shown = {
        let frame = frame.clone();
        Memo::new(move |_| match (zoom.get(), room.get()) {
            (None, Some(room)) if natural.width > 0.0 => {
                clamp_zoom(fit(&frame, natural, room), cap())
            }
            (zoom, _) => drawn_zoom(zoom, cap()),
        })
    };
    let id = ViewId::new();
    // The ring is painted from the hovered node, which no style reads.
    floem::reactive::Effect::new(move |_| {
        hovered.track();
        shown.track();
        outside.track();
        id.request_paint();
    });
    let picture = Diagram {
        id,
        drawing,
        hot: hot.clone(),
        hovered,
        outside,
        shown,
        painted: Size::ZERO,
        scale,
    };
    let (state, expanded) = (cx.state, matches!(frame, Frame::Expanded));
    let marks = badges(&cx, &hot, shown);
    let (move_hot, down_hot, click_hot) = (hot.clone(), hot.clone(), hot);
    let pressed: RwSignal<Option<usize>> = RwSignal::new(None);
    let picture = picture
        .debug_name("Changescape diagram")
        // Points arrive in the view's own coordinates.
        .on_event_cont(el::PointerMove, move |_, update| {
            let at = update.current.logical_point();
            let under = hit(&move_hot, scaled(at, shown.get_untracked()));
            if hovered.get_untracked() != under {
                hovered.set(under);
            }
        })
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(None))
        .on_event_cont(el::PointerDown, move |_, e: &PointerButtonEvent| {
            let at = e.state.logical_point();
            pressed.set(hit(&down_hot, scaled(at, shown.get_untracked())));
        })
        .on_event_stop(el::Click, move |_, _| {
            if let Some(i) = pressed.get_untracked() {
                cx.open(&click_hot[i].el);
                cx.state.changescape_expanded.set(None);
            }
        })
        .style(move |s| {
            let z = shown.get();
            s.width((natural.width * z).floor())
                .height((natural.height * z).floor())
                .flex_shrink(0.0_f32)
                .apply_if(hovered.get().is_some(), |s| {
                    s.cursor(floem::style::CursorStyle::Pointer)
                })
        });
    let scroller = Container::new(
        Stack::vertical((picture, marks)).style(|s| s.position(floem::style::Position::Relative)),
    )
    .style(|s| s.padding(12.0).background(CARD))
    .scroll()
    .on_event_stop(
        LayoutChanged::listener(),
        move |_,
              LayoutChanged {
                  new_content_box, ..
              }| {
            let size = new_content_box.size();
            let inside = Size::new(size.width - 24.0, size.height - 24.0);
            if inside.width > 0.0 && room.get_untracked() != Some(inside) {
                room.set(Some(inside));
            }
        },
    )
    .style(move |s| {
        s.width_full()
            .apply_if(!expanded, |s| s.max_height(MAX_HEIGHT))
            .apply_if(expanded, |s| {
                s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0)
            })
            .background(CARD)
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
    });
    Stack::vertical((zoom_bar(state, zoom, shown, cap, frame), scroller))
        .style(move |s| {
            s.flex_col()
                .gap(4.0)
                .width_full()
                .apply_if(expanded, |s| s.height_full())
        })
        .into_any()
}

/// Each hot node's badge of open threads, at the node's top right corner,
/// moved with the zoom.
fn badges(cx: &Ctx, hot: &[Hot], shown: Memo<f64>) -> impl IntoView {
    let marks: Vec<AnyView> = hot
        .iter()
        .map(|h| {
            let rect = h.rect;
            Container::new(cx.badge(&h.el))
                .style(move |s| {
                    let z = shown.get();
                    s.position(floem::style::Position::Absolute)
                        .inset_left(rect.x1 * z - 14.0)
                        .inset_top(rect.y0 * z - 9.0)
                })
                .into_any()
        })
        .collect();
    Stack::vertical_from_iter(marks).style(|s| {
        s.position(floem::style::Position::Absolute)
            .inset_left(0.0)
            .inset_top(0.0)
    })
}

/// Smaller, the room's fit, larger, and Expand, or Close once expanded.
fn zoom_bar(
    state: ReviewState,
    zoom: RwSignal<Option<f64>>,
    shown: Memo<f64>,
    cap: impl Fn() -> f64 + Copy + 'static,
    frame: Frame,
) -> impl IntoView {
    let step = move |by: f64| {
        let z = shown.get_untracked() * by;
        zoom.set(Some(clamp_zoom(z, cap())));
    };
    let button = |text: &'static str, name: &'static str, act: Box<dyn Fn()>| {
        Container::new(Label::new(text).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .selectable(false)
        }))
        .debug_name(name)
        .on_event_stop(el::Click, move |_, _| act())
        .style(|s| {
            clickable(
                s.padding_horiz(8.0)
                    .height(22.0)
                    .items_center()
                    .border_radius(theme::RADIUS),
            )
            .hover(|s| s.background(theme::canvas_hover()))
        })
    };
    Stack::horizontal((
        button(
            "−",
            "Changescape zoom out",
            Box::new(move || step(1.0 / ZOOM_STEP)),
        ),
        // `None` fits the room again.
        button(
            "Fit",
            "Changescape zoom fit",
            Box::new(move || zoom.set(None)),
        ),
        button(
            "+",
            "Changescape zoom in",
            Box::new(move || step(ZOOM_STEP)),
        ),
        match frame {
            Frame::Block(at) => button(
                "Expand",
                "Changescape expand",
                Box::new(move || state.changescape_expanded.set(Some(at.clone()))),
            ),
            Frame::Expanded => button(
                "Close",
                "Changescape collapse",
                Box::new(move || state.changescape_expanded.set(None)),
            ),
        },
    ))
    .style(|s| s.gap(2.0).justify_end().width_full())
}

/// A point in the view, in the picture's own units.
fn scaled(at: Point, z: f64) -> Point {
    Point::new(at.x / z, at.y / z)
}

/// The hot nodes the tree has, with their bounding boxes.
fn hot_nodes(drawing: &Drawing, svg: &Svg) -> Vec<Hot> {
    svg.hot
        .iter()
        .filter_map(|(node, el)| {
            let node = drawing.tree.node_by_id(node)?;
            let b = node.abs_bounding_box();
            Some(Hot {
                rect: Rect::new(
                    f64::from(b.left()),
                    f64::from(b.top()),
                    f64::from(b.right()),
                    f64::from(b.bottom()),
                ),
                el: el.clone(),
            })
        })
        .collect()
}

/// The hot node under a point. Where boxes nest, a module's around its
/// functions, the smallest is the one on top.
fn hit(hot: &[Hot], at: Point) -> Option<usize> {
    hot.iter()
        .enumerate()
        .filter(|(_, h)| h.rect.contains(at))
        .min_by(|(_, a), (_, b)| a.rect.area().total_cmp(&b.rect.area()))
        .map(|(i, _)| i)
}

/// The picture at its zoom, and a ring round the hot node under the
/// pointer. A view of its own size, as `picture.rs` draws one, since
/// floem's `draw_svg` would scale a rect's origin with the picture.
struct Diagram {
    id: ViewId,
    drawing: Drawing,
    hot: Arc<Vec<Hot>>,
    hovered: RwSignal<Option<usize>>,
    /// The hot nodes a facet dims, by index.
    outside: Memo<Vec<usize>>,
    /// The zoom it draws at.
    shown: Memo<f64>,
    painted: Size,
    /// The device scale it last painted at.
    scale: RwSignal<f64>,
}

impl View for Diagram {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> Cow<'static, str> {
        "Changescape diagram".into()
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        note_scale(cx, self.scale);
        let Some(layout) = self.id.get_layout() else {
            return;
        };
        let rect = Size::new(f64::from(layout.size.width), f64::from(layout.size.height)).to_rect();
        if rect.is_zero_area() {
            return;
        }
        cx.fill(&rect, CARD, 0.0);
        cx.clip(&rect);
        cx.draw_svg(
            RendererSvg {
                tree: &self.drawing.tree,
                hash: self.drawing.hash(),
            },
            rect,
            None::<&Brush>,
        );
        let z = self.shown.get_untracked();
        // A node outside the facet option under a veil of the card, over
        // the box the pointer finds it by.
        self.outside.with_untracked(|outside| {
            for hot in outside.iter().filter_map(|&i| self.hot.get(i)) {
                let cover = Rect::new(
                    hot.rect.x0 * z,
                    hot.rect.y0 * z,
                    hot.rect.x1 * z,
                    hot.rect.y1 * z,
                );
                cx.fill(&cover, CARD.multiply_alpha(DIM_COVER), 0.0);
            }
        });
        if let Some(hot) = self.hovered.get_untracked().and_then(|i| self.hot.get(i)) {
            let ring = Rect::new(
                hot.rect.x0 * z,
                hot.rect.y0 * z,
                hot.rect.x1 * z,
                hot.rect.y1 * z,
            )
            .inflate(2.0, 2.0);
            cx.stroke(&ring, theme::accent(), &Stroke::new(2.0));
        }
        cx.clear_clip();
        if self.painted != rect.size() {
            self.painted = rect.size();
            self.id.request_paint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zoom_stops_where_the_raster_would_pass_the_most() {
        // 620 wide at 2 pixels a point: 4096 / 1240 is 3.3, under 4.
        let cap = zoom_cap(Size::new(620.0, 194.0), 2.0);
        assert!((cap - MAX_RASTER / 1240.0).abs() < 1e-9);
        assert_eq!(clamp_zoom(ZOOM_MAX, cap), cap);
        assert_eq!(clamp_zoom(1.0, cap), 1.0);
        // A small picture keeps the whole range.
        assert_eq!(zoom_cap(Size::new(100.0, 50.0), 2.0), ZOOM_MAX);
        // Before the fit, a picture wider than the cap at one draws at
        // the cap.
        let wide = zoom_cap(Size::new(3000.0, 400.0), 2.0);
        assert!(wide < 1.0);
        assert_eq!(drawn_zoom(None, wide), wide);
    }

    const TWO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><g id="outer"><rect x="0" y="0" width="200" height="100" fill="#eee"/><g id="inner"><rect x="20" y="20" width="40" height="30" fill="#ccc"/></g></g></svg>"##;

    fn svg(hot: &[(&str, &str)]) -> Svg {
        Svg {
            svg: TWO.into(),
            hot: hot
                .iter()
                .map(|(n, e)| (n.to_string(), e.to_string()))
                .collect(),
        }
    }

    #[test]
    fn the_smallest_box_under_the_point_wins() {
        let s = svg(&[("outer", "mod:m"), ("inner", "fn:f"), ("missing", "fn:x")]);
        let drawing = Drawing::parse(&s.svg).unwrap();
        let hot = hot_nodes(&drawing, &s);
        assert_eq!(hot.len(), 2, "a node the tree lacks is dropped");
        let at = |x, y| hit(&hot, Point::new(x, y)).map(|i| hot[i].el.as_str());
        assert_eq!(at(30.0, 30.0), Some("fn:f"));
        assert_eq!(at(150.0, 80.0), Some("mod:m"));
        assert_eq!(at(250.0, 80.0), None);
    }
}
