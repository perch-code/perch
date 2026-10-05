//! A face as the window draws it: cropped square and cut to its circle
//! here, once, as it arrives, so the view draws the picture as it is. An
//! app's face is cut to a rounded square instead, as GitHub draws a bot.
//!
//! The view could clip it round, but vger, the renderer, clips with a hard
//! edge and rounds all four corners of a clip by the top-left one's
//! radius, so a face half under the top of a list, where its clip is cut
//! square, would draw square. A circle in the picture's alpha has a soft
//! rim whatever draws it.
//!
//! It is shrunk here too. GitHub sends identicons at 420 px whatever size
//! is asked for, and vger keeps a picture in its atlas at its own size: a
//! few of those filled the atlas past the point where it clears itself,
//! so it cleared and refilled every frame, and the face that no longer
//! fit was never drawn.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Cursor;
use std::sync::{LazyLock, Mutex};

use image::{imageops, ImageFormat};

/// Faces already cut, by a hash of what GitHub sent and the shape. The
/// rows send every face again from disk each time they change, and the
/// cut, a decode and an encode, is most of what that costs: some
/// milliseconds a face in a debug build.
static CUT: LazyLock<Mutex<HashMap<u64, Vec<u8>>>> = LazyLock::new(Default::default);

/// The most pixels a side a face keeps: what `GitHub::fetch_avatar` asks
/// for, and more than the largest face drawn (24 pt) needs at 2x.
const SIDE: u32 = 64;

/// The face as the window draws it, from what GitHub sent to `url`: its
/// `cut`, round or, for an app, a rounded square; or the bytes as they
/// came when they are not a picture.
pub(crate) fn drawn(bytes: Vec<u8>, url: &str) -> Vec<u8> {
    let app = perch_github::is_app_avatar(url);
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    app.hash(&mut hasher);
    let key = hasher.finish();
    if let Some(cut) = CUT.lock().ok().and_then(|cut| cut.get(&key).cloned()) {
        return cut;
    }
    match cut(
        &bytes,
        if app {
            crate::theme::APP_AVATAR_CORNER as f32
        } else {
            0.5
        },
    ) {
        Some(cut) => {
            if let Ok(mut memo) = CUT.lock() {
                memo.insert(key, cut.clone());
            }
            cut
        }
        None => bytes,
    }
}

/// The picture in `bytes` cropped square about its centre, shrunk to
/// `SIDE` if it is bigger, with its corners rounded by `corner` of its
/// side, as PNG: a half is a circle. Only the alpha changes: the colour
/// under the rim stays the picture's, so scaling the picture blends the
/// rim with its own colours rather than with black. `None` for bytes that
/// are not a picture.
fn cut(bytes: &[u8], corner: f32) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(bytes).ok()?.into_rgba8();
    let side = picture.width().min(picture.height());
    if side == 0 {
        return None;
    }
    let (x, y) = ((picture.width() - side) / 2, (picture.height() - side) / 2);
    let mut square = imageops::crop_imm(&picture, x, y, side, side).to_image();
    if side > SIDE {
        square = imageops::resize(&square, SIDE, SIDE, imageops::FilterType::Lanczos3);
    }
    let half = square.width() as f32 / 2.0;
    let radius = square.width() as f32 * corner;
    for (px, py, pixel) in square.enumerate_pixels_mut() {
        // How far past the straight edges toward a corner's centre the
        // pixel is, and so how much of it is inside, over a rim a pixel
        // wide.
        let (dx, dy) = (
            ((px as f32 + 0.5 - half).abs() - (half - radius)).max(0.0),
            ((py as f32 + 0.5 - half).abs() - (half - radius)).max(0.0),
        );
        let inside = (radius - dx.hypot(dy) + 0.5).clamp(0.0, 1.0);
        pixel[3] = (f32::from(pixel[3]) * inside).round() as u8;
    }
    let mut png = Vec::new();
    square
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some(png)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    /// A picture of one opaque colour, `width` by `height`, as PNG.
    pub(crate) fn picture(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        RgbaImage::from_pixel(width, height, Rgba([200, 40, 90, 255]))
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        png
    }

    /// A wide picture comes back square about its middle, clear in the
    /// corners, solid inside, and soft at the rim, in its own colour
    /// all over.
    #[test]
    fn a_face_is_cropped_square_and_cut_to_a_soft_circle() {
        let cut = image::load_from_memory(&drawn(picture(40, 32), "https://avatars/u/1"))
            .unwrap()
            .into_rgba8();
        assert_eq!(cut.dimensions(), (32, 32));
        let alpha = |x, y| cut.get_pixel(x, y)[3];
        for (x, y) in [(0, 0), (31, 0), (0, 31), (31, 31), (3, 3)] {
            assert_eq!(alpha(x, y), 0, "({x}, {y}) is outside the circle");
        }
        for (x, y) in [(16, 16), (16, 1), (1, 16), (30, 16), (16, 30)] {
            assert_eq!(alpha(x, y), 255, "({x}, {y}) is inside the circle");
        }
        let rim = cut.pixels().filter(|p| p[3] > 0 && p[3] < 255).count();
        assert!(rim > 16, "the rim is stepped: {rim} pixels between");
        assert!(cut.pixels().all(|p| p.0[..3] == [200, 40, 90]));
    }

    /// An identicon, which GitHub sends at 420 px, comes back at the size
    /// asked for, so it takes no more of vger's atlas than any other face.
    #[test]
    fn a_big_face_is_shrunk_to_the_size_asked_for() {
        let cut = image::load_from_memory(&drawn(picture(420, 420), "https://avatars/u/1"))
            .unwrap()
            .into_rgba8();
        assert_eq!(cut.dimensions(), (SIDE, SIDE));
        assert_eq!(
            cut.get_pixel(0, 0)[3],
            0,
            "the corner is outside the circle"
        );
        assert_eq!(cut.get_pixel(SIDE / 2, SIDE / 2)[3], 255);
    }

    /// The same picture sent again is cut once: the second time is the
    /// first's bytes.
    #[test]
    fn a_face_sent_again_is_not_cut_again() {
        let sent = picture(24, 24);
        let first = drawn(sent.clone(), "https://avatars/u/1");
        let mut hasher = DefaultHasher::new();
        sent.hash(&mut hasher);
        false.hash(&mut hasher);
        let key = hasher.finish();
        assert_eq!(CUT.lock().unwrap().get(&key), Some(&first));
        assert_eq!(drawn(sent, "https://avatars/u/1"), first);
    }

    /// Bytes that are not a picture go through as they came, for the
    /// view to fail on as it would have.
    /// An app's face keeps more of its corners than a person's: clear
    /// only in the rounding, solid where a circle would have cut it.
    #[test]
    fn an_apps_face_is_cut_to_a_rounded_square() {
        let cut = image::load_from_memory(&drawn(
            picture(32, 32),
            "https://avatars.githubusercontent.com/in/15368?v=4",
        ))
        .unwrap()
        .into_rgba8();
        let alpha = |x, y| cut.get_pixel(x, y)[3];
        assert_eq!(alpha(0, 0), 0, "the very corner is rounded off");
        assert_eq!(
            alpha(3, 3),
            255,
            "a circle would cut here; the square keeps it"
        );
        assert_eq!(alpha(16, 0), 255, "the edges are straight");
    }

    #[test]
    fn what_is_not_a_picture_goes_through_as_it_came() {
        let bytes = b"<html>not found</html>".to_vec();
        assert_eq!(drawn(bytes.clone(), "https://avatars/u/1"), bytes);
    }
}
