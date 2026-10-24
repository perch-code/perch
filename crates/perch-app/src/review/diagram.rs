//! A Mermaid diagram drawn as a figure: mermaid-svg lays it out and
//! writes it as an SVG in Perch's colours, and it is drawn as a picture
//! is. It is laid out again when the appearance changes, since its
//! colours are in the SVG.
//!
//! What vger draws of an SVG holds here too: a dashed line is drawn
//! solid, and rotated text unrotated.
//!
//! One wider than the column is shrunk to fit it, labels and all, and a
//! click opens it at its own size over the window. The renderer draws a
//! picture whole and no bigger than [`picture::MAX_SIDE`] on a side, so
//! an opened diagram is cut into tiles it can each draw.

use std::cell::Cell;

use floem::event::EventPropagation;
use floem::kurbo::{Rect, Size};
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, ScrollExt, Stack};
use floem::ViewId;

use crate::icons::SidebarIcon;
use crate::review::cards;
use crate::review::picture::{self, Drawing};
use crate::theme;
use crate::ui::widgets::{clickable, icon_button};

/// `source` laid out and drawn in the appearance's colours, or why not.
#[cfg(test)]
pub fn render(source: &str) -> Result<Drawing, String> {
    Drawing::parse(&markup(source)?)
}

/// `source` laid out as an SVG in the appearance's colours. mermaid-svg
/// can draw past the frame it gives itself, a sequence diagram's note to
/// self say, so the frame is widened to what it draws.
fn markup(source: &str) -> Result<String, String> {
    let svg = mermaid_svg::render_with(source, &perch_theme()).map_err(|e| e.to_string())?;
    let drawing = Drawing::parse(&svg)?;
    Ok(fit(&svg, drawing.size(), drawing.bounds()).unwrap_or(svg))
}

/// Room left around what is drawn past the frame.
const FIT_MARGIN: f64 = 12.0;

/// `svg` with its root's `viewBox` widened to take in `drawn`, where that
/// runs past the `size` it declares; `None` when it doesn't, or the root
/// has no `viewBox` to widen.
fn fit(svg: &str, size: Size, drawn: Rect) -> Option<String> {
    let frame = size.to_rect();
    if drawn.is_zero_area()
        || (drawn.x0 >= frame.x0
            && drawn.y0 >= frame.y0
            && drawn.x1 <= frame.x1
            && drawn.y1 <= frame.y1)
    {
        return None;
    }
    let grow = |inside: bool, by: f64| if inside { 0.0 } else { by };
    let x0 = drawn.x0.min(frame.x0) - grow(drawn.x0 >= frame.x0, FIT_MARGIN);
    let y0 = drawn.y0.min(frame.y0) - grow(drawn.y0 >= frame.y0, FIT_MARGIN);
    let x1 = drawn.x1.max(frame.x1) + grow(drawn.x1 <= frame.x1, FIT_MARGIN);
    let y1 = drawn.y1.max(frame.y1) + grow(drawn.y1 <= frame.y1, FIT_MARGIN);
    with_view_box(svg, Rect::new(x0, y0, x1, y1))
}

