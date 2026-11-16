//! The pictures a comment shows: a screenshot pasted into a pull request,
//! drawn in the thread as GitHub draws it rather than named by a link.
//!
//! A picture is asked for the first time a view shows it, downloaded by
//! the engine, and decoded here, off the main thread, into the pixels
//! the renderer draws. What came back is kept for the session, by
//! address, for every window: a thread drawn again, or the same comment
//! in another window, draws at once.
//!
//! It is shrunk to [`MAX_SIDE`] as it is decoded. vger keeps a picture in
//! its atlas at its own size and grows the atlas, for good, to twice the
//! longest side it has seen: a retina screenshot as it came, 3000 pixels
//! and more across, would leave atlases of 6000 pixels a side behind it,
//! hundreds of megabytes of video memory, for a picture drawn in a column
//! narrower than half of that.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Cursor;
use std::sync::Arc;

use floem::context::PaintCx;
use floem::kurbo::Size;
use floem::peniko::{Blob, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};
use floem::reactive::{Memo, RwSignal, Scope, SignalUpdate, SignalWith};
use floem::{Renderer, View, ViewId};
use image::imageops::FilterType;
use perch_engine::review::images::Downloaded;

/// The most pixels a side of a picture keeps: a comment's column at 2x.
pub const MAX_SIDE: u32 = 1536;

/// How much of the decoded pictures the session keeps for views yet to be
/// drawn. A view already showing one holds its own, so going past this
/// only means a picture scrolled back to is downloaded again.
const KEPT_BYTES: usize = 192 * 1024 * 1024;

/// A picture ready to draw.
pub struct Decoded {
    brush: ImageBrush,
    /// The renderer's key for it, from its address.
    hash: [u8; 8],
    /// Its size as it came, in pixels: GitHub draws a picture a pixel to
    /// a point, as wide as that or the column, whichever is narrower.
    pub natural: (u32, u32),
}

impl std::fmt::Debug for Decoded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoded")
            .field("natural", &self.natural)
            .field("kept", &(self.brush.image.width, self.brush.image.height))
            .finish()
    }
}

impl Decoded {
    fn bytes(&self) -> usize {
        self.brush.image.data.len()
    }
}

/// One picture is another when it is the same address's.
impl PartialEq for Decoded {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.natural == other.natural
    }
}

