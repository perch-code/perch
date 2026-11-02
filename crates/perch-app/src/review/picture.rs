//! A file drawn as a picture: an SVG rendered in place of its markup, and
//! before beside after when the pull request changed it.
//!
//! An SVG is text, and loads as every other file does, diffed and
//! highlighted; the loader also parses each side into the tree the
//! renderer draws, so the pane can show what the file looks like rather
//! than what it is written as. The two sides of a change are drawn at
//! one scale, the one that fits the larger of them into a half, so a
//! change of size shows as one. The markup is a click away: the strip's
//! Source tile shows the file as the editor shows every other, with the
//! diff, the gutter and the threads anchored in it.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

use floem::context::{LayoutChanged, PaintCx};
use floem::kurbo::{Size, Stroke};
use floem::peniko::Brush;
use floem::prelude::*;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{Container, Decorators, Empty, Label, Stack};
use floem::{RendererSvg, View, ViewId};
use floem_renderer::usvg::{self, fontdb, Tree};
use perch_git::ChangeStatus;

use crate::icons::SidebarIcon;
use crate::review::model::LoadedFile;
use crate::theme;
use crate::ui::widgets::centered_message;

/// An SVG file's two sides as they draw. `head` is `None` for a
/// deletion, `base` for an added file or one outside the changed list,
/// and a side that is there but is not an SVG the renderer can draw is
/// `Err` with usvg's reason.
#[derive(Debug, Clone)]
pub struct Picture {
    pub head: Option<Result<Drawing, String>>,
    pub base: Option<Result<Drawing, String>>,
}

/// One side, parsed: the tree the renderer draws, and the key it keeps
/// the drawn scene under from one frame to the next.
#[derive(Debug, Clone)]
pub struct Drawing {
    pub tree: Arc<Tree>,
    hash: [u8; 8],
}

impl Picture {
    /// The picture of the file at `path`, or `None` when the file is not
    /// one that draws: only an `.svg` is, by its extension in any case.
    /// `head` is `None` for a deletion and `base` for a file with no base
    /// side.
    pub fn parse(path: &str, head: Option<&str>, base: Option<&str>) -> Option<Picture> {
        if !is_svg(path) {
            return None;
        }
        Some(Picture {
            head: head.map(Drawing::parse),
            base: base.map(Drawing::parse),
        })
    }

    /// The side the file is now: the head, or the base of a deletion.
    #[cfg(test)]
    pub fn current(&self) -> Option<&Result<Drawing, String>> {
        self.head.as_ref().or(self.base.as_ref())
    }
}

/// Whether a path names an SVG file.
pub fn is_svg(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

impl Drawing {
    pub(crate) fn parse(markup: &str) -> Result<Drawing, String> {
        let tree = Tree::from_str(markup, &options(markup)).map_err(|e| e.to_string())?;
        let mut hasher = std::hash::DefaultHasher::new();
        markup.hash(&mut hasher);
        Ok(Drawing {
            tree: Arc::new(tree),
            hash: hasher.finish().to_le_bytes(),
        })
    }

    /// The size the SVG says it is, in its own units.
    pub fn size(&self) -> Size {
        let size = self.tree.size();
        Size::new(f64::from(size.width()), f64::from(size.height()))
    }

    /// Where what it draws reaches, strokes and text included, in the
    /// units of [`size`](Self::size): past it when the SVG draws outside
    /// its own frame.
    pub fn bounds(&self) -> floem::kurbo::Rect {
        let b = self.tree.root().abs_stroke_bounding_box();
        floem::kurbo::Rect::new(
            f64::from(b.left()),
            f64::from(b.top()),
            f64::from(b.right()),
            f64::from(b.bottom()),
        )
    }
}

/// How usvg parses a file's markup. Text is set into paths as the file is
/// parsed, from the fonts usvg is given, and it is given none by default,
/// so a diagram's labels would be dropped: a file with text gets the
/// system's fonts, loaded once for the app's life. A file without keeps
/// the empty database, since loading the fonts takes a moment.
fn options(markup: &str) -> usvg::Options<'static> {
    let mut options = usvg::Options::default();
    if markup.contains("<text") {
        options.fontdb = fonts();
    }
    options
}

