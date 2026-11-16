//! A Work run may not make or change a Git repository inside its worktree:
//! a nested `.git` can hold hooks and config that the next `git` run there
//! obeys. Its sandbox settings deny writes to `<worktree>/*/**/.git`, but
//! Claude Code on Linux drops glob patterns from `denyWrite`, so there the
//! rule is not kept. The engine looks instead: the `.git`s below the
//! worktree's top before the run and after it, with what in each one
//! decides what git runs, and one that appeared or changed fails the run's
//! record, with its path named.
//!
//! The run may have left anything there, so the look never follows a link,
//! never opens what is not a plain file (a FIFO would block it for good,
//! a link to `/dev/zero` would fill memory), keeps no more than
//! [`KEPT_BYTES`] of a file, and goes through at most [`MOST_DIRS`]
//! directories.

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

/// The most directories looked through: a run that made millions would
/// otherwise hold up its own end, and the engine thread doing the look.
pub(crate) const MOST_DIRS: usize = 200_000;

/// The most of a file kept to compare. A longer one is known by its size
/// and its inode's change time, which a write always moves and nothing a
/// run can do sets back.
const KEPT_BYTES: u64 = 256 * 1024;

/// The files in a repository's `.git`, besides `config` and its hooks,
/// that change what git runs there: `commondir` takes config and hooks
/// from another directory, `config.worktree` adds to the config, and
/// `gitdir` is a linked worktree's way back.
const DECIDING: [&str; 3] = ["commondir", "config.worktree", "gitdir"];

/// What a `.git` holds that decides what git does there: a repository's
/// `config` and hooks, or where a `.git` file or link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    Repository(BTreeMap<String, Vec<u8>>),
    Pointer(Vec<u8>),
}

/// The `.git`s below a worktree's top, and what each holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Nested {
    pub(crate) gits: BTreeMap<PathBuf, Held>,
    /// False when the look stopped at [`MOST_DIRS`] with more to go.
    pub(crate) whole: bool,
}

/// Every `.git` below `top`, as a directory or as the file a worktree or a
/// submodule has, and not `top`'s own. A `.git` is not looked inside past
/// what [`Held`] keeps, and links are not followed: what a link points at
/// is not in the worktree.
pub(crate) fn below(top: &Path) -> Nested {
    below_within(top, MOST_DIRS)
}

fn below_within(top: &Path, most_dirs: usize) -> Nested {
    let mut found = Nested {
        gits: BTreeMap::new(),
        whole: true,
    };
    let mut dirs = vec![top.to_path_buf()];
    let mut looked = 0;
    while let Some(dir) = dirs.pop() {
        if looked == most_dirs {
            found.whole = false;
            break;
        }
        looked += 1;
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
                    } else {
                        Held::Pointer(file(&path).unwrap_or_default())
                    };
                    found.gits.insert(path, held);
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

/// A repository's `config`, the other files that decide what git runs
/// there, and every hook, by name. `hooks` as a link is kept as one.
fn repository(git: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut held = BTreeMap::new();
    for name in std::iter::once("config").chain(DECIDING) {
        if let Some(kept) = file(&git.join(name)) {
            held.insert(name.to_string(), kept);
        }
    }
    let hooks = git.join("hooks");
    match std::fs::symlink_metadata(&hooks) {
        Ok(meta) if meta.is_dir() => {
            if let Ok(entries) = std::fs::read_dir(&hooks) {
                for hook in entries.flatten() {
                    let name = format!("hooks/{}", hook.file_name().to_string_lossy());
                    held.insert(name, file(&hook.path()).unwrap_or_default());
                }
            }
        }
        Ok(_) => {
            held.insert("hooks".to_string(), file(&hooks).unwrap_or_default());
        }
        Err(_) => {}
    }
    held
}

/// What stands at `path`, to compare, without following it if it is a
/// link or opening it if it is not a plain file: a link's target, a plain
/// file's bytes up to [`KEPT_BYTES`], or for a longer file, or a FIFO,
/// socket or device, what kind it is and its inode. None when nothing is
/// there.
fn file(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        let mut kept = b"link:".to_vec();
        if let Ok(to) = std::fs::read_link(path) {
            kept.extend(to.into_os_string().into_encoded_bytes());
        }
        return Some(kept);
    }
    if kind.is_file() && meta.len() <= KEPT_BYTES {
        if let Some(bytes) = read_plain(path) {
            return Some(bytes);
        }
    }
    Some(stamp(&meta).into_bytes())
}

