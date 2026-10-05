//! Listing the files in a worktree, for the file rail.

use std::path::Path;

use crate::Error;

/// One entry of the worktree tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Path relative to the worktree root, with `/` separators.
    pub path: String,
    pub is_dir: bool,
    /// How deep the entry sits, for indenting without re-splitting the path.
    pub depth: usize,
}

impl TreeEntry {
    pub fn new(path: impl Into<String>, is_dir: bool) -> Self {
        let path = path.into();
        let depth = path.matches('/').count();
        TreeEntry {
            path,
            is_dir,
            depth,
        }
    }
}

/// The order `list_tree` returns entries in, for putting one in among
/// them without listing again.
pub fn compare_entries(a: &TreeEntry, b: &TreeEntry) -> std::cmp::Ordering {
    compare_paths(&a.path, a.is_dir, &b.path, b.is_dir)
}

/// Walk a worktree, honouring `.gitignore` and skipping `.git`.
///
/// Returns entries sorted so a directory is immediately followed by its
/// contents, which is the order a collapsible tree wants.
pub fn list_tree(worktree: &Path, max_entries: usize) -> Result<Vec<TreeEntry>, Error> {
    let mut paths: Vec<(String, bool)> = Vec::new();

    let walker = ignore::WalkBuilder::new(worktree)
        .hidden(false)
        .git_ignore(true)
        // A worktree keeps its git metadata in a file rather than a
        // directory, and a plain directory has none at all. Without this the
        // walker silently stops honouring .gitignore.
        .require_git(false)
        .git_global(false)
        .git_exclude(true)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        let Ok(relative) = entry.path().strip_prefix(worktree) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        paths.push((relative.to_string_lossy().replace('\\', "/"), is_dir));

        if paths.len() >= max_entries {
            break;
        }
    }

    // Sort so that within a directory, subdirectories come before files and
    // each group is alphabetical. Comparing component-wise keeps a directory
    // adjacent to its own contents.
    paths.sort_by(|a, b| compare_paths(&a.0, a.1, &b.0, b.1));

    Ok(paths
        .into_iter()
        .map(|(path, is_dir)| TreeEntry::new(path, is_dir))
        .collect())
}

/// The order two paths take in the tree: a directory before a file
/// beside it, otherwise by name, component by component. `a_dir` and
/// `b_dir` say whether the last component is a directory; every earlier
/// one is.
pub fn compare_paths(a: &str, a_dir: bool, b: &str, b_dir: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut ap = a.split('/').peekable();
    let mut bp = b.split('/').peekable();

    loop {
        match (ap.next(), bp.next()) {
            (Some(x), Some(y)) => {
                let last_a = ap.peek().is_none();
                let last_b = bp.peek().is_none();
                if x == y && last_a == last_b {
                    continue;
                }
                if x != y {
                    // A directory outranks a file at the same level.
                    let x_is_dir = !last_a || a_dir;
                    let y_is_dir = !last_b || b_dir;
                    return match (x_is_dir, y_is_dir) {
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => x.cmp(y),
                    };
                }
                // Same name, but one path continues: the shorter is the
                // parent and must come first.
                return if last_a {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(mut paths: Vec<(&str, bool)>) -> Vec<String> {
        paths.sort_by(|a, b| compare_paths(a.0, a.1, b.0, b.1));
        paths.into_iter().map(|(p, _)| p.to_string()).collect()
    }

    #[test]
    fn directories_sort_before_files_at_the_same_level() {
        let got = order(vec![
            ("README.md", false),
            ("src", true),
            ("Cargo.toml", false),
        ]);
        assert_eq!(got, vec!["src", "Cargo.toml", "README.md"]);
    }

    #[test]
    fn a_directory_is_followed_by_its_own_contents() {
        let got = order(vec![
            ("src/main.rs", false),
            ("src", true),
            ("README.md", false),
            ("src/ui", true),
            ("src/ui/row.rs", false),
        ]);
        assert_eq!(
            got,
            vec!["src", "src/ui", "src/ui/row.rs", "src/main.rs", "README.md"]
        );
    }

    #[test]
    fn depth_comes_from_the_path() {
        assert_eq!(TreeEntry::new("a/b/c.rs", false).depth, 2);
        assert_eq!(TreeEntry::new("README.md", false).depth, 0);
    }

    #[test]
    fn entries_compare_in_listing_order() {
        use std::cmp::Ordering;
        let dir = TreeEntry::new("src", true);
        let file = TreeEntry::new("src/main.rs", false);
        let readme = TreeEntry::new("README.md", false);
        assert_eq!(
            compare_entries(&dir, &file),
            Ordering::Less,
            "a directory comes before its contents"
        );
        assert_eq!(
            compare_entries(&file, &readme),
            Ordering::Less,
            "a directory's contents come before a file beside it"
        );
        assert_eq!(compare_entries(&readme, &readme), Ordering::Equal);
    }

    #[test]
    fn listing_a_real_directory_skips_the_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi").unwrap();

        let entries = list_tree(dir.path(), 1000).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

        assert!(paths.contains(&"src"));
        assert!(paths.contains(&"src/main.rs"));
        assert!(paths.contains(&"README.md"));
        assert!(
            !paths.iter().any(|p| p.starts_with(".git")),
            "the git directory must not appear in the tree: {paths:?}"
        );
    }

    #[test]
    fn listing_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target\n").unwrap();
        std::fs::write(dir.path().join("target/big.o"), "x").unwrap();
        std::fs::write(dir.path().join("keep.rs"), "x").unwrap();

        let entries = list_tree(dir.path(), 1000).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"keep.rs"));
        assert!(!paths.iter().any(|p| p.starts_with("target")), "{paths:?}");
    }

    #[test]
    fn listing_stops_at_the_entry_cap() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "x").unwrap();
        }
        let entries = list_tree(dir.path(), 10).unwrap();
        assert_eq!(entries.len(), 10);
    }
}
