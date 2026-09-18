//! Which rows of the worktree tree are on screen.
//!
//! A checkout of any size has thousands of entries, so the rail shows a
//! directory's contents only while that directory is expanded. With the
//! filter on, the tree is cut down to the changed files and the
//! directories on the way to them, and a run of directories that hold
//! nothing but the next is folded into one row. The rules are pure and
//! tested, because getting them wrong either hides files that should show
//! or floods the rail with the whole repository.

use std::collections::{HashMap, HashSet};

use perch_git::{compare_entries, TreeEntry};

/// One row of the rail's tree: an entry, or a run of directories folded
/// into one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Row {
    /// The entry the row stands for. For a folded run, the innermost
    /// directory: the one whose contents follow.
    pub path: String,
    /// The outermost directory of a folded run; `path` otherwise. Whether
    /// the row is on screen follows this, since the run is one row at the
    /// outermost's place in the tree.
    pub head: String,
    /// What the row says: the last component, or `a/b/c` for a folded run.
    pub label: String,
    pub is_dir: bool,
    /// How far to indent, with the folded-away directories not counted.
    pub depth: usize,
}

impl Row {
    /// The directories the row stands for, outermost first. One for a
    /// plain directory, none for a file.
    pub fn directories(&self) -> Vec<&str> {
        if !self.is_dir {
            return Vec::new();
        }
        ancestors(&self.path)
            .filter(|dir| dir.len() >= self.head.len())
            .chain(std::iter::once(self.path.as_str()))
            .collect()
    }
}

/// The tree with `paths` in it: any the worktree lacks are added, each
/// with the directories above it, in listing order. A deleted file is
/// gone from the checkout but is in the changed list, and its row has to
/// be somewhere. `None` when every path is already there, so the caller
/// can keep the list it has.
///
/// A path the worktree has as a directory, a submodule say, is there
/// already and gets no file beside it. A directory above a path that the
/// worktree has as a file, when a change turned the one into the other,
/// is added all the same: the listing's order puts a directory's contents
/// under it, and a file with contents under it is not a shape the order
/// can place.
pub fn with_paths<'a>(tree: &[TreeEntry], paths: impl IntoIterator<Item = &'a str>) -> Option<Vec<TreeEntry>> {
    let present: HashSet<(&str, bool)> = tree.iter().map(|e| (e.path.as_str(), e.is_dir)).collect();
    let mut missing: Vec<TreeEntry> = Vec::new();
    let mut added: HashSet<(String, bool)> = HashSet::new();
    for path in paths {
        if present.contains(&(path, false)) || present.contains(&(path, true)) {
            continue;
        }
        for dir in ancestors(path) {
            if !present.contains(&(dir, true)) && added.insert((dir.to_string(), true)) {
                missing.push(TreeEntry::new(dir, true));
            }
        }
        if added.insert((path.to_string(), false)) {
            missing.push(TreeEntry::new(path, false));
        }
    }
    if missing.is_empty() {
        return None;
    }
    let mut merged = tree.to_vec();
    for entry in missing {
        let at = merged.partition_point(|e| compare_entries(e, &entry).is_lt());
        merged.insert(at, entry);
    }
    Some(merged)
}

/// The changed files and the directories on the way to them, in the
/// order they came. A changed path the worktree has as a directory, a
/// submodule say, is kept as that directory.
pub fn changed_only<'a>(all: &[TreeEntry], changed: impl IntoIterator<Item = &'a str>) -> Vec<TreeEntry> {
    let files: HashSet<&str> = changed.into_iter().collect();
    let dirs: HashSet<&str> = files.iter().flat_map(|path| ancestors(path)).collect();
    all.iter()
        .filter(|entry| {
            files.contains(entry.path.as_str()) || (entry.is_dir && dirs.contains(entry.path.as_str()))
        })
        .cloned()
        .collect()
}