/// What a file is known by when it is not read.
#[cfg(unix)]
fn stamp(meta: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!(
        "not read: mode {:o}, inode {}, {} bytes, changed {}.{}",
        meta.mode(),
        meta.ino(),
        meta.len(),
        meta.ctime(),
        meta.ctime_nsec()
    )
}

#[cfg(not(unix))]
fn stamp(meta: &std::fs::Metadata) -> String {
    format!(
        "not read: {:?}, {} bytes, modified {:?}",
        meta.file_type(),
        meta.len(),
        meta.modified().ok()
    )
}

/// A plain file's bytes, up to [`KEPT_BYTES`]; None when what is there is
/// not a plain file by the time it is opened, or is longer. Opened without
/// following a link or waiting on a FIFO, as either could have been put
/// there since it was looked at.
fn read_plain(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let opened = options.open(path).ok()?;
    if !opened.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    opened.take(KEPT_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= KEPT_BYTES).then_some(bytes)
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
    for (path, held) in &after.gits {
        match before.gits.get(path) {
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
    let Some(first) = said.next() else {
        // Past the most looked through, what was not looked at could hold
        // one. A worktree that was that big before is not the run's doing.
        return (before.whole && !after.whole).then(|| format!(
            "This Work run took the worktree past {MOST_DIRS} folders, more than Perch looks through for a Git repository made below its top. Claude Code's sandbox on Linux cannot stop a run making one, so look for any before running git there."
        ));
    };
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
            before.gits.keys().collect::<Vec<_>>(),
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
        assert!(below(top.path()).gits.is_empty());
    }

    /// What a run leaves in a nested `.git` is not followed or opened: a
    /// link to `/dev/zero` would fill memory and a FIFO would wait for a
    /// writer for good. Either is still a change.
    #[cfg(unix)]
    #[test]
    fn a_link_or_fifo_in_a_nested_git_is_named_without_being_opened() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("x/.git/hooks")).unwrap();
        std::fs::create_dir(top.join("y")).unwrap();
        let before = below(top);

        std::os::unix::fs::symlink("/dev/zero", top.join("x/.git/config")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", top.join("x/.git/hooks/pre-commit")).unwrap();
        fifo(&top.join("x/.git/hooks/post-checkout"));
        fifo(&top.join("y/.git"));
        let after = below(top);
        let said = appeared(top, &before, &after).unwrap();
        assert!(
            said.starts_with(
                "A Git repository appeared at y/.git, and the Git repository at x/.git was changed"
            ),
            "{said}"
        );
        let Some(Held::Repository(held)) = after.gits.get(&top.join("x/.git")) else {
            panic!("no repository");
        };
        assert_eq!(held["config"], b"link:/dev/zero");
    }

    #[cfg(unix)]
    fn fifo(path: &Path) {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }

    /// A long hook is known by its size and change time, not kept whole,
    /// and still seen to change.
    #[test]
    fn a_long_hook_is_not_kept_whole_but_is_seen_to_change() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("x/.git/hooks")).unwrap();
        let hook = top.join("x/.git/hooks/pre-commit");
        std::fs::write(&hook, vec![b'a'; KEPT_BYTES as usize + 1]).unwrap();
        let before = below(top);
        let Some(Held::Repository(held)) = before.gits.get(&top.join("x/.git")) else {
            panic!("no repository");
        };
        assert!(held["hooks/pre-commit"].len() < 200);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&hook, vec![b'b'; KEPT_BYTES as usize + 1]).unwrap();
        assert!(appeared(top, &before, &below(top)).is_some());
    }

    /// A `commondir` takes config and hooks from elsewhere: one written
    /// into a repository that was there is a change.
    #[test]
    fn a_commondir_written_into_a_nested_git_is_named() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("vendor/old/.git/hooks")).unwrap();
        let before = below(top);
        std::fs::write(top.join("vendor/old/.git/commondir"), "../../../evil\n").unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("The Git repository at vendor/old/.git was changed"),
            "{said}"
        );
    }

    /// The look stops at its most folders; a run that took the worktree
    /// past it is said to have, and one that was past it before is not.
    #[test]
    fn a_worktree_grown_past_the_most_looked_through_is_named() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("a")).unwrap();
        let before = below_within(top, 3);
        assert!(before.whole);
        std::fs::create_dir_all(top.join("b/c/d")).unwrap();
        let after = below_within(top, 3);
        assert!(!after.whole);
        let said = appeared(top, &before, &after).unwrap();
        assert!(
            said.starts_with("This Work run took the worktree past"),
            "{said}"
        );
        assert_eq!(appeared(top, &after, &after), None);
    }
}
