//! The key an outline is kept under. The sticky lines it pins are
//! perch-app's `review::sticky`.

use std::hash::{Hash, Hasher};

/// A hash of a text's lines, which says whether an outline was counted
/// in the lines the editor shows. Lines rather than the text, so that the
/// file's last newline, which the head's lines drop, is no difference.
pub fn lines_hash<'a>(lines: impl Iterator<Item = &'a str>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in lines {
        line.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_hash_ignores_the_last_newline_and_nothing_else() {
        let hash = |text: &str| lines_hash(text.lines());
        assert_eq!(hash("a\nb\n"), hash("a\nb"));
        assert_ne!(hash("a\nb\n"), hash("a\nc\n"));
        assert_ne!(hash("ab\n"), hash("a\nb\n"));
    }
}