/// A row per entry, in order. With `fold`, a directory whose only content
/// is one directory is shown with it as one row, `a/b/c`, and what is in
/// the innermost follows at the row's depth plus one. Only the pruned
/// tree is folded: in the whole worktree such runs are rare and a reader
/// expects one row per directory.
pub fn rows(entries: &[TreeEntry], fold: bool) -> Vec<Row> {
    // The directories folded into their one child.
    let folded: HashSet<&str> = if fold {
        let mut children: HashMap<&str, (usize, bool)> = HashMap::new();
        for entry in entries {
            if let Some(parent) = parent(&entry.path) {
                let slot = children.entry(parent).or_insert((0, false));
                slot.0 += 1;
                slot.1 = entry.is_dir;
            }
        }
        entries
            .iter()
            .filter(|e| e.is_dir && matches!(children.get(e.path.as_str()), Some((1, true))))
            .map(|e| e.path.as_str())
            .collect()
    } else {
        HashSet::new()
    };

    entries
        .iter()
        .filter(|entry| !folded.contains(entry.path.as_str()))
        .map(|entry| {
            let mut head = entry.path.as_str();
            while let Some(above) = parent(head) {
                if folded.contains(above) {
                    head = above;
                } else {
                    break;
                }
            }
            let label = match parent(head) {
                Some(above) => entry.path[above.len() + 1..].to_string(),
                None => entry.path.clone(),
            };
            let removed = ancestors(&entry.path).filter(|dir| folded.contains(dir)).count();
            Row {
                path: entry.path.clone(),
                head: head.to_string(),
                label,
                is_dir: entry.is_dir,
                depth: entry.depth - removed,
            }
        })
        .collect()
}

/// Rows whose every ancestor directory is expanded.
///
/// Order is preserved, so a directory is still immediately followed by its
/// own contents.
pub fn visible(rows: &[Row], expanded: &HashSet<String>) -> Vec<Row> {
    rows.iter()
        .filter(|row| ancestors(&row.head).all(|dir| expanded.contains(dir)))
        .cloned()
        .collect()
}

/// Whether a directory row shows its contents: every directory it stands
/// for is expanded.
pub fn is_expanded(row: &Row, expanded: &HashSet<String>) -> bool {
    row.is_dir && row.directories().iter().all(|dir| expanded.contains(*dir))
}

/// Expand or collapse a directory row. A folded run opens as one, and
/// closes at its innermost directory, which is the one with contents.
pub fn toggle_row(expanded: &mut HashSet<String>, row: &Row) {
    if is_expanded(row, expanded) {
        toggle(expanded, &row.path);
    } else {
        for dir in row.directories() {
            expanded.insert(dir.to_string());
        }
    }
}

/// Every directory above `path`, from the top down.
///
/// `a/b/c.rs` yields `a` then `a/b`. A top-level entry yields nothing, so it
/// is always visible.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(i, _)| &path[..i])
}