/// The system's fonts, loaded on the first call and kept.
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = fontdb::Database::new();
            fonts.load_system_fonts();
            #[cfg(not(target_os = "macos"))]
            use_system_generics(&mut fonts);
            Arc::new(fonts)
        })
        .clone()
}

/// fontdb takes `serif`, `sans-serif` and `monospace` for Times New Roman,
/// Arial and Courier New, and usvg falls back to `serif`, so on a Linux
/// desktop without Microsoft's fonts every label was dropped. Off macOS
/// the generics are the families fontconfig names for them, as fontique
/// has them, else whatever family is installed.
#[cfg(not(target_os = "macos"))]
fn use_system_generics(fonts: &mut fontdb::Database) {
    use floem::text::FONT_CONTEXT;
    use fontique::GenericFamily;

    let Some(any) = fonts.faces().find_map(|face| face.families.first()) else {
        return;
    };
    let any = any.0.clone();
    let mut context = FONT_CONTEXT.lock();
    let collection = &mut context.collection;
    let mut named = |generic| -> String {
        let installed = |name: &str| {
            fonts
                .faces()
                .any(|f| f.families.iter().any(|(n, _)| n == name))
        };
        collection
            .generic_families(generic)
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|id| collection.family_name(id).map(str::to_owned))
            .find(|name| installed(name))
            .unwrap_or_else(|| any.clone())
    };
    let serif = named(GenericFamily::Serif);
    let sans_serif = named(GenericFamily::SansSerif);
    let monospace = named(GenericFamily::Monospace);
    fonts.set_serif_family(serif);
    fonts.set_sans_serif_family(sans_serif);
    fonts.set_monospace_family(monospace);
}

/// `24 × 24`, with a fraction only when a side has one.
pub fn dims(size: Size) -> String {
    fn dim(v: f64) -> String {
        if (v - v.round()).abs() < 0.05 {
            format!("{:.0}", v.round())
        } else {
            format!("{v:.1}")
        }
    }
    format!("{} \u{d7} {}", dim(size.width), dim(size.height))
}

/// Between a caption's word and the side's size.
const CAPTION_SEPARATOR: &str = " \u{00b7} ";

// ---------------------------------------------------------------------------
// The pane
// ---------------------------------------------------------------------------

/// The pane for a file with a picture: the two sides of a changed file,
/// before on the left and after on the right, or the one side there is.
/// A side alone is captioned when it is the whole of an addition or a
/// deletion, so it is not taken for the file as it stands.
pub fn pane(file: &LoadedFile, picture: &Picture) -> impl IntoView {
    let Picture { head, base } = picture.clone();
    match (base, head) {
        (Some(base), Some(head)) => {
            let frame = frame_of(&[&base, &head]);
            Stack::horizontal((
                side("before", Some(caption("Before", &base)), base, frame)
                    .style(|s| s.border_right(1.0).border_color(theme::border_muted())),
                side("after", Some(caption("After", &head)), head, frame),
            ))
            .style(|s| s.size_full().min_height(0.0))
            .into_any()
        }
        (None, Some(head)) => {
            let word =
                matches!(file.status, Some(ChangeStatus::Added)).then(|| caption("Added", &head));
            let frame = frame_of(&[&head]);
            side("after", word, head, frame).into_any()
        }
        (Some(base), None) => {
            let word = caption("Deleted", &base);
            let frame = frame_of(&[&base]);
            side("before", Some(word), base, frame).into_any()
        }
        (None, None) => centered_message(
            SidebarIcon::Warning,
            || "Nothing to draw".to_string(),
            || "This file has no side to show.".to_string(),
        )
        .into_any(),
    }
}

/// One side: its caption, when it has one, over the picture, or over why
/// there is none. `name` tells the two apart in the tree.
fn side(
    name: &'static str,
    caption: Option<String>,
    drawing: Result<Drawing, String>,
    frame: Size,
) -> impl IntoView {
    let caption = match caption {
        Some(text) => Label::derived(move || text.clone())
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .padding_horiz(theme::PICTURE_INSET)
                    .padding_top(14.0)
                    .selectable(false)
                    .flex_shrink(0.0_f32)
            })
            .into_any(),
        None => Empty::new().into_any(),
    };
    let picture = match drawing {
        Ok(drawing) => drawn(drawing, frame).into_any(),
        Err(why) => centered_message(
            SidebarIcon::Warning,
            || "Could not draw this SVG".to_string(),
            move || why.clone(),
        )
        .into_any(),
    };
    Stack::vertical((caption, picture))
        .debug_name(format!("picture side {name}"))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height_full()
                .flex_col()
        })
}

