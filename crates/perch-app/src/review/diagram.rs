//! A Mermaid diagram drawn as a figure: mermaid-svg lays it out and
//! writes it as an SVG in Perch's colours, and it is drawn as a picture
//! is. It is laid out again when the appearance changes, since its
//! colours are in the SVG.
//!
//! What vger draws of an SVG holds here too: a dashed line is drawn
//! solid, and rotated text unrotated.

use floem::kurbo::{Rect, Size};
use floem::peniko::Color;
use floem::prelude::*;
use floem::views::{dyn_container, Decorators, Label};

use crate::review::picture::{self, Drawing};
use crate::theme;

/// `source` laid out and drawn in the appearance's colours, or why not.
/// mermaid-svg can draw past the frame it gives itself, a sequence
/// diagram's note to self say, so the frame is widened to what it draws.
pub fn render(source: &str) -> Result<Drawing, String> {
    let svg = mermaid_svg::render_with(source, &perch_theme()).map_err(|e| e.to_string())?;
    let drawing = Drawing::parse(&svg)?;
    match fit(&svg, drawing.size(), drawing.bounds()) {
        Some(fitted) => Drawing::parse(&fitted),
        None => Ok(drawing),
    }
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
    let root_end = svg.find('>')?;
    let start = svg[..root_end].find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + svg[start..root_end].find('"')?;
    Some(format!(
        "{}{} {} {} {}{}",
        &svg[..start],
        x0,
        y0,
        x1 - x0,
        y1 - y0,
        &svg[end..]
    ))
}

/// The diagram as a figure, in the colours of the appearance there is.
pub fn view(source: String) -> impl IntoView {
    dyn_container(theme::appearance, move |_| match render(&source) {
        Ok(drawing) => picture::figure(drawing).into_any(),
        Err(why) => Label::derived(move || format!("The diagram couldn't be drawn: {why}"))
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .text_wrap()
                    .width_full()
            })
            .into_any(),
    })
    .debug_name("walkthrough-diagram")
    .style(|s| s.width_full())
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