/// Where the root's `viewBox` value sits in `svg`, and what it says.
fn view_box(svg: &str) -> Option<(std::ops::Range<usize>, Rect)> {
    let root_end = svg.find('>')?;
    let start = svg[..root_end].find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + svg[start..root_end].find('"')?;
    let numbers: Vec<f64> = svg[start..end]
        .split([' ', ','])
        .filter(|n| !n.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [x, y, width, height] = numbers[..] else {
        return None;
    };
    Some((start..end, Rect::new(x, y, x + width, y + height)))
}

/// `svg` with its root's `viewBox` set to `frame`.
fn with_view_box(svg: &str, frame: Rect) -> Option<String> {
    let (at, _) = view_box(svg)?;
    Some(format!(
        "{}{} {} {} {}{}",
        &svg[..at.start],
        frame.x0,
        frame.y0,
        frame.width(),
        frame.height(),
        &svg[at.end..]
    ))
}

/// The most an opened diagram is drawn at on a side, three tiles; one
/// bigger is shrunk to it.
const OPENED_MAX: f64 = 3.0 * picture::MAX_SIDE;

/// `svg` cut into tiles the renderer can each draw whole: the size of
/// the whole on screen, and each tile's place in it and its part of the
/// drawing. All of it at its own size, or shrunk to [`OPENED_MAX`] on its
/// longer side. A tile runs a pixel into the next, which draws over it:
/// tiles that only met would leave a seam where they meet off the pixel
/// grid.
fn tiles(svg: &str) -> Option<(Size, Vec<(Rect, Drawing)>)> {
    let (_, frame) = view_box(svg)?;
    if frame.is_zero_area() {
        return None;
    }
    let scale = (OPENED_MAX / frame.width())
        .min(OPENED_MAX / frame.height())
        .min(1.0);
    // Where each tile starts and where the next does, in whole pixels.
    let cuts = |length: f64| {
        let length = (length * scale).round().max(1.0);
        let count = (length / (picture::MAX_SIDE - 1.0)).ceil().max(1.0);
        (0..=count as usize)
            .map(|i| (length * i as f64 / count).round())
            .collect::<Vec<f64>>()
    };
    let (columns, rows) = (cuts(frame.width()), cuts(frame.height()));
    let whole = Size::new(*columns.last()?, *rows.last()?);
    let mut tiles = Vec::new();
    for row in rows.windows(2) {
        for column in columns.windows(2) {
            let shown = Rect::new(
                column[0],
                row[0],
                (column[1] + 1.0).min(whole.width),
                (row[1] + 1.0).min(whole.height),
            );
            let part = Rect::new(
                frame.x0 + shown.x0 / scale,
                frame.y0 + shown.y0 / scale,
                frame.x0 + shown.x1 / scale,
                frame.y0 + shown.y1 / scale,
            );
            tiles.push((shown, Drawing::parse(&with_view_box(svg, part)?).ok()?));
        }
    }
    Some((whole, tiles))
}

/// The diagram as a figure, in the colours of the appearance there is.
/// One shrunk to fit opens at its own size on a click.
pub fn view(source: String) -> impl IntoView {
    dyn_container(theme::appearance, move |_| {
        let laid_out = markup(&source).and_then(|svg| Ok((Drawing::parse(&svg)?, svg)));
        match laid_out {
            Ok((drawing, svg)) => figure(drawing, svg).into_any(),
            Err(why) => Label::derived(move || format!("The diagram couldn't be drawn: {why}"))
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .text_wrap()
                        .width_full()
                })
                .into_any(),
        }
    })
    .debug_name("walkthrough-diagram")
    .style(|s| s.width_full())
}

/// The figure, which opens the diagram when the column has shrunk it.
fn figure(drawing: Drawing, svg: String) -> impl IntoView {
    let natural = drawing.size().width;
    let width = RwSignal::new(0.0_f64);
    let shrunk = Memo::new(move |_| width.get() > 0.0 && natural > width.get() + 0.5);
    Container::new(picture::figure(drawing))
        .on_event_stop(el::Click, move |_, _| {
            if shrunk.get_untracked() {
                open(&svg);
            }
        })
        .on_event_cont(
            floem::context::LayoutChanged::listener(),
            move |_, changed: &floem::context::LayoutChanged| {
                let size = changed.new_content_box.size().width;
                if width.get_untracked() != size {
                    width.set(size);
                }
            },
        )
        .debug_name("diagram-figure")
        .style(move |s| s.width_full().apply_if(shrunk.get(), clickable))
}

thread_local! {
    /// The opened diagram's overlay, while one is up.
    static OPENED: Cell<Option<ViewId>> = const { Cell::new(None) };
}

/// Whether a diagram is open.
#[cfg(test)]
pub fn is_open() -> bool {
    OPENED.with(|opened| {
        let id = opened.take();
        opened.set(id);
        id.is_some()
    })
}