/// `Before · 24 × 24`: which side, and its size when it drew.
fn caption(word: &str, drawing: &Result<Drawing, String>) -> String {
    match drawing {
        Ok(drawing) => format!("{word}{CAPTION_SEPARATOR}{}", dims(drawing.size())),
        Err(_) => word.to_string(),
    }
}

/// The size the sides of a split are fitted as one: the larger in each
/// direction, so the two draw at one scale. A side that did not parse
/// has no size to add.
fn frame_of(sides: &[&Result<Drawing, String>]) -> Size {
    sides
        .iter()
        .filter_map(|side| side.as_ref().ok())
        .map(Drawing::size)
        .fold(Size::ZERO, |a, b| {
            Size::new(a.width.max(b.width), a.height.max(b.height))
        })
}

/// The most a picture draws at, on a side, in logical pixels. The
/// renderer rasterizes a picture into its atlas at the size it is drawn,
/// and grows the atlas, once and for good, to twice the largest thing it
/// has held: at this cap a picture is 2048 device pixels on a 2x screen
/// and the atlas 4096 square, 80 MB across its colour and mask halves,
/// and a side of 4096 would ask for a texture past the 8192 wgpu allows.
pub(crate) const MAX_SIDE: f64 = 1024.0;

/// How big a picture `natural` big draws in a box `bounds` big: at the
/// one scale that fits `frame` into the box, keeping its shape. A
/// picture scales up as readily as down: a 16-pixel icon is drawn to be
/// looked at, and vector art loses nothing by it; but no further than
/// [`MAX_SIDE`], for the atlas. `None` when there is no room, or nothing
/// to fit.
fn fitted(natural: Size, frame: Size, bounds: Size) -> Option<Size> {
    if frame.width <= 0.0 || frame.height <= 0.0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let bounds = Size::new(bounds.width.min(MAX_SIDE), bounds.height.min(MAX_SIDE));
    let scale = (bounds.width / frame.width).min(bounds.height / frame.height);
    Some(Size::new(natural.width * scale, natural.height * scale))
}

