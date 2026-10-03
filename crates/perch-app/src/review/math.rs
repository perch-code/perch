//! TeX math, typeset: `$O(n \log n)$` in a comment drawn as GitHub's
//! MathJax draws it, not as the dollar signs and backslashes it is
//! written in.
//!
//! latex-rust parses the TeX and lays it out by the TeXbook's rules
//! against STIX Two Math, which it bundles, and writes the result as an
//! SVG of the glyphs' outlines. That is parsed once into the tree the
//! renderer draws, and the formula keeps its box: its width, and how far
//! it rises above the baseline and drops below it, in ems. A formula in a
//! line of text is an inline box in the text's layout, as wide and as
//! tall above the baseline as it is, which [`crate::review::linked_text`]
//! draws it into, hanging its depth below; one on a line of its own is a
//! [`display`] view, centred.
//!
//! What latex-rust cannot set, an unknown command or text-mode TeX, it
//! refuses rather than guesses, and the formula is shown as written, as
//! code.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use floem::context::{LayoutChanged, PaintCx};
use floem::kurbo::{Rect, Size};
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{Container, Decorators};
use floem::{RendererSvg, View, ViewId};
use floem_renderer::usvg::{self, Tree};
use latex_rust::{Dim, MathFont, MathStyle, SvgOptions};

/// A formula's em against the text's: STIX Two's lower case is shorter
/// than San Francisco's, and this brings an `x` in a formula to the
/// height of one beside it, as MathJax matches the surrounding font's.
pub const SCALE: f32 = 1.1;

/// The longest formula typeset; a longer one is shown as written. Layout
/// is fast, but a comment is anyone's text.
const MAX_SOURCE: usize = 4096;

/// How many typeset formulas the session keeps before starting over.
const KEPT: usize = 1024;

/// The em of the SVG latex-rust writes, in its units. Its numbers carry
/// few decimals, so at a 1-unit em a glyph's outline would be rounded
/// out of shape; the renderer scales the drawing to its box either way.
const SVG_EM: i64 = 100;

/// A typeset formula.
pub struct Formula {
    /// The TeX it was set from, without its dollar signs.
    pub source: String,
    /// Set as display math, `$$…$$`, larger operators and limits above
    /// and below, rather than in a line of text.
    pub display: bool,
    tree: Tree,
    hash: [u8; 8],
    /// Its box, in ems: its width, and how far it rises above the
    /// baseline and drops below it.
    pub width: f64,
    pub height: f64,
    pub depth: f64,
}

impl std::fmt::Debug for Formula {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Formula")
            .field("source", &self.source)
            .field("display", &self.display)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("depth", &self.depth)
            .finish()
    }
}

/// One formula is another when it is the same TeX, set the same way.
impl PartialEq for Formula {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.display == other.display
    }
}

/// The formulas typeset so far, by TeX and whether set as display math,
/// with `None` for one that could not be.
type Typeset = HashMap<(String, bool), Option<Rc<Formula>>>;

thread_local! {
    static FONT: Option<MathFont> = MathFont::stix_two_math().ok();
    static SET: RefCell<Typeset> = RefCell::new(HashMap::new());
}

/// `source` typeset, in a line of text or, `display`, on a line of its
/// own; `None` when it cannot be. Kept for the session, so a thread drawn
/// again sets nothing again.
pub fn typeset(source: &str, display: bool) -> Option<Rc<Formula>> {
    let key = (source.to_string(), display);
    if let Some(set) = SET.with_borrow(|set| set.get(&key).cloned()) {
        return set;
    }
    let formula = set(source, display).map(Rc::new);
    SET.with_borrow_mut(|set| {
        if set.len() >= KEPT {
            set.clear();
        }
        set.insert(key, formula.clone());
    });
    formula
}

fn set(source: &str, display: bool) -> Option<Formula> {
    if source.trim().is_empty() || source.len() > MAX_SOURCE {
        return None;
    }
    FONT.with(|font| {
        let font = font.as_ref()?;
        let ast = latex_rust::parse(source).ok()?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let boxed = latex_rust::layout(&ast, font, style).ok()?;
        let options = SvgOptions {
            font_size_pt: Dim::from_i64(SVG_EM),
            ..SvgOptions::new()
        };
        let svg = latex_rust::render_svg(&boxed, font, &options).ok()?;
        let (width, height, depth) = (ems(&boxed.width)?, ems(&boxed.height)?, ems(&boxed.depth)?);
        if width <= 0.0 || height + depth <= 0.0 {
            return None;
        }
        let tree = Tree::from_str(&svg, &usvg::Options::default()).ok()?;
        let mut hasher = DefaultHasher::new();
        svg.hash(&mut hasher);
        Some(Formula {
            source: source.to_string(),
            display,
            tree,
            hash: hasher.finish().to_le_bytes(),
            width,
            height,
            depth,
        })
    })
}