fn parent(path: &str) -> Option<&str> {
    path.rfind('/').map(|i| &path[..i])
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
        TreeEntry::new(path, is_dir)
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

    fn row_paths(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.path.as_str()).collect()
    }

    fn expanded(dirs: &[&str]) -> HashSet<String> {
        dirs.iter().map(|d| d.to_string()).collect()
    }

    fn shown(entries: &[TreeEntry], expanded: &HashSet<String>) -> Vec<Row> {
        visible(&rows(entries, false), expanded)
    }

    #[test]
    fn nothing_expanded_shows_only_the_top_level() {
        let visible = shown(&fixture(), &HashSet::new());
        assert_eq!(row_paths(&visible), ["src", "docs", "README.md"]);
    }

    #[test]
    fn expanding_a_directory_reveals_its_immediate_contents_only() {
        let visible = shown(&fixture(), &expanded(&["src"]));
        assert_eq!(
            row_paths(&visible),
            ["src", "src/ui", "src/main.rs", "docs", "README.md"],
            "src/ui's contents stay hidden until it is expanded too"
        );
    }

    #[test]
    fn a_nested_directory_needs_every_ancestor_expanded() {
        // Expanding only the inner one must not leak it into view.
        assert_eq!(
            row_paths(&shown(&fixture(), &expanded(&["src/ui"]))),
            ["src", "docs", "README.md"]
        );

        assert_eq!(
            row_paths(&shown(&fixture(), &expanded(&["src", "src/ui"]))),
            ["src", "src/ui", "src/ui/row.rs", "src/ui/mod.rs", "src/main.rs", "docs", "README.md"]
        );
    }

    #[test]
    fn order_is_preserved_so_contents_follow_their_directory() {
        let visible = shown(&fixture(), &expanded(&["src", "src/ui"]));
        let ui = visible.iter().position(|e| e.path == "src/ui").unwrap();
        let row = visible.iter().position(|e| e.path == "src/ui/row.rs").unwrap();
        assert_eq!(row, ui + 1);
    }

    #[test]
    fn a_plain_row_is_its_entry() {
        let rows = rows(&fixture(), false);
        let row = rows.iter().find(|r| r.path == "src/ui/row.rs").unwrap();
        assert_eq!(row.label, "row.rs");
        assert_eq!(row.head, "src/ui/row.rs");
        assert_eq!(row.depth, 2);
        assert!(!row.is_dir);
        assert!(row.directories().is_empty(), "a file stands for no directory");

        let dir = rows.iter().find(|r| r.path == "src/ui").unwrap();
        assert_eq!(dir.label, "ui");
        assert_eq!(dir.directories(), ["src/ui"]);
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
        let mut expanded = expanded(&["src", "src/ui"]);
        toggle(&mut expanded, "src");
        assert!(
            !expanded.contains("src/ui"),
            "reopening src should show its own contents, not a tree from before"
        );
    }

    #[test]
    fn collapsing_leaves_unrelated_directories_alone() {
        let mut expanded = expanded(&["src", "src/ui", "docs"]);
        toggle(&mut expanded, "src");
        assert!(expanded.contains("docs"));
    }

    #[test]
    fn a_prefix_match_is_not_a_child() {
        // "src2" is not inside "src" and must survive collapsing it.
        let mut expanded = expanded(&["src", "src2"]);
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

        let visible = shown(&fixture(), &expanded);
        assert!(row_paths(&visible).contains(&"src/ui/row.rs"));
    }

    #[test]
    fn revealing_a_top_level_file_expands_nothing() {
        let mut expanded = HashSet::new();
        reveal(&mut expanded, "README.md");
        assert!(expanded.is_empty());
    }

    // The filter.

    #[test]
    fn the_filter_keeps_changed_files_and_the_way_to_them() {
        let pruned = changed_only(&fixture(), ["src/ui/row.rs", "README.md"]);
        assert_eq!(paths(&pruned), ["src", "src/ui", "src/ui/row.rs", "README.md"]);
    }

    #[test]
    fn the_filter_drops_a_directory_that_only_shares_a_prefix() {
        let all = vec![entry("src", true), entry("src/a.rs", false), entry("src2", true), entry("src2/b.rs", false)];
        let pruned = changed_only(&all, ["src2/b.rs"]);
        assert_eq!(paths(&pruned), ["src2", "src2/b.rs"], "src is not on the way to src2/b.rs");
    }

    #[test]
    fn the_filter_with_nothing_changed_is_empty() {
        assert!(changed_only(&fixture(), []).is_empty());
    }

    // Deleted files.

    #[test]
    fn a_path_the_worktree_lacks_is_added_with_its_directories() {
        let merged = with_paths(&fixture(), ["src/gone.rs", "lib/old/thing.rs"]).expect("two were missing");
        assert_eq!(
            paths(&merged),
            [
                "lib",
                "lib/old",
                "lib/old/thing.rs",
                "src",
                "src/ui",
                "src/ui/row.rs",
                "src/ui/mod.rs",
                "src/gone.rs",
                "src/main.rs",
                "docs",
                "docs/guide.md",
                "README.md",
            ],
            "each lands where the listing would have put it"
        );
        let old = merged.iter().find(|e| e.path == "lib/old").unwrap();
        assert!(old.is_dir);
        assert_eq!(old.depth, 1);
    }

    #[test]
    fn paths_already_in_the_tree_change_nothing() {
        assert_eq!(with_paths(&fixture(), ["src/main.rs", "README.md"]), None);
        assert_eq!(with_paths(&fixture(), []), None);
    }

    #[test]
    fn a_changed_path_the_worktree_has_as_a_directory_stays_a_directory() {
        // A submodule: the change is at `vendor/lib`, which the listing
        // walked into.
        let all = vec![entry("vendor", true), entry("vendor/lib", true), entry("vendor/lib/lib.c", false)];
        assert_eq!(with_paths(&all, ["vendor/lib"]), None, "nothing to add");
        let pruned = changed_only(&all, ["vendor/lib"]);
        assert_eq!(paths(&pruned), ["vendor", "vendor/lib"]);
        assert!(pruned[1].is_dir);
        // Folded into one row, as a lone run is.
        let rows = rows(&pruned, true);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].label.as_str(), rows[0].path.as_str()), ("vendor/lib", "vendor/lib"));
    }

    #[test]
    fn a_directory_the_worktree_has_as_a_file_is_added_above_its_contents() {
        // `config/` became the file `config`: the file is in the worktree
        // and the old contents are deleted.
        let all = vec![
            entry("crates", true),
            entry("crates/app", true),
            entry("crates/app/main.rs", false),
            entry("Cargo.toml", false),
            entry("config", false),
            entry("README.md", false),
        ];
        let merged = with_paths(&all, ["config", "config/default.toml"]).expect("the old contents are missing");
        assert_eq!(
            paths(&merged),
            ["config", "config/default.toml", "crates", "crates/app", "crates/app/main.rs", "Cargo.toml", "config", "README.md"],
            "the directory sits among the directories, the file among the files"
        );
        assert!(merged[0].is_dir);
        assert!(!merged[6].is_dir);
        let pruned = changed_only(&merged, ["config", "config/default.toml"]);
        assert_eq!(paths(&pruned), ["config", "config/default.toml", "config"]);
        let rows = rows(&pruned, true);
        assert_eq!(
            rows.iter().map(|r| (r.label.as_str(), r.depth, r.is_dir)).collect::<Vec<_>>(),
            [("config", 0, true), ("default.toml", 1, false), ("config", 0, false)]
        );
    }

    #[test]
    fn two_missing_files_in_one_missing_directory_share_it() {
        let merged = with_paths(&fixture(), ["gone/a.rs", "gone/b.rs"]).unwrap();
        assert_eq!(merged.iter().filter(|e| e.path == "gone").count(), 1);
    }

    // Folding.

    fn deep() -> Vec<TreeEntry> {
        vec![
            entry("crates", true),
            entry("crates/app", true),
            entry("crates/app/src", true),
            entry("crates/app/src/review", true),
            entry("crates/app/src/review/rail.rs", false),
            entry("crates/git", true),
            entry("crates/git/src", true),
            entry("crates/git/src/tree.rs", false),
            entry("crates/git/src/diff.rs", false),
            entry("README.md", false),
        ]
    }

    #[test]
    fn a_run_of_lone_directories_folds_into_one_row() {
        let rows = rows(&deep(), true);
        let labels: Vec<(&str, usize)> = rows.iter().map(|r| (r.label.as_str(), r.depth)).collect();
        assert_eq!(
            labels,
            [
                ("crates", 0),
                ("app/src/review", 1),
                ("rail.rs", 2),
                ("git/src", 1),
                ("tree.rs", 2),
                ("diff.rs", 2),
                ("README.md", 0),
            ],
            "crates has two children so it stands alone; each child's run folds"
        );
        let review = &rows[1];
        assert_eq!(review.path, "crates/app/src/review");
        assert_eq!(review.head, "crates/app");
        assert!(review.is_dir);
        assert_eq!(review.directories(), ["crates/app", "crates/app/src", "crates/app/src/review"]);
    }

    #[test]
    fn a_directory_with_a_file_in_it_does_not_fold() {
        // src holds a file beside its one directory, so it is a row of its
        // own, and ui folds nowhere since it is the only directory but not
        // the only thing.
        let rows = rows(&fixture(), true);
        assert_eq!(
            row_paths(&rows),
            ["src", "src/ui", "src/ui/row.rs", "src/ui/mod.rs", "src/main.rs", "docs", "docs/guide.md", "README.md"]
        );
    }

    #[test]
    fn a_run_from_the_top_folds_to_depth_zero() {
        let all = vec![entry("a", true), entry("a/b", true), entry("a/b/c", true), entry("a/b/c/x.rs", false)];
        let rows = rows(&all, true);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].label.as_str(), rows[0].depth, rows[0].head.as_str()), ("a/b/c", 0, "a"));
        assert_eq!((rows[1].label.as_str(), rows[1].depth), ("x.rs", 1));
    }

    #[test]
    fn a_folded_row_is_visible_by_its_outermost_directory() {
        let all = vec![entry("a", true), entry("a/b", true), entry("a/b/c", true), entry("a/b/c/x.rs", false)];
        let rows = rows(&all, true);
        // Nothing expanded: the folded row is top level, so it shows, shut.
        let shown = visible(&rows, &HashSet::new());
        assert_eq!(row_paths(&shown), ["a/b/c"]);
        assert!(!is_expanded(&rows[0], &HashSet::new()));
        // Its contents need every directory of the run open.
        assert_eq!(row_paths(&visible(&rows, &expanded(&["a", "a/b"]))), ["a/b/c"]);
        assert_eq!(row_paths(&visible(&rows, &expanded(&["a", "a/b", "a/b/c"]))), ["a/b/c", "a/b/c/x.rs"]);
    }

    #[test]
    fn toggling_a_folded_row_opens_the_run_and_shuts_its_innermost() {
        let all = vec![entry("a", true), entry("a/b", true), entry("a/b/c", true), entry("a/b/c/x.rs", false)];
        let rows = rows(&all, true);
        let mut expanded = HashSet::new();
        toggle_row(&mut expanded, &rows[0]);
        assert_eq!(expanded, self::expanded(&["a", "a/b", "a/b/c"]));
        assert!(is_expanded(&rows[0], &expanded));

        toggle_row(&mut expanded, &rows[0]);
        assert_eq!(expanded, self::expanded(&["a", "a/b"]), "the outer ones stay open for the whole tree");
        assert!(!is_expanded(&rows[0], &expanded));
        assert_eq!(row_paths(&visible(&rows, &expanded)), ["a/b/c"]);
    }

    #[test]
    fn toggling_a_plain_row_is_a_toggle() {
        let rows = rows(&fixture(), false);
        let src = rows.iter().find(|r| r.path == "src").unwrap();
        let mut expanded = HashSet::new();
        toggle_row(&mut expanded, src);
        assert_eq!(expanded, self::expanded(&["src"]));
        toggle_row(&mut expanded, src);
        assert!(expanded.is_empty());
    }

    #[test]
    fn a_file_row_is_never_expanded() {
        let rows = rows(&fixture(), false);
        let file = rows.iter().find(|r| r.path == "README.md").unwrap();
        assert!(!is_expanded(file, &expanded(&["README.md"])));
    }

    #[test]
    fn the_filtered_tree_end_to_end() {
        // A branch that touched two files, one of them since deleted.
        let tree = fixture();
        let changed = ["src/ui/row.rs", "src/gone.rs"];
        let tree = with_paths(&tree, changed).unwrap();
        let rows = rows(&changed_only(&tree, changed), true);
        assert_eq!(
            rows.iter().map(|r| (r.label.as_str(), r.depth)).collect::<Vec<_>>(),
            [("src", 0), ("ui", 1), ("row.rs", 2), ("gone.rs", 1)]
        );
    }
}