/// Open `svg` at its own size over the window.
fn open(svg: &str) {
    close();
    let Some((whole, tiles)) = tiles(svg) else {
        return;
    };
    let overlay = floem::action::add_overlay(opened(whole, tiles));
    OPENED.with(|opened| opened.set(Some(overlay)));
}

/// Put the opened diagram away.
pub fn close() {
    if let Some(overlay) = OPENED.with(Cell::take) {
        overlay.clear_focus();
        floem::action::remove_overlay(overlay);
    }
}

/// The opened diagram: its tiles in a panel over a scrim, scrolling when
/// the window is smaller than it, and as modal as a popover: Escape, the
/// close button or a press outside puts it away, and the wheel scrolls
/// nothing under it.
fn opened(whole: Size, tiles: Vec<(Rect, Drawing)>) -> impl View {
    let grid = Stack::vertical_from_iter(tiles.into_iter().map(|(place, drawing)| {
        Container::new(picture::plain(drawing, place.size()))
            .style(move |s| s.absolute().inset_left(place.x0).inset_top(place.y0))
    }))
    .style(move |s| {
        s.width(whole.width)
            .height(whole.height)
            .flex_shrink(0.0_f32)
    });
    let close_button = cards::tip(
        icon_button(|| SidebarIcon::Close, theme::fg_muted, close).debug_name("diagram-close"),
        "Close".to_string(),
    );
    let panel = Stack::vertical((
        Stack::horizontal((Empty::new().style(|s| s.flex_grow(1.0_f32)), close_button))
            .style(|s| s.width_full().items_center()),
        grid.scroll().style(|s| {
            s.max_width_full()
                .max_height_full()
                .min_width(0.0)
                .min_height(0.0)
        }),
    ))
    .on_event_stop(el::PointerDown, |_, _| {})
    .debug_name("diagram-opened")
    .style(|s| {
        cards::perch_surface(s)
            .flex_col()
            .gap(4.0)
            .padding(12.0)
            .padding_top(8.0)
            .max_width_full()
            .max_height_full()
            .min_width(0.0)
            .min_height(0.0)
            .box_shadow_blur(32.0)
            .box_shadow_v_offset(16.0)
            .box_shadow_spread(-8.0)
            .box_shadow_color(theme::panel_shadow())
    });
    let asked = Cell::new(false);
    let root = Container::new(panel);
    let root_id = root.id();
    root.on_event_stop(el::PointerDown, |_, _| close())
        .on_event_stop(el::PointerWheel, |_, _| {})
        .on_event(el::KeyDown, |_, key: &KeyboardEvent| {
            if key.code == Code::Escape && key.modifiers.is_empty() {
                close();
            }
            // Nothing pressed here reaches the window's keys.
            EventPropagation::Stop
        })
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if !asked.replace(true) {
                root_id.request_focus();
            }
        })
        .debug_name("diagram-overlay")
        .style(|s| {
            s.inset(0.0)
                .padding(40.0)
                .items_center()
                .justify_center()
                .background(theme::scrim())
                .focusable()
        })
}

/// Mermaid's quiet theme for the appearance, in Perch's colours: the
/// card's grey under it, boxes a step darker, lines in the muted ink.
fn perch_theme() -> mermaid_svg::Theme {
    let dark = theme::appearance() == theme::Appearance::Dark;
    let mut t = if dark {
        mermaid_svg::Theme::dark()
    } else {
        mermaid_svg::Theme::neutral()
    };
    let fg = hex(theme::fg());
    let muted = hex(theme::fg_muted());
    let subtle = hex(theme::fg_subtle());
    let card = hex(theme::perch_bg());
    let box_fill = hex(theme::border_muted());
    let line = hex(theme::border());
    let note = hex(theme::canvas_subtle());
    t.fg = fg.clone().into();
    t.fg_muted = muted.clone().into();
    t.bg = card.clone().into();
    t.actor_fill = box_fill.clone().into();
    t.actor_stroke = subtle.clone().into();
    t.actor_text_color = Some(fg.clone().into());
    t.lifeline = line.clone().into();
    t.arrow_stroke = muted.clone().into();
    t.signal_text_color = Some(fg.clone().into());
    t.note_fill = note.clone().into();
    t.note_stroke = subtle.clone().into();
    t.activation_fill = box_fill.clone().into();
    t.activation_stroke = subtle.clone().into();
    t.frame_label_fill = box_fill.clone().into();
    t.title_color = Some(fg.into());
    t.flow_node_fill = box_fill.into();
    t.flow_node_stroke = subtle.into();
    t.flow_edge_stroke = muted.into();
    t.flow_label_bg = card.into();
    t.flow_cluster_fill = note.into();
    t.flow_cluster_stroke = line.into();
    t
}

