//! A Work run may not make a Git repository inside its worktree: a nested
//! `.git` can hold hooks and config that the next `git` run there obeys.
//! Its sandbox settings deny writes to `<worktree>/*/**/.git`, but Claude
//! Code on Linux drops glob patterns from `denyWrite`, so there the rule is
//! not kept. The engine looks instead: the `.git`s below the worktree's top
//! before the run and after it, and one that appeared fails the run's
//! record, with its path named.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Whether runs are looked at this way: only where the sandbox cannot
/// keep the rule itself, and in this crate's tests, so the engine's side
/// of it is tried on every machine.
pub(crate) const WATCHED: bool = cfg!(any(target_os = "linux", test));

/// Every `.git` below `top`, as a directory or as the file a worktree or a
/// submodule has, and not `top`'s own. A `.git` is not looked inside, and
/// links are not followed: what a link points at is not in the worktree.
pub(crate) fn below(top: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut dirs = vec![top.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if entry.file_name() == ".git" {
                if dir != top {
                    found.insert(entry.path());
                }
                continue;
            }
            if kind.is_dir() {
                dirs.push(entry.path());
            }
        }
    }
    found
}

/// The `.git`s in `after` that were not in `before`, and what the run's
/// record says about them: None when there are none.
pub(crate) fn appeared(
    top: &Path,
    before: &BTreeSet<PathBuf>,
    after: &BTreeSet<PathBuf>,
) -> Option<String> {
    let new: Vec<String> = after
        .difference(before)
        .map(|path| path.strip_prefix(top).unwrap_or(path).display().to_string())
        .collect();
    let named = match new.as_slice() {
        [] => return None,
        [one] => format!("A Git repository appeared at {one}"),
        many => format!("Git repositories appeared at {}", many.join(", ")),
    };
    Some(format!(
        "{named}, below the worktree's top, during this Work run. Claude Code's sandbox on Linux cannot stop a run making one, so look at what it holds before running git there."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_that_appeared_below_the_top_is_named() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir(top.join(".git")).unwrap();
        std::fs::create_dir_all(top.join("vendor/old/.git/objects")).unwrap();
        std::fs::create_dir_all(top.join("src")).unwrap();
        let before = below(top);
        assert_eq!(
            before,
            BTreeSet::from([top.join("vendor/old/.git")]),
            "the top's own .git is not counted, nor a .git's insides"
        );
        assert_eq!(appeared(top, &before, &below(top)), None);

        std::fs::create_dir_all(top.join("src/inner/.git")).unwrap();
        std::fs::write(top.join("sub.git-file-holder"), "").unwrap();
        std::fs::create_dir(top.join("module")).unwrap();
        std::fs::write(top.join("module/.git"), "gitdir: /elsewhere\n").unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("Git repositories appeared at module/.git, src/inner/.git,"),
            "{said}"
        );

        std::fs::remove_dir_all(top.join("module")).unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("A Git repository appeared at src/inner/.git, below"),
            "{said}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_worktree_is_not_followed() {
        let top = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elsewhere.path().join("repo/.git")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), top.path().join("link")).unwrap();
        assert!(below(top.path()).is_empty());
    }
}
