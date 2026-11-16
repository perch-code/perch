//! A file that draws as a picture: an SVG, whose markup the engine loads
//! as it loads any file's text and hands over as it is. The window parses
//! it into what the renderer draws (perch-app's `review::picture`).

/// An SVG file's two sides as markup. `head` is `None` for a deletion,
/// `base` for an added file or one outside the changed list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PictureSource {
    pub head: Option<String>,
    pub base: Option<String>,
}

impl PictureSource {
    /// The sides of the file at `path`, or `None` when the file is not one
    /// that draws: only an `.svg` is, by its extension in any case.
    pub fn of(path: &str, head: Option<&str>, base: Option<&str>) -> Option<PictureSource> {
        if !is_svg(path) {
            return None;
        }
        Some(PictureSource {
            head: head.map(str::to_string),
            base: base.map(str::to_string),
        })
    }
}

/// Whether a path names an SVG file.
pub fn is_svg(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}
