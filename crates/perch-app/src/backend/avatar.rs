//! A face as the window draws it: cropped square and cut to its circle
//! here, once, as it arrives, so the view draws the picture as it is.
//!
//! The view cannot cut it. vger, the renderer, clips with one rectangle at
//! a time and a hard edge, and an image fitted by cover sets its own
//! square clip over its parent's round one, so a clipped face came out
//! square. A circle in the picture's alpha has a soft rim whatever draws
//! it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Cursor;
use std::sync::{LazyLock, Mutex};

use image::{imageops, ImageFormat};

/// Faces already cut, by a hash of what GitHub sent. The rows send every
/// face again from disk each time they change, and the cut, a decode and
/// an encode, is most of what that costs: some milliseconds a face in a
/// debug build.
static CUT: LazyLock<Mutex<HashMap<u64, Vec<u8>>>> = LazyLock::new(Default::default);

/// The face as the window draws it, from what GitHub sent: its `circle`,
/// or the bytes as they came when they are not a picture.
pub(crate) fn drawn(bytes: Vec<u8>) -> Vec<u8> {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let key = hasher.finish();
    if let Some(cut) = CUT.lock().ok().and_then(|cut| cut.get(&key).cloned()) {
        return cut;
    }
    match circle(&bytes) {
        Some(cut) => {
            if let Ok(mut memo) = CUT.lock() {
                memo.insert(key, cut.clone());
            }
            cut
        }
        None => bytes,
    }
}

/// The picture in `bytes` cropped square about its centre, cut to the
/// circle inside that square, as PNG. Only the alpha changes: the colour
/// under the rim stays the picture's, so scaling the picture blends the
/// rim with its own colours rather than with black. `None` for bytes that
/// are not a picture.
fn circle(bytes: &[u8]) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(bytes).ok()?.into_rgba8();
    let side = picture.width().min(picture.height());
    if side == 0 {
        return None;
    }
    let (x, y) = ((picture.width() - side) / 2, (picture.height() - side) / 2);
    let mut square = imageops::crop_imm(&picture, x, y, side, side).to_image();
    let radius = side as f32 / 2.0;
    for (px, py, pixel) in square.enumerate_pixels_mut() {
        // How much of the pixel is inside, over a rim a pixel wide.
        let (dx, dy) = (px as f32 + 0.5 - radius, py as f32 + 0.5 - radius);
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
        let cut = image::load_from_memory(&drawn(picture(40, 32)))
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

    /// The same picture sent again is cut once: the second time is the
    /// first's bytes.
    #[test]
    fn a_face_sent_again_is_not_cut_again() {
        let sent = picture(24, 24);
        let first = drawn(sent.clone());
        let mut hasher = DefaultHasher::new();
        sent.hash(&mut hasher);
        let key = hasher.finish();
        assert_eq!(CUT.lock().unwrap().get(&key), Some(&first));
        assert_eq!(drawn(sent), first);
    }

    /// Bytes that are not a picture go through as they came, for the
    /// view to fail on as it would have.
    #[test]
    fn what_is_not_a_picture_goes_through_as_it_came() {
        let bytes = b"<html>not found</html>".to_vec();
        assert_eq!(drawn(bytes.clone()), bytes);
    }
}