fn hex(color: Color) -> String {
    let [r, g, b, _] = color.to_rgba8().to_u8_array();
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A diagram draws, with its labels, in the light colours and the
    /// dark; one that isn't Mermaid says why.
    #[test]
    fn a_diagram_draws_in_either_appearance_and_a_broken_one_says_why() {
        let source = "flowchart LR\n  A[Flush] --> B[(Manifest)]";
        for appearance in [theme::Appearance::Light, theme::Appearance::Dark] {
            theme::set_appearance(appearance);
            let svg = mermaid_svg::render_with(source, &perch_theme()).unwrap();
            assert!(svg.contains(&hex(theme::fg())), "{appearance:?}: {svg}");
            assert!(svg.contains("Manifest"));
            let drawing = render(source).unwrap();
            assert!(drawing.size().width > 0.0);
        }
        theme::set_appearance(theme::Appearance::Light);
        assert!(render("flowchart LR\n  A[open --> ").is_err());
    }

    /// A note to self that runs past the frame mermaid-svg gives the
    /// diagram is taken in: the frame grows to it, and only on the side
    /// it runs over.
    #[test]
    fn a_diagram_drawn_past_its_frame_is_framed_again() {
        let source = "sequenceDiagram\n  participant F as Flusher\n  participant M as Manifest\n  \
                      M->>M: decode, the second sequence number is wrong and huge";
        let svg = mermaid_svg::render_with(source, &perch_theme()).unwrap();
        let first = Drawing::parse(&svg).unwrap();
        assert!(
            first.bounds().x1 > first.size().width,
            "mermaid-svg still draws past its frame: {:?} {:?}",
            first.bounds(),
            first.size()
        );
        let drawing = render(source).unwrap();
        let (size, bounds) = (drawing.size(), drawing.bounds());
        assert!(
            bounds.x1 <= size.width && bounds.y1 <= size.height,
            "{bounds:?} {size:?}"
        );
        assert!(
            size.height < first.size().height + 1.0,
            "only the right side grew"
        );
    }

    const CHAIN: &str = "flowchart LR\n  A[SlateDB request] --> B[Retry wrapper] --> \
                         C[Metrics wrapper] --> D[Supplied object store] --> E[Remote storage]";

    /// An opened diagram is cut into tiles no bigger than the renderer
    /// draws, which meet edge to edge and together are the whole at its
    /// own size; one past three tiles a side is shrunk to three.
    #[test]
    fn an_opened_diagram_is_cut_into_tiles_the_renderer_can_draw() {
        let svg = markup(CHAIN).unwrap();
        let natural = Drawing::parse(&svg).unwrap().size();
        assert!(natural.width > picture::MAX_SIDE, "{natural:?}");
        let (whole, cut) = tiles(&svg).unwrap();
        assert_eq!(
            whole,
            Size::new(natural.width.round(), natural.height.round())
        );
        assert_eq!(cut.len(), 2);
        let (left, right) = (cut[0].0, cut[1].0);
        assert_eq!((left.x0, right.x1), (0.0, whole.width));
        assert_eq!(left.x1, right.x0 + 1.0, "a pixel over the next");
        for (shown, drawing) in &cut {
            assert!(shown.width() <= picture::MAX_SIDE && shown.height() <= picture::MAX_SIDE);
            let part = drawing.size();
            assert!((part.width / part.height - shown.width() / shown.height()).abs() < 0.01);
        }

        let huge = r#"<svg width="100%" viewBox="-10 0 5000 100"></svg>"#;
        let (whole, cut) = tiles(huge).unwrap();
        assert_eq!(cut.len(), 4);
        assert_eq!(whole, Size::new(OPENED_MAX, 61.0));
        assert!(tiles("<svg></svg>").is_none());
    }

    /// A diagram the column shrinks opens over the window on a click,
    /// holding the keyboard, and Escape or a press outside puts it away.
    /// One drawn at its own size opens nothing.
    #[test]
    fn a_shrunk_diagram_opens_full_size_and_goes_away() {
        use floem::headless::{HeadlessHarness, TestRoot};

        let named = |id: ViewId, name: &str| {
            fn walk(id: ViewId, name: &str) -> Option<ViewId> {
                if id.debug_name().split_whitespace().any(|part| part == name) {
                    return Some(id);
                }
                id.children()
                    .into_iter()
                    .find_map(|child| walk(child, name))
            }
            walk(id, name)
        };
        let settle = |harness: &mut HeadlessHarness| {
            harness.rebuild();
            harness.paint();
        };
        let click_on = |harness: &mut HeadlessHarness, id: ViewId| {
            let center = id.get_visual_rect_no_clip().center();
            harness.click(center.x, center.y);
            harness.rebuild();
            harness.paint();
        };
        let escape = || {
            floem::event::Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Named(NamedKey::Escape),
                code: Code::Escape,
                location: Default::default(),
                modifiers: Modifiers::empty(),
                repeat: false,
                is_composing: false,
            })
        };

        let root = TestRoot::new();
        let page = Stack::vertical((
            view("flowchart LR\n  A[Flush] --> B[(Manifest)]".into()),
            view(CHAIN.into()),
        ))
        .style(|s| s.flex_col().width(800.0));
        let mut harness = HeadlessHarness::new_with_size(root, page, 1200.0, 800.0);
        settle(&mut harness);
        let figures: Vec<ViewId> = harness
            .root_id()
            .children()
            .into_iter()
            .flat_map(|page| page.children())
            .filter_map(|diagram| named(diagram, "diagram-figure"))
            .collect();
        assert_eq!(figures.len(), 2);

        click_on(&mut harness, figures[0]);
        assert!(!is_open(), "drawn at its own size: nothing to open");

        click_on(&mut harness, figures[1]);
        assert!(is_open());
        let overlay = named(harness.root_id(), "diagram-overlay").expect("over the window");
        assert!(harness.is_focused(overlay), "it takes the keyboard");
        let panel = named(overlay, "diagram-opened").unwrap();
        assert!(panel.get_visual_rect_no_clip().width() > 800.0);
        harness.dispatch_event(escape());
        settle(&mut harness);
        assert!(!is_open());
        assert!(named(harness.root_id(), "diagram-overlay").is_none());

        click_on(&mut harness, figures[1]);
        assert!(is_open());
        harness.click(5.0, 795.0);
        settle(&mut harness);
        assert!(!is_open(), "a press outside puts it away");
    }

    #[test]
    fn a_diagram_inside_its_frame_is_left_alone() {
        let svg = r#"<svg width="100%" viewBox="0 0 100 50"></svg>"#;
        assert_eq!(
            fit(
                svg,
                Size::new(100.0, 50.0),
                Rect::new(10.0, 10.0, 90.0, 40.0)
            ),
            None
        );
        assert_eq!(
            fit(
                svg,
                Size::new(100.0, 50.0),
                Rect::new(10.0, 10.0, 130.0, 40.0)
            )
            .as_deref(),
            Some(r#"<svg width="100%" viewBox="0 0 142 50"></svg>"#)
        );
    }

    #[test]
    fn a_colour_is_written_as_hex() {
        assert_eq!(hex(Color::from_rgb8(0x25, 0x25, 0x23)), "#252523");
    }
}