// What the pictures were drawn at in the last paint, natural size and
// rect, oldest first, for the window tests: the headless renderer draws
// nothing, so this is what says the fit was right.
#[cfg(test)]
thread_local! {
    static DRAWN: std::cell::RefCell<Vec<(Size, floem::kurbo::Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// One SVG, filling its view. Under it, the wash and the hairline of its
/// canvas, so the file's extent shows, and so does a shape in the pane's
/// own white.
struct Drawn {
    id: ViewId,
    drawing: Drawing,
    /// Drawn straight onto what is under it, with no canvas or hairline:
    /// a figure in a column, not a file.
    plain: bool,
    /// The size the last paint drew at. The renderer keeps pictures in
    /// an atlas it grows only as a frame begins, so the first paint at a
    /// size the atlas cannot hold draws nothing; a paint at a new size
    /// asks for one more.
    painted: Size,
}

/// A picture in the room its side gives it: a box that takes the room,
/// and in it, centred, the picture sized by `fitted` from the box's
/// layout, so the box lays out first and the picture follows.
///
/// The picture is a view of its own size rather than drawn into the box
/// because floem's vello `draw_svg` scales a rect's origin along with
/// the picture: nothing at an icon's scale of one, and a picture drawn
/// at twelve times its size would land twelve times too far over. A view
/// draws at its own origin, which no scale moves, under vger as well.
fn drawn(drawing: Drawing, frame: Size) -> impl IntoView {
    let natural = drawing.size();
    let bounds = RwSignal::new(Size::ZERO);
    Container::new(
        Drawn {
            id: ViewId::new(),
            drawing,
            plain: false,
            painted: Size::ZERO,
        }
        .style(move |s| {
            let size = fitted(natural, frame, bounds.get()).unwrap_or(Size::ZERO);
            // Whole pixels, so the hairline is one pixel and not two half
            // ones, and never past the box; the shape drifts by less
            // than a pixel for it.
            s.width(size.width.floor())
                .height(size.height.floor())
                .flex_shrink(0.0_f32)
        }),
    )
    .debug_name("picture box")
    .on_event_stop(
        LayoutChanged::listener(),
        move |_,
              LayoutChanged {
                  new_content_box, ..
              }| {
            let size = new_content_box.size();
            if bounds.get_untracked() != size {
                bounds.set(size);
            }
        },
    )
    .style(|s| {
        s.flex_grow(1.0_f32)
            .min_height(0.0)
            .min_width(0.0)
            .margin(theme::PICTURE_INSET)
            .items_center()
            .justify_center()
    })
}

/// How big a figure `natural` big draws in a column `width` wide: as
/// it is, or smaller to fit the column, never larger; and no side past
/// [`MAX_SIDE`], for the atlas.
fn in_column(natural: Size, width: f64) -> Size {
    if natural.width <= 0.0 || natural.height <= 0.0 || width <= 0.0 {
        return Size::ZERO;
    }
    let scale = (width / natural.width)
        .min(MAX_SIDE / natural.width)
        .min(MAX_SIDE / natural.height)
        .min(1.0);
    Size::new(natural.width * scale, natural.height * scale)
}

/// A picture drawn plain at `size`, as a figure is: a tile of one too
/// big for the renderer to draw whole.
pub(crate) fn plain(drawing: Drawing, size: Size) -> impl IntoView {
    Drawn {
        id: ViewId::new(),
        drawing,
        plain: true,
        painted: Size::ZERO,
    }
    .style(move |s| s.width(size.width).height(size.height).flex_shrink(0.0_f32))
}

/// A picture set in a column of text, as a figure: at its own size, or
/// shrunk to the column's width, on whatever is under it.
pub(crate) fn figure(drawing: Drawing) -> impl IntoView {
    let natural = drawing.size();
    let width = RwSignal::new(0.0_f64);
    Container::new(
        Drawn {
            id: ViewId::new(),
            drawing,
            plain: true,
            painted: Size::ZERO,
        }
        .style(move |s| {
            let size = in_column(natural, width.get());
            s.width(size.width.floor())
                .height(size.height.floor())
                .flex_shrink(0.0_f32)
        }),
    )
    .debug_name("figure")
    .on_event_stop(
        LayoutChanged::listener(),
        move |_,
              LayoutChanged {
                  new_content_box, ..
              }| {
            let size = new_content_box.size().width;
            if width.get_untracked() != size {
                width.set(size);
            }
        },
    )
    .style(|s| s.width_full().justify_center())
}

impl View for Drawn {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> Cow<'static, str> {
        "picture".into()
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(layout) = self.id.get_layout() else {
            return;
        };
        let rect = Size::new(f64::from(layout.size.width), f64::from(layout.size.height)).to_rect();
        if rect.is_zero_area() {
            return;
        }
        #[cfg(test)]
        DRAWN.with(|drawn| drawn.borrow_mut().push((self.drawing.size(), rect)));
        if !self.plain {
            cx.fill(&rect, theme::picture_canvas(), 0.0);
            cx.stroke(&rect.inset(-0.5), theme::border_muted(), &Stroke::new(1.0));
        }
        // Clipped to its canvas: what an SVG draws outside its own bounds
        // would otherwise land on the side beside it.
        cx.clip(&rect);
        cx.draw_svg(
            RendererSvg {
                tree: &self.drawing.tree,
                hash: &self.drawing.hash,
            },
            rect,
            None::<&Brush>,
        );
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

    const CIRCLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><circle cx="12" cy="12" r="10" fill="#c00"/></svg>"##;
    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 16"><rect width="32" height="16" fill="#0c0"/></svg>"##;

    #[test]
    fn only_an_svg_is_a_picture_and_each_side_is_what_it_is_given() {
        assert!(Picture::parse("src/lib.rs", Some("fn a() {}"), None).is_none());
        assert!(Picture::parse("logo.svgz", Some(CIRCLE), None).is_none());
        assert!(
            Picture::parse("svg", Some(CIRCLE), None).is_none(),
            "no extension"
        );

        // Added: the head alone.
        let added = Picture::parse("assets/logo.svg", Some(CIRCLE), None).unwrap();
        assert!(added.base.is_none());
        let head = added.head.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(head.size(), Size::new(24.0, 24.0));
        assert_eq!(dims(head.size()), "24 \u{d7} 24");

        // Deleted: the base alone, and it is what the file is now.
        let deleted = Picture::parse("Logo.SVG", None, Some(SQUARE)).unwrap();
        assert!(deleted.head.is_none());
        assert_eq!(
            deleted.current().unwrap().as_ref().unwrap().size(),
            Size::new(32.0, 16.0),
            "the viewBox gives the size when there is no width and height"
        );

        // Modified: both, and the head is current.
        let modified = Picture::parse("a.svg", Some(CIRCLE), Some(SQUARE)).unwrap();
        assert_eq!(
            modified.current().unwrap().as_ref().unwrap().size(),
            Size::new(24.0, 24.0)
        );
    }

    #[test]
    fn a_side_that_is_not_an_svg_says_why_and_the_others_still_draw() {
        let picture = Picture::parse("a.svg", Some("<svg>"), Some(CIRCLE)).unwrap();
        let head = picture.head.as_ref().unwrap();
        assert!(head.is_err(), "{head:?}");
        assert!(picture.base.as_ref().unwrap().is_ok());
        // The frame of a split is the sides that drew.
        assert_eq!(
            frame_of(&[picture.base.as_ref().unwrap(), head]),
            Size::new(24.0, 24.0)
        );
        assert_eq!(caption("After", head), "After");
        assert_eq!(
            caption("Before", picture.base.as_ref().unwrap()),
            "Before \u{00b7} 24 \u{d7} 24"
        );
    }

    #[test]
    fn two_drawings_of_the_same_markup_share_a_key_and_of_different_markup_do_not() {
        let a = Drawing::parse(CIRCLE).unwrap();
        let b = Drawing::parse(CIRCLE).unwrap();
        let c = Drawing::parse(SQUARE).unwrap();
        assert_eq!(a.hash, b.hash);
        assert_ne!(a.hash, c.hash);
    }

    #[test]
    fn text_is_set_in_the_systems_fonts() {
        if fonts().is_empty() {
            // Nothing to set it in here; the file parses all the same.
            return;
        }
        let labelled = r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="20"><text x="2" y="15" font-size="12">hi</text></svg>"#;
        let drawing = Drawing::parse(labelled).unwrap();
        // With no fonts the text would be dropped and the root empty; set
        // in a system font, it is paths.
        assert!(
            drawing.tree.root().has_children(),
            "the label was dropped for want of a font"
        );
    }

    #[test]
    fn a_picture_is_fitted_at_one_scale_for_its_frame() {
        let bounds = Size::new(400.0, 300.0);
        // Alone, a 24-square fills the shorter side.
        let alone = fitted(Size::new(24.0, 24.0), Size::new(24.0, 24.0), bounds).unwrap();
        assert_eq!(alone, Size::new(300.0, 300.0));
        // Beside a 32-square it draws at the 32-square's scale, three
        // quarters the size.
        let beside = fitted(Size::new(24.0, 24.0), Size::new(32.0, 32.0), bounds).unwrap();
        assert_eq!(beside, Size::new(225.0, 225.0));
        // A wide picture fits its width.
        let wide = fitted(Size::new(32.0, 16.0), Size::new(32.0, 16.0), bounds).unwrap();
        assert_eq!(wide, Size::new(400.0, 200.0));
        // Nothing to fit into, or to fit.
        assert!(fitted(Size::new(24.0, 24.0), Size::new(24.0, 24.0), Size::ZERO).is_none());
        assert!(fitted(Size::new(24.0, 24.0), Size::ZERO, bounds).is_none());
        // Room past the cap is room the picture does not take, on
        // either side, and a picture beside a bigger one keeps its scale.
        let vast = Size::new(3000.0, 2000.0);
        let capped = fitted(Size::new(24.0, 24.0), Size::new(24.0, 24.0), vast).unwrap();
        assert_eq!(capped, Size::new(MAX_SIDE, MAX_SIDE));
        let wide = fitted(Size::new(32.0, 16.0), Size::new(32.0, 16.0), vast).unwrap();
        assert_eq!(wide, Size::new(MAX_SIDE, MAX_SIDE / 2.0));
        let beside = fitted(Size::new(24.0, 24.0), Size::new(32.0, 32.0), vast).unwrap();
        assert_eq!(beside, Size::new(MAX_SIDE * 0.75, MAX_SIDE * 0.75));
    }

    #[test]
    fn a_size_with_a_fraction_says_it_and_one_without_does_not() {
        assert_eq!(dims(Size::new(24.0, 24.0)), "24 \u{d7} 24");
        assert_eq!(dims(Size::new(24.0, 18.5)), "24 \u{d7} 18.5");
        assert_eq!(dims(Size::new(1200.0, 630.0)), "1200 \u{d7} 630");
    }
}

/// The pane in a headless window.
#[cfg(test)]
mod window {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::kurbo::{Rect, Size};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_git::ChangeStatus;
    use perch_github::{Item, ItemCheckout};

    use crate::review::model::{ItemTab, Phase, ReviewState};
    use crate::state::AppState;

    const CIRCLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><circle cx="12" cy="12" r="10" fill="#c00"/></svg>"##;
    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 16"><rect width="32" height="16" fill="#0c0"/></svg>"##;

    /// The whole window on Code with the file at `path` open, loaded
    /// from the given sides.
    fn window_with(
        path: &str,
        head: &str,
        base: &str,
        status: Option<ChangeStatus>,
    ) -> (HeadlessHarness, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
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
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.tab.set(ItemTab::Code);
        state.composer_autofocus.set(false);

        let file = Arc::new(crate::backend::build_loaded_file(
            path.into(),
            head.into(),
            base.into(),
            status,
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));
        state.open_tab(path);
        state.finish_load(path, file);
        // As a previous file's editor would have left it.
        state.selection.set(Some((0, 0)));

        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        super::DRAWN.with(|drawn| drawn.borrow_mut().clear());
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    /// The first view whose debug name contains `text`.
    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    /// The first view whose debug name is `text` and nothing more.
    fn find_exact(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name() == text {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find_exact(c, text))
    }

    /// The rects the pictures were drawn at in the last paint, by natural
    /// size.
    fn drawn() -> Vec<(Size, Rect)> {
        super::DRAWN.with(|drawn| drawn.borrow().clone())
    }

    fn repaint(harness: &mut HeadlessHarness) {
        super::DRAWN.with(|drawn| drawn.borrow_mut().clear());
        harness.rebuild();
        harness.paint();
    }

    #[test]
    fn a_changed_svg_is_drawn_before_beside_after_at_one_scale() {
        let (harness, state) = window_with(
            "assets/logo.svg",
            CIRCLE,
            SQUARE,
            Some(ChangeStatus::Modified),
        );
        let root = harness.root_id();
        let before = find(root, "picture side before").expect("the base side");
        let after = find(root, "picture side after").expect("the head side");
        let (left, right) = (
            before.get_visual_rect_no_clip(),
            after.get_visual_rect_no_clip(),
        );
        assert!(
            left.x1 <= right.x0 + 1.0,
            "before is left of after: {left:?} {right:?}"
        );
        assert!(
            (left.width() - right.width()).abs() < 2.0,
            "halves: {left:?} {right:?}"
        );
        assert!(left.width() > 300.0, "each half has room: {left:?}");
        assert!(find(before, "Label: \"Before \u{b7} 32 \u{d7} 16\"").is_some());
        assert!(find(after, "Label: \"After \u{b7} 24 \u{d7} 24\"").is_some());

        // Each picture is centred in the room its side gives it.
        for side in [before, after] {
            let picture = find_exact(side, "picture")
                .expect("the side's picture")
                .get_visual_rect_no_clip();
            let room = find(side, "picture box")
                .expect("the side's room")
                .get_visual_rect_no_clip();
            assert!(room.contains(picture.center()), "{picture:?} in {room:?}");
            assert!(
                (room.center() - picture.center()).hypot() < 1.5,
                "{picture:?} in {room:?}"
            );
            assert!(
                picture.width() > 100.0 && picture.height() > 100.0,
                "{picture:?}"
            );
        }

        // Both were drawn, at the one scale: the frame is 32 by 24, and
        // each side is its own size at the scale that fits the frame.
        let drawn = drawn();
        assert_eq!(drawn.len(), 2, "{drawn:?}");
        let scale_of = |natural: Size, rect: Rect| rect.width() / natural.width;
        let (square, circle) = (drawn[0], drawn[1]);
        assert_eq!(square.0, Size::new(32.0, 16.0));
        assert_eq!(circle.0, Size::new(24.0, 24.0));
        let (s1, s2) = (scale_of(square.0, square.1), scale_of(circle.0, circle.1));
        assert!((s1 - s2).abs() < 0.1, "one scale: {s1} {s2}");
        assert!(s1 > 5.0, "scaled up to be looked at: {s1}");

        // No editor: nothing registered a document, and nothing is
        // selected for a question to be anchored on.
        assert!(state.docs.get().is_none());
        assert!(state.selection.get().is_none());
        assert!(state.anchor_from_selection().is_none());
        // The strip has the Source tile.
        assert!(find(root, "source tile").is_some());
    }

    #[test]
    fn the_source_tile_shows_the_markup_and_the_picture_again() {
        let (mut harness, state) = window_with(
            "assets/logo.svg",
            CIRCLE,
            SQUARE,
            Some(ChangeStatus::Modified),
        );
        // Drawn, the file has no lines to jump between: the strip's
        // change navigation is gone, and the keys do nothing rather than
        // leave a jump for the next editor to take.
        let root = harness.root_id();
        assert!(find(root, "Label: \"1 change\"").is_none());
        crate::review::tabs::jump_to_change(state, true);
        assert!(state.jump_to_line.get().is_none());

        state.toggle_source();
        repaint(&mut harness);
        let root = harness.root_id();
        assert!(
            find(root, "picture side").is_none(),
            "the pictures are gone"
        );
        assert!(state.docs.get().is_some(), "the editor took their place");
        assert!(drawn().is_empty());
        assert!(
            find(root, "source tile").is_some(),
            "the tile stays, to flip back"
        );
        assert!(
            find(root, "Label: \"1 change\"").is_some(),
            "the markup has changes to jump between"
        );

        state.toggle_source();
        repaint(&mut harness);
        let root = harness.root_id();
        assert!(find(root, "picture side before").is_some());
        assert!(state.docs.get().is_none());
        assert_eq!(drawn().len(), 2);

        // The choice is the file's: another file opens as itself.
        assert!(!state.shows_source("assets/logo.svg"));
    }

    #[test]
    fn an_added_or_deleted_svg_is_one_captioned_side_and_a_plain_one_is_none() {
        let (harness, _) = window_with("new.svg", CIRCLE, "", Some(ChangeStatus::Added));
        let root = harness.root_id();
        assert!(find(root, "picture side before").is_none());
        let after = find(root, "picture side after").expect("the head alone");
        assert!(find(after, "Label: \"Added \u{b7} 24 \u{d7} 24\"").is_some());
        assert_eq!(drawn().len(), 1);
        drop(harness);

        let (harness, _) = window_with("old.svg", "", SQUARE, Some(ChangeStatus::Deleted));
        let root = harness.root_id();
        assert!(find(root, "picture side after").is_none());
        let before = find(root, "picture side before").expect("the base alone");
        assert!(find(before, "Label: \"Deleted \u{b7} 32 \u{d7} 16\"").is_some());
        drop(harness);

        // A file outside the changed list draws whole, with no caption.
        let (harness, _) = window_with("docs/arch.svg", SQUARE, "", None);
        let root = harness.root_id();
        let after = find(root, "picture side after").expect("the file as it is");
        assert!(find(after, "Label:").is_none(), "no caption");
        assert_eq!(drawn().len(), 1);
        drop(harness);

        // And a file that is only text has no picture and no tile.
        let (harness, state) = window_with(
            "src/x.rs",
            "fn a() {}\n",
            "fn b() {}\n",
            Some(ChangeStatus::Modified),
        );
        let root = harness.root_id();
        assert!(find(root, "picture side").is_none());
        assert!(find(root, "source tile").is_none());
        assert!(state.docs.get().is_some());
    }

    #[test]
    fn a_side_that_does_not_parse_says_so_beside_the_one_that_does() {
        let (harness, _) = window_with(
            "assets/logo.svg",
            "<svg",
            CIRCLE,
            Some(ChangeStatus::Modified),
        );
        let root = harness.root_id();
        let after = find(root, "picture side after").expect("the head side");
        assert!(find(after, "Label: \"Could not draw this SVG\"").is_some());
        assert!(find(after, "Label: \"After\"").is_some(), "no size to say");
        let drawn = drawn();
        assert_eq!(drawn.len(), 1, "only the base drew: {drawn:?}");
        assert_eq!(drawn[0].0, Size::new(24.0, 24.0));
    }
}