/// The bytes downloaded from `url` as a picture to draw, shrunk to
/// [`MAX_SIDE`]; `None` when they are not a picture this decodes, or one
/// too large to. Slow for a big one: off the main thread.
pub fn decode(bytes: &[u8], url: &str) -> Option<Decoded> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let natural = (picture.width(), picture.height());
    if natural.0 == 0 || natural.1 == 0 {
        return None;
    }
    let picture = if natural.0.max(natural.1) > MAX_SIDE {
        picture.resize(MAX_SIDE, MAX_SIDE, FilterType::CatmullRom)
    } else {
        picture
    };
    let pixels = picture.into_rgba8();
    let (width, height) = pixels.dimensions();
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let brush = ImageBrush::new(ImageData {
        data: Blob::new(Arc::new(pixels.into_raw())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
    .with_quality(ImageQuality::High);
    Some(Decoded {
        brush,
        hash: hasher.finish().to_be_bytes(),
        natural,
    })
}

/// Whether a picture's address is one to download: a relative one names a
/// file in a repository whose root this does not know.
pub fn fetchable(src: &str) -> bool {
    src.starts_with("https://") || src.starts_with("http://")
}

struct Store {
    /// What the store's signal, and each decode's way back to the main
    /// thread, are made under.
    scope: Scope,
    pictures: RwSignal<HashMap<String, Arc<Decoded>>>,
    /// Every address asked for, so each is asked for once.
    asked: RefCell<HashSet<String>>,
    /// The addresses being decoded, so a picture every window hears of is
    /// decoded once.
    decoding: RefCell<HashSet<String>>,
    /// The pictures kept, oldest first, with what each weighs.
    kept: RefCell<VecDeque<(String, usize)>>,
}

thread_local! {
    /// The pictures of every window. Its signal outlives any view that
    /// first asked for it.
    static STORE: Store = {
        let scope = Scope::new();
        scope.enter(|| Store {
            scope,
            pictures: RwSignal::new(HashMap::new()),
            asked: RefCell::default(),
            decoding: RefCell::default(),
            kept: RefCell::default(),
        })
    };
}

/// Have the picture at `src` downloaded, unless it has been asked for or
/// cannot be.
pub fn want(src: &str) {
    if !fetchable(src) {
        return;
    }
    let first = STORE.with(|store| store.asked.borrow_mut().insert(src.to_string()));
    if first {
        ask(src);
    }
}

fn ask(src: &str) {
    #[cfg(test)]
    ASKED.with(|asked| asked.borrow_mut().push(src.to_string()));
    crate::windows::want_picture(src);
}

#[cfg(test)]
thread_local! {
    static ASKED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The addresses asked for since the last call.
#[cfg(test)]
pub(crate) fn asked() -> Vec<String> {
    ASKED.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
}

/// What the engine downloaded from `url`, `None` when it could not. Each
/// window hears it; the first has it decoded on a thread of its own,
/// since a big one is slow, and put in place by [`arrived`].
pub fn received(url: String, downloaded: Option<Downloaded>) {
    let Some(Downloaded(bytes)) = downloaded else {
        arrived(url, None);
        return;
    };
    let Some(scope) = claim(&url) else {
        return;
    };
    let done =
        floem::ext_event::create_ext_action(scope, |(url, decoded): (String, Option<Decoded>)| {
            decoded_from(url, decoded);
        });
    std::thread::spawn(move || {
        let decoded = decode(&bytes, &url);
        if decoded.is_none() {
            tracing::debug!(url, "picture did not decode");
        }
        done((url, decoded));
    });
}

/// The store's scope, for the decode's way back, when the picture at
/// `url` is this window's to decode: none is drawn yet and no other window
/// has started on it.
fn claim(url: &str) -> Option<Scope> {
    STORE.with(|store| {
        let have = store
            .pictures
            .with_untracked(|pictures| pictures.contains_key(url));
        (!have && store.decoding.borrow_mut().insert(url.to_string())).then_some(store.scope)
    })
}

/// A decode [`claim`]ed for `url` is done, `None` when the bytes were not
/// a picture.
fn decoded_from(url: String, decoded: Option<Decoded>) {
    STORE.with(|store| store.decoding.borrow_mut().remove(&url));
    arrived(url, decoded.map(Arc::new));
}

/// The picture at `url` decoded, `None` when it could not be downloaded
/// or decoded. One that failed is forgotten, to be asked for again the
/// next time a view shows it.
pub fn arrived(url: String, decoded: Option<Arc<Decoded>>) {
    STORE.with(|store| {
        let Some(decoded) = decoded else {
            store.asked.borrow_mut().remove(&url);
            return;
        };
        if store
            .pictures
            .with_untracked(|pictures| pictures.contains_key(&url))
        {
            return;
        }
        let gone = {
            let mut kept = store.kept.borrow_mut();
            kept.push_back((url.clone(), decoded.bytes()));
            trim(&mut kept, KEPT_BYTES)
        };
        let mut asked = store.asked.borrow_mut();
        for url in &gone {
            asked.remove(url);
        }
        store.pictures.update(|pictures| {
            for url in &gone {
                pictures.remove(url);
            }
            pictures.insert(url, decoded);
        });
    });
}

/// The oldest of `kept` let go until the rest weigh `budget` or less, or
/// only the newest is left; the addresses let go.
fn trim(kept: &mut VecDeque<(String, usize)>, budget: usize) -> Vec<String> {
    let mut total: usize = kept.iter().map(|(_, bytes)| bytes).sum();
    let mut gone = Vec::new();
    while total > budget && kept.len() > 1 {
        let Some((url, bytes)) = kept.pop_front() else {
            break;
        };
        total -= bytes;
        gone.push(url);
    }
    gone
}

/// The picture at `src`, `None` until it has come. Once shown it stays
/// shown here, though the session lets it go.
pub fn watch(src: String) -> Memo<Option<Arc<Decoded>>> {
    let pictures = STORE.with(|store| store.pictures);
    Memo::new(move |before: Option<&Option<Arc<Decoded>>>| {
        pictures
            .with(|pictures| pictures.get(&src).cloned())
            .or_else(|| before.cloned().flatten())
    })
}

/// A picture, filling its view.
pub struct Shown {
    id: ViewId,
    decoded: Arc<Decoded>,
    /// Whether it has been painted. The renderer's atlas grows to take a
    /// picture larger than it holds only once a frame is done, so the
    /// first paint of one may draw nothing: it asks for one more.
    painted: bool,
}

impl Shown {
    pub fn new(decoded: Arc<Decoded>) -> Self {
        Self {
            id: ViewId::new(),
            decoded,
            painted: false,
        }
    }
}

// What the pictures were drawn at in the last paint, for the tests: the
// headless renderer draws nothing.
#[cfg(test)]
thread_local! {
    pub(crate) static DRAWN: RefCell<Vec<((u32, u32), floem::kurbo::Rect)>> = const { RefCell::new(Vec::new()) };
}

impl View for Shown {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> Cow<'static, str> {
        "comment picture".into()
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
        DRAWN.with(|drawn| drawn.borrow_mut().push((self.decoded.natural, rect)));
        cx.draw_img(
            floem_renderer::Img {
                img: self.decoded.brush.clone(),
                hash: &self.decoded.hash,
            },
            rect,
        );
        if !self.painted {
            self.painted = true;
            self.id.request_paint();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use floem::reactive::SignalGet;
    use image::{Rgba, RgbaImage};

    /// A picture of one colour, `width` by `height`, as PNG.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(width, height, Rgba([30, 120, 200, 255]))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn a_big_picture_is_shrunk_to_the_most_a_side_keeps_and_remembers_its_size() {
        let decoded = decode(&png(3024, 1964), "https://x/big.png").unwrap();
        assert_eq!(decoded.natural, (3024, 1964));
        assert_eq!(decoded.brush.image.width, MAX_SIDE);
        assert_eq!(
            decoded.brush.image.height,
            (1964.0 * MAX_SIDE as f64 / 3024.0).round() as u32
        );
        assert_eq!(
            decoded.bytes(),
            (decoded.brush.image.width * decoded.brush.image.height * 4) as usize
        );

        let small = decode(&png(40, 20), "https://x/small.png").unwrap();
        assert_eq!(small.natural, (40, 20));
        assert_eq!(
            (small.brush.image.width, small.brush.image.height),
            (40, 20)
        );
        assert_ne!(small.hash, decoded.hash, "each address is its own key");
    }

    #[test]
    fn what_is_not_a_picture_does_not_decode() {
        assert!(decode(b"<html>Not Found</html>", "https://x/a.png").is_none());
        assert!(decode(&[], "https://x/a.png").is_none());
    }

    #[test]
    fn a_picture_is_asked_for_once_and_only_from_the_web() {
        asked();
        want("https://x/once.png");
        want("https://x/once.png");
        want("relative/path.png");
        want("data:image/png;base64,AAAA");
        assert_eq!(asked(), vec!["https://x/once.png".to_string()]);
    }

    #[test]
    fn a_shown_picture_stays_shown_and_the_first_window_to_hear_it_wins() {
        let _root = floem::headless::TestRoot::new();
        let url = "https://x/stays.png".to_string();
        let seen = watch(url.clone());
        assert_eq!(seen.get_untracked(), None);
        let decoded = Arc::new(decode(&png(4, 4), &url).unwrap());
        arrived(url.clone(), Some(decoded.clone()));
        let again = Arc::new(decode(&png(8, 8), &url).unwrap());
        arrived(url.clone(), Some(again));
        assert!(Arc::ptr_eq(&seen.get_untracked().unwrap(), &decoded));

        // Let go by the session, it is still what this view shows.
        STORE.with(|store| {
            store.pictures.update(|pictures| {
                pictures.remove(&url);
            })
        });
        assert!(Arc::ptr_eq(&seen.get_untracked().unwrap(), &decoded));
    }

    #[test]
    fn a_picture_that_failed_is_asked_for_again() {
        asked();
        let url = "https://x/failed.png";
        want(url);
        arrived(url.to_string(), None);
        want(url);
        assert_eq!(asked(), vec![url.to_string(), url.to_string()]);
    }

    /// Every window hears a download; the first has it decoded, and the
    /// rest, while that runs or once it has landed, do nothing.
    #[test]
    fn a_download_every_window_hears_is_decoded_once() {
        let _root = floem::headless::TestRoot::new();
        let url = "https://x/heard.png".to_string();
        received(url.clone(), Some(Downloaded(png(4, 4).into())));
        assert!(claim(&url).is_none(), "a second window decoded it too");

        decoded_from(url.clone(), decode(&png(4, 4), &url));
        assert!(watch(url.clone()).get_untracked().is_some());
        STORE.with(|store| assert!(!store.decoding.borrow().contains(&url)));
        assert!(claim(&url).is_none(), "decoded again once drawn");
    }

    /// Bytes that are not a picture, or no bytes, leave nothing behind:
    /// the next view to show it asks again, and that download is decoded.
    #[test]
    fn a_download_that_does_not_decode_is_asked_for_again() {
        let _root = floem::headless::TestRoot::new();
        asked();
        let url = "https://x/not-a-picture.png".to_string();
        want(&url);
        assert!(claim(&url).is_some());
        decoded_from(url.clone(), decode(b"<html>Not Found</html>", &url));
        STORE.with(|store| assert!(!store.decoding.borrow().contains(&url)));
        assert!(watch(url.clone()).get_untracked().is_none());
        want(&url);

        received(url.clone(), None);
        want(&url);
        assert_eq!(asked(), vec![url.clone(), url.clone(), url.clone()]);
        assert!(claim(&url).is_some(), "the next download is decoded");
    }

    #[test]
    fn the_oldest_pictures_go_past_the_budget_but_never_the_newest() {
        let mut kept: VecDeque<(String, usize)> = [("a", 40), ("b", 30), ("c", 20)]
            .map(|(url, bytes)| (url.to_string(), bytes))
            .into();
        assert_eq!(trim(&mut kept, 100), Vec::<String>::new());
        assert_eq!(trim(&mut kept, 50), vec!["a".to_string()]);
        assert_eq!(kept.len(), 2);
        assert_eq!(trim(&mut kept, 1), vec!["b".to_string()]);
        assert_eq!(kept, VecDeque::from([("c".to_string(), 20)]));
    }
}
