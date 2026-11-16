//! A Work run may not make or change a Git repository inside its worktree:
//! a nested `.git` can hold hooks and config that the next `git` run there
//! obeys. Its sandbox settings deny writes to `<worktree>/*/**/.git`, but
//! Claude Code on Linux drops glob patterns from `denyWrite`, so there the
//! rule is not kept. The engine looks instead: the `.git`s below the
//! worktree's top before the run and after it, with what in each one
//! decides what git runs, and one that appeared or changed fails the run's
//! record, with its path named.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Whether runs are looked at this way: only where the sandbox cannot
/// keep the rule itself, and in this crate's tests, so the engine's side
/// of it is tried on every machine.
pub(crate) const WATCHED: bool = cfg!(any(target_os = "linux", test));

/// Directories not looked inside: installed dependencies and build output,
/// which can hold many thousands of files and repositories of their own
/// (`pip install -e git+…`, a test suite's fixtures), and where nobody runs
/// git. Looking through them would hold up every Work run's start.
const PASSED_OVER: [&str; 4] = ["node_modules", "target", ".venv", "venv"];

/// What a `.git` holds that decides what git does there: a repository's
/// `config` and hooks, or where a `.git` file or link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    Repository(BTreeMap<String, Vec<u8>>),
    Pointer(Vec<u8>),
}

/// The `.git`s below a worktree's top, and what each holds.
pub(crate) type Nested = BTreeMap<PathBuf, Held>;

/// Every `.git` below `top`, as a directory or as the file a worktree or a
/// submodule has, and not `top`'s own. A `.git` is not looked inside past
/// what [`Held`] keeps, and links are not followed: what a link points at
/// is not in the worktree.
pub(crate) fn below(top: &Path) -> Nested {
    let mut found = Nested::new();
    let mut dirs = vec![top.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if name == ".git" {
                if dir != top {
                    let path = entry.path();
                    let held = if kind.is_dir() {
                        Held::Repository(repository(&path))
                    } else if kind.is_symlink() {
                        Held::Pointer(
                            std::fs::read_link(&path)
                                .map(|to| to.into_os_string().into_encoded_bytes())
                                .unwrap_or_default(),
                        )
                    } else {
                        Held::Pointer(std::fs::read(&path).unwrap_or_default())
                    };
                    found.insert(path, held);
                }
                continue;
            }
            if kind.is_dir() && !PASSED_OVER.iter().any(|over| name == *over) {
                dirs.push(entry.path());
            }
        }
    }
    found
}

/// A repository's `config` and every hook, by name.
fn repository(git: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut held = BTreeMap::new();
    if let Ok(config) = std::fs::read(git.join("config")) {
        held.insert("config".to_string(), config);
    }
    if let Ok(hooks) = std::fs::read_dir(git.join("hooks")) {
        for hook in hooks.flatten() {
            let name = format!("hooks/{}", hook.file_name().to_string_lossy());
            held.insert(name, std::fs::read(hook.path()).unwrap_or_default());
        }
    }
    held
}

/// The submodules `.gitmodules` at `top` declares, by the `.git` file each
/// has once checked out. `git submodule update` makes those, and they point
/// into the top's own repository, which the sandbox keeps.
fn submodules(top: &Path) -> BTreeSet<PathBuf> {
    let Ok(text) = std::fs::read_to_string(top.join(".gitmodules")) else {
        return BTreeSet::new();
    };
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "path").then(|| top.join(value.trim()).join(".git"))
        })
        .collect()
}

/// The `.git`s in `after` that were not in `before`, or hold something
/// else, and what the run's record says about them: None when there are
/// none.
pub(crate) fn appeared(top: &Path, before: &Nested, after: &Nested) -> Option<String> {
    let named = |path: &Path| path.strip_prefix(top).unwrap_or(path).display().to_string();
    let submodules = submodules(top);
    let mut new = Vec::new();
    let mut changed = Vec::new();
    for (path, held) in after {
        match before.get(path) {
            None if matches!(held, Held::Pointer(_)) && submodules.contains(path) => {}
            None => new.push(named(path)),
            Some(was) if was != held => changed.push(named(path)),
            Some(_) => {}
        }
    }
    let mut said = Vec::new();
    match new.as_slice() {
        [] => {}
        [one] => said.push(format!("A Git repository appeared at {one}")),
        many => said.push(format!("Git repositories appeared at {}", many.join(", "))),
    }
    match changed.as_slice() {
        [] => {}
        [one] => said.push(format!("the Git repository at {one} was changed")),
        many => said.push(format!(
            "the Git repositories at {} were changed",
            many.join(", ")
        )),
    }
    let said = said.join(", and ");
    let mut said = said.chars();
    let first = said.next()?;
    Some(format!(
        "{}{}, below the worktree's top, during this Work run. Claude Code's sandbox on Linux cannot stop a run doing that, so look at what it holds before running git there.",
        first.to_uppercase(),
        said.as_str()
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
            before.keys().collect::<Vec<_>>(),
            [&top.join("vendor/old/.git")],
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

    /// A hook or config written into a repository that was there before
    /// is what a nested `.git` is a danger for, and is named as much as
    /// one made new.
    #[test]
    fn a_hook_or_config_written_into_a_nested_git_is_named() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("vendor/old/.git/hooks")).unwrap();
        std::fs::write(top.join("vendor/old/.git/config"), "[core]\n").unwrap();
        std::fs::create_dir(top.join("linked")).unwrap();
        std::fs::write(top.join("linked/.git"), "gitdir: /there\n").unwrap();
        let before = below(top);

        std::fs::write(
            top.join("vendor/old/.git/hooks/post-checkout"),
            "#!/bin/sh\n",
        )
        .unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("The Git repository at vendor/old/.git was changed, below"),
            "{said}"
        );

        std::fs::remove_file(top.join("vendor/old/.git/hooks/post-checkout")).unwrap();
        assert_eq!(appeared(top, &before, &below(top)), None);
        std::fs::write(
            top.join("vendor/old/.git/config"),
            "[core]\n\thooksPath = /tmp\n",
        )
        .unwrap();
        std::fs::write(top.join("linked/.git"), "gitdir: /elsewhere\n").unwrap();
        std::fs::create_dir_all(top.join("new/.git")).unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with(
                "A Git repository appeared at new/.git, and the Git repositories at linked/.git, vendor/old/.git were changed, below"
            ),
            "{said}"
        );
    }

    /// `git submodule update` checks out what `.gitmodules` names, each
    /// with a `.git` file, and dependencies and build output are not
    /// looked through.
    #[test]
    fn submodules_dependencies_and_build_output_are_not_the_run_s() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::write(
            top.join(".gitmodules"),
            "[submodule \"lib\"]\n\tpath = vendor/lib\n\turl = https://example.com/lib\n",
        )
        .unwrap();
        let before = below(top);
        std::fs::create_dir_all(top.join("vendor/lib")).unwrap();
        std::fs::write(
            top.join("vendor/lib/.git"),
            "gitdir: ../../.git/modules/lib\n",
        )
        .unwrap();
        for dir in ["node_modules/x", "target/debug/fixture", ".venv/src/pkg"] {
            std::fs::create_dir_all(top.join(dir).join(".git")).unwrap();
        }
        assert_eq!(appeared(top, &before, &below(top)), None);

        // A repository made where a submodule goes is still named.
        std::fs::remove_file(top.join("vendor/lib/.git")).unwrap();
        std::fs::create_dir(top.join("vendor/lib/.git")).unwrap();
        assert!(appeared(top, &before, &below(top)).is_some());
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