fn ems(dim: &Dim) -> Option<f64> {
    let ems: f64 = dim.to_dec_string().parse().ok()?;
    ems.is_finite().then_some(ems)
}

impl Formula {
    /// Its box at `em`, a pixel size: width, and height above the
    /// baseline and below it.
    pub fn size_at(&self, em: f64) -> (f64, f64, f64) {
        (self.width * em, self.height * em, self.depth * em)
    }

    /// Draw it into `rect`, the whole of its box, ascent and depth, in
    /// `color`.
    pub fn draw(&self, cx: &mut PaintCx, rect: Rect, color: Color) {
        if rect.is_zero_area() {
            return;
        }
        cx.draw_svg(
            RendererSvg {
                tree: &self.tree,
                hash: &self.hash,
            },
            rect,
            Some(color),
        );
    }
}

/// A formula set on a line of its own, `$$…$$`, at `em`, centred in the
/// column; one wider than the column is shrunk to fit it.
pub fn display(formula: Rc<Formula>, em: f64, color: fn() -> Color) -> impl IntoView {
    let (width, height, depth) = formula.size_at(em);
    let natural = Size::new(width, height + depth);
    let room = RwSignal::new(0.0_f64);
    Container::new(
        Shown {
            id: ViewId::new(),
            formula,
            color,
            painted: Size::ZERO,
        }
        .style(move |s| {
            let room = room.get();
            let scale = if room > 0.0 && natural.width > room {
                room / natural.width
            } else {
                1.0
            };
            s.width(natural.width * scale)
                .height(natural.height * scale)
                .flex_shrink(0.0_f32)
        }),
    )
    .debug_name("display math")
    .on_event_stop(
        LayoutChanged::listener(),
        move |_,
              LayoutChanged {
                  new_content_box, ..
              }| {
            let width = new_content_box.width();
            if room.get_untracked() != width {
                room.set(width);
            }
        },
    )
    .style(|s| {
        s.width_full()
            .min_width(0.0)
            .justify_center()
            .items_center()
    })
}

/// A formula filling its view.
struct Shown {
    id: ViewId,
    formula: Rc<Formula>,
    color: fn() -> Color,
    /// The size the last paint drew at: the renderer grows its atlas
    /// only as a frame begins, so the first paint at a size the atlas
    /// cannot hold draws nothing, and a paint at a new size asks for one
    /// more.
    painted: Size,
}

impl View for Shown {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> Cow<'static, str> {
        format!("formula {:?}", self.formula.source).into()
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(layout) = self.id.get_layout() else {
            return;
        };
        let rect = Size::new(f64::from(layout.size.width), f64::from(layout.size.height)).to_rect();
        self.formula.draw(cx, rect, (self.color)());
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
    fn a_formula_is_set_with_a_box_that_sits_on_the_baseline() {
        let o = typeset(r"O(n \log n)", false).expect("set");
        assert!(o.width > 3.0 && o.width < 6.0, "{o:?}");
        // The parentheses rise above the letters and drop below the line.
        assert!(o.height > 0.6 && o.height < 1.0, "{o:?}");
        assert!(o.depth > 0.1 && o.depth < 0.4, "{o:?}");
        assert_eq!(o.size_at(10.0).0, o.width * 10.0);
    }

    #[test]
    fn display_math_is_set_larger_than_the_same_in_a_line() {
        let inline = typeset(r"\sum_{i=0}^n x_i", false).unwrap();
        let display = typeset(r"\sum_{i=0}^n x_i", true).unwrap();
        assert!(display.height + display.depth > inline.height + inline.depth + 0.5);
        assert!(display.display && !inline.display);
    }

    #[test]
    fn what_cannot_be_set_is_none() {
        assert!(typeset(r"\nosuchcommand{x}", false).is_none());
        assert!(typeset(r"\frac{1}{", false).is_none());
        assert!(typeset("  ", false).is_none());
        assert!(typeset(&"x".repeat(MAX_SOURCE + 1), false).is_none());
    }

    #[test]
    fn a_formula_set_again_is_the_one_kept() {
        let a = typeset("x^2", false).unwrap();
        let b = typeset("x^2", false).unwrap();
        assert!(Rc::ptr_eq(&a, &b));
        assert!(!Rc::ptr_eq(&a, &typeset("x^2", true).unwrap()));
    }
}
