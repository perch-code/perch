//! Which rows of the worktree tree are on screen.
//!
//! A checkout of any size has thousands of entries, so the rail shows a
//! directory's contents only while that directory is expanded. The rule is
//! pure and tested, because getting it wrong either hides files that should
//! show or floods the rail with the whole repository.

use std::collections::HashSet;

use perch_git::TreeEntry;

/// Entries whose every ancestor directory is expanded.
///
/// Order is preserved, so a directory is still immediately followed by its
/// own contents.
pub fn visible_entries(all: &[TreeEntry], expanded: &HashSet<String>) -> Vec<TreeEntry> {
    all.iter()
        .filter(|entry| ancestors(&entry.path).all(|dir| expanded.contains(dir)))
        .cloned()
        .collect()
}

/// Every directory above `path`, from the top down.
///
/// `a/b/c.rs` yields `a` then `a/b`. A top-level entry yields nothing, so it
/// is always visible.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(i, _)| &path[..i])
}

/// Expand or collapse one directory.
///
/// Collapsing also drops anything expanded beneath it, so reopening a
/// directory shows its immediate contents rather than restoring a tree the
/// reader has long forgotten.
pub fn toggle(expanded: &mut HashSet<String>, dir: &str) {
    if expanded.remove(dir) {
        let prefix = format!("{dir}/");
        expanded.retain(|p| !p.starts_with(&prefix));
    } else {
        expanded.insert(dir.to_string());
    }
}

/// Expand every directory on the way to `path`, so a file can be revealed.
pub fn reveal(expanded: &mut HashSet<String>, path: &str) {
    for dir in ancestors(path) {
        expanded.insert(dir.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> TreeEntry {
        TreeEntry {
            depth: path.matches('/').count(),
            path: path.to_string(),
            is_dir,
        }
    }

    fn fixture() -> Vec<TreeEntry> {
        vec![
            entry("src", true),
            entry("src/ui", true),
            entry("src/ui/row.rs", false),
            entry("src/ui/mod.rs", false),
            entry("src/main.rs", false),
            entry("docs", true),
            entry("docs/guide.md", false),
            entry("README.md", false),
        ]
    }

    fn paths(entries: &[TreeEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn nothing_expanded_shows_only_the_top_level() {
        let visible = visible_entries(&fixture(), &HashSet::new());
        assert_eq!(paths(&visible), ["src", "docs", "README.md"]);
    }

    #[test]
    fn expanding_a_directory_reveals_its_immediate_contents_only() {
        let expanded = HashSet::from(["src".to_string()]);
        let visible = visible_entries(&fixture(), &expanded);
        assert_eq!(
            paths(&visible),
            ["src", "src/ui", "src/main.rs", "docs", "README.md"],
            "src/ui's contents stay hidden until it is expanded too"
        );
    }

    #[test]
    fn a_nested_directory_needs_every_ancestor_expanded() {
        // Expanding only the inner one must not leak it into view.
        let inner_only = HashSet::from(["src/ui".to_string()]);
        assert_eq!(
            paths(&visible_entries(&fixture(), &inner_only)),
            ["src", "docs", "README.md"]
        );

        let both = HashSet::from(["src".to_string(), "src/ui".to_string()]);
        assert_eq!(
            paths(&visible_entries(&fixture(), &both)),
            ["src", "src/ui", "src/ui/row.rs", "src/ui/mod.rs", "src/main.rs", "docs", "README.md"]
        );
    }

    #[test]
    fn order_is_preserved_so_contents_follow_their_directory() {
        let expanded = HashSet::from(["src".to_string(), "src/ui".to_string()]);
        let visible = visible_entries(&fixture(), &expanded);
        let ui = visible.iter().position(|e| e.path == "src/ui").unwrap();
        let row = visible.iter().position(|e| e.path == "src/ui/row.rs").unwrap();
        assert_eq!(row, ui + 1);
    }

    #[test]
    fn toggling_expands_then_collapses() {
        let mut expanded = HashSet::new();
        toggle(&mut expanded, "src");
        assert!(expanded.contains("src"));
        toggle(&mut expanded, "src");
        assert!(!expanded.contains("src"));
    }

    #[test]
    fn collapsing_forgets_what_was_open_underneath() {
        let mut expanded = HashSet::from(["src".to_string(), "src/ui".to_string()]);
        toggle(&mut expanded, "src");
        assert!(
            !expanded.contains("src/ui"),
            "reopening src should show its own contents, not a tree from before"
        );
    }

    #[test]
    fn collapsing_leaves_unrelated_directories_alone() {
        let mut expanded = HashSet::from([
            "src".to_string(),
            "src/ui".to_string(),
            "docs".to_string(),
        ]);
        toggle(&mut expanded, "src");
        assert!(expanded.contains("docs"));
    }

    #[test]
    fn a_prefix_match_is_not_a_child() {
        // "src2" is not inside "src" and must survive collapsing it.
        let mut expanded = HashSet::from(["src".to_string(), "src2".to_string()]);
        toggle(&mut expanded, "src");
        assert!(expanded.contains("src2"));
    }

    #[test]
    fn revealing_a_file_opens_the_path_to_it() {
        let mut expanded = HashSet::new();
        reveal(&mut expanded, "src/ui/row.rs");
        assert!(expanded.contains("src"));
        assert!(expanded.contains("src/ui"));
        assert!(!expanded.contains("src/ui/row.rs"), "the file itself is not a directory");

        let visible = visible_entries(&fixture(), &expanded);
        assert!(paths(&visible).contains(&"src/ui/row.rs"));
    }

    #[test]
    fn revealing_a_top_level_file_expands_nothing() {
        let mut expanded = HashSet::new();
        reveal(&mut expanded, "README.md");
        assert!(expanded.is_empty());
    }
}
