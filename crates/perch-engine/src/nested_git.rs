//! A Work run may not make or change a Git repository inside its worktree:
//! a nested `.git` can hold hooks and config that the next `git` run there
//! obeys. Its sandbox settings deny writes to `<worktree>/*/**/.git`, but
//! Claude Code on Linux drops glob patterns from `denyWrite`, and on any
//! platform a run can move in a directory that already holds a `.git`. The
//! engine looks as well: the `.git`s below the worktree's top as the run
//! starts, with what in each one decides what git runs. One that appeared
//! or changed since keeps a grant from answering a git write outside the
//! sandbox while the run goes ([`unsettled`]), and fails the run's record
//! afterwards, with its path named ([`appeared`]).
//!
//! The run may have left anything there, so the look follows no link but
//! a `.git` one (and a `.git` file's `gitdir:`) to the repository git would
//! use, and there only to what [`Held`] keeps, never opens what is not a
//! plain file (a FIFO would block it for good,
//! a link to `/dev/zero` would fill memory), reads no more than
//! [`KEPT_BYTES`] of a file and keeps only its digest, and goes through at
//! most [`MOST_LOOKED`] directories and hooks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Directories not looked inside: installed dependencies and build output,
/// which can hold many thousands of files and repositories of their own
/// (`pip install -e git+…`, a test suite's fixtures), and where nobody runs
/// git. Looking through them would hold up every Work run's start.
const PASSED_OVER: [&str; 4] = ["node_modules", "target", ".venv", "venv"];

/// The most directories, and hooks in nested repositories, looked at: a
/// run that made millions would otherwise hold up its own end, and the
/// engine thread doing the look, and fill its memory with what is kept of
/// each.
pub(crate) const MOST_LOOKED: usize = 200_000;

/// The most of a file read to compare. A longer one is known by its size
/// and its inode's change time, which a write always moves and nothing a
/// run can do sets back.
const KEPT_BYTES: u64 = 256 * 1024;

/// What is kept of a file to compare: a SHA-256 of what it is and holds,
/// so many hooks, or many links to one long file, cost little to keep.
pub(crate) type Kept = [u8; 32];

/// The files in a repository's `.git`, besides `config` and its hooks,
/// that change what git runs there: `commondir` takes config and hooks
/// from another directory, `config.worktree` adds to the config, and
/// `gitdir` is a linked worktree's way back.
const DECIDING: [&str; 3] = ["commondir", "config.worktree", "gitdir"];

/// What a `.git` holds that decides what git does there: a repository's
/// `config` and hooks, or where a `.git` file or link points and what the
/// repository there holds, as the run could write that too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    Repository(BTreeMap<String, Kept>),
    Pointer(Kept, Option<BTreeMap<String, Kept>>),
}

/// The `.git`s below a worktree's top, and what each holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Nested {
    pub(crate) gits: BTreeMap<PathBuf, Held>,
    /// False when the look stopped at [`MOST_LOOKED`] with more to go.
    pub(crate) whole: bool,
    /// The submodules `.gitmodules` declared when this was taken (see
    /// [`submodules`]). Only the look before the run is trusted with it:
    /// the run can write `.gitmodules`.
    submodules: BTreeSet<PathBuf>,
    /// Where the top's submodules keep their git directories: `modules/`
    /// in each git directory the top uses, which the sandbox keeps
    /// read-only, made real.
    modules: Vec<PathBuf>,
}

impl Nested {
    /// Whether a `.git` that was not there before is a submodule checked
    /// out by `git submodule update`: a `.git` file where `.gitmodules`
    /// said one goes before the run, pointing into a `modules/` the run
    /// could not write.
    fn checked_out(&self, path: &Path) -> bool {
        let Some(to) = pointed(path).and_then(|to| to.canonicalize().ok()) else {
            return false;
        };
        self.submodules.contains(path)
            && self
                .modules
                .iter()
                .any(|modules| to != *modules && to.starts_with(modules))
    }
}

/// Every `.git` below `top`, as a directory or as the file a worktree or a
/// submodule has, and not `top`'s own. A `.git` is not looked inside past
/// what [`Held`] keeps, and links to folders are not followed: what a
/// link points at is not in the worktree. A `.git` link or file is
/// followed to its repository, as git would.
pub(crate) fn below(top: &Path) -> Nested {
    below_within(top, MOST_LOOKED)
}

fn below_within(top: &Path, most_looked: usize) -> Nested {
    let mut found = Nested {
        gits: BTreeMap::new(),
        whole: true,
        submodules: submodules(top),
        modules: modules(top),
    };
    let mut dirs = vec![top.to_path_buf()];
    let mut left = most_looked;
    'dirs: while let Some(dir) = dirs.pop() {
        if left == 0 {
            found.whole = false;
            break;
        }
        left -= 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            // On a filesystem that ignores case, as macOS's does, git
            // finds `.GIT` where it looks for `.git`.
            if name.eq_ignore_ascii_case(".git") {
                if dir != top {
                    let path = entry.path();
                    let held = if kind.is_dir() {
                        let Some(held) = repository(&path, &mut left) else {
                            found.whole = false;
                            break 'dirs;
                        };
                        Held::Repository(held)
                    } else {
                        // Git follows a `.git` link, and a `.git` file's
                        // `gitdir:`, to the repository it uses.
                        let to = if kind.is_symlink() {
                            Some(path.clone())
                        } else {
                            pointed(&path)
                        };
                        let there = match to {
                            Some(to) if to.is_dir() => {
                                let Some(held) = repository(&to, &mut left) else {
                                    found.whole = false;
                                    break 'dirs;
                                };
                                Some(held)
                            }
                            _ => None,
                        };
                        Held::Pointer(file(&path).unwrap_or_default(), there)
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
/// there, and every hook, by name. `hooks` as a link is kept as one. Each
/// hook is one of the `left` the look has; None when they run out.
fn repository(git: &Path, left: &mut usize) -> Option<BTreeMap<String, Kept>> {
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
                    if *left == 0 {
                        return None;
                    }
                    *left -= 1;
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
    Some(held)
}

/// What stands at `path`, to compare, without following it if it is a
/// link or opening it if it is not a plain file: a link's target, a plain
/// file's bytes up to [`KEPT_BYTES`], or for a longer file, or a FIFO,
/// socket or device, what kind it is and its inode. None when nothing is
/// there.
fn file(path: &Path) -> Option<Kept> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        let to = std::fs::read_link(path)
            .map(|to| to.into_os_string().into_encoded_bytes())
            .unwrap_or_default();
        return Some(kept("link", &to));
    }
    if kind.is_file() && meta.len() <= KEPT_BYTES {
        if let Some(bytes) = read_plain(path) {
            return Some(kept("file", &bytes));
        }
    }
    Some(kept("stamp", stamp(&meta).as_bytes()))
}

/// The digest of `bytes`, under what they are, so a link and a file
/// holding its target's name are not the same.
fn kept(what: &str, bytes: &[u8]) -> Kept {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(what.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    hasher.finalize().into()
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

/// Where a `.git` file points, as git reads it: `gitdir: <dir>`, relative
/// to the file's folder. None for anything else, or a file that is not
/// plain or is too long.
fn pointed(path: &Path) -> Option<PathBuf> {
    let bytes = read_plain(path)?;
    let text = String::from_utf8(bytes).ok()?;
    let dir = text.trim_end().strip_prefix("gitdir:")?.trim();
    (!dir.is_empty() && !dir.contains('\n')).then(|| path.parent().unwrap_or(path).join(dir))
}

/// The `modules/` of each git directory `top` uses, where git keeps its
/// submodules' repositories: a linked worktree's own (`git submodule
/// update` there puts them under `worktrees/<name>/modules`) and the
/// repository's. The sandbox keeps both read-only.
fn modules(top: &Path) -> Vec<PathBuf> {
    let mut dirs = perch_agent::linked_git_dirs(top);
    if dirs.is_empty() {
        dirs.push(top.join(".git"));
    }
    dirs.into_iter()
        .map(|dir| dir.canonicalize().unwrap_or(dir).join("modules"))
        .collect()
}

/// The submodules `.gitmodules` at `top` declares, by the `.git` file each
/// has once checked out. `git submodule update` makes those, and they point
/// into the top's own repository, which the sandbox keeps. The run can
/// write `.gitmodules` too, so the declared ones are taken before it, and
/// it is read as a hook is: one that is not a plain file, or is too long,
/// declares none.
fn submodules(top: &Path) -> BTreeSet<PathBuf> {
    let Some(bytes) = read_plain(&top.join(".gitmodules")) else {
        return BTreeSet::new();
    };
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "path").then(|| top.join(value.trim()).join(".git"))
        })
        .collect()
}

/// Why a git write outside the sandbox, which a grant would answer while
/// a run goes, must be asked about instead: a `.git` below the top that
/// was not there as the run started (`before`), or holds something else
/// `now`, or a worktree too big to look through. Git there runs what that
/// `.git`'s config says, outside the sandbox: `git add -A` looks inside a
/// submodule with its `core.fsmonitor`. A submodule checked out since is
/// asked about too. None when nothing has.
pub(crate) fn unsettled(top: &Path, before: &Nested, now: &Nested) -> Option<String> {
    let named = |path: &Path| path.strip_prefix(top).unwrap_or(path).display().to_string();
    let why = " Git outside the sandbox would run what it holds, so the commit grant doesn't answer for this.";
    for (path, held) in &now.gits {
        match before.gits.get(path) {
            None => {
                return Some(format!(
                    "A Git repository appeared at {} during this run.{why}",
                    named(path)
                ))
            }
            Some(was) if was != held => {
                return Some(format!(
                    "The Git repository at {} changed during this run.{why}",
                    named(path)
                ))
            }
            Some(_) => {}
        }
    }
    (!now.whole).then(|| format!(
        "The worktree has more than {MOST_LOOKED} folders and Git hooks, more than Perch looks through for a Git repository made inside it, so the commit grant doesn't answer for this."
    ))
}

/// The `.git`s in `after` that were not in `before`, or hold something
/// else, and what the run's record says about them: None when there are
/// none.
pub(crate) fn appeared(top: &Path, before: &Nested, after: &Nested) -> Option<String> {
    let named = |path: &Path| path.strip_prefix(top).unwrap_or(path).display().to_string();
    let mut new = Vec::new();
    let mut changed = Vec::new();
    for (path, held) in &after.gits {
        match before.gits.get(path) {
            None if matches!(held, Held::Pointer(..)) && before.checked_out(path) => {}
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
            "This Work run took the worktree past {MOST_LOOKED} folders and Git hooks, more than Perch looks through for a Git repository made below its top. The sandbox cannot stop every way a run can make one, so look for any before running git there."
        ));
    };
    Some(format!(
        "{}{}, below the worktree's top, during this Work run. The sandbox cannot stop every way a run can do that, so look at what it holds before running git there.",
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
        std::fs::create_dir_all(top.join(".git/modules/lib")).unwrap();
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

    /// A `.git` file where a submodule goes passes only when it points
    /// into the repository's `modules/`, which the run could not write,
    /// and only where `.gitmodules` said one goes before the run: the run
    /// can write `.gitmodules`, and a `.git` file naming a repository it
    /// made under any other name.
    #[test]
    fn a_submodule_s_git_file_passes_only_into_modules_as_declared_before() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join(".git/modules/lib")).unwrap();
        std::fs::write(
            top.join(".gitmodules"),
            "[submodule \"lib\"]\n\tpath = vendor/lib\n",
        )
        .unwrap();
        let before = below(top);

        std::fs::create_dir_all(top.join("vendor/.evil_git")).unwrap();
        std::fs::create_dir_all(top.join("vendor/lib")).unwrap();
        std::fs::write(top.join("vendor/lib/.git"), "gitdir: ../.evil_git\n").unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("A Git repository appeared at vendor/lib/.git,"),
            "{said}"
        );
        // Out of `modules/` by `..` is out.
        std::fs::write(
            top.join("vendor/lib/.git"),
            "gitdir: ../../.git/modules/../../vendor/.evil_git\n",
        )
        .unwrap();
        assert!(appeared(top, &before, &below(top)).is_some());

        // Declared by a `.gitmodules` the run wrote is not declared.
        std::fs::remove_dir_all(top.join("vendor")).unwrap();
        std::fs::create_dir_all(top.join(".git/modules/other")).unwrap();
        std::fs::write(
            top.join(".gitmodules"),
            "[submodule \"lib\"]\n\tpath = vendor/lib\n[submodule \"other\"]\n\tpath = other\n",
        )
        .unwrap();
        std::fs::create_dir(top.join("other")).unwrap();
        std::fs::write(top.join("other/.git"), "gitdir: ../.git/modules/other\n").unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("A Git repository appeared at other/.git,"),
            "{said}"
        );
    }

    /// Git follows a `.git` file to the repository it names, so a change
    /// there is a change to the `.git`, though the file is as it was.
    #[test]
    fn a_change_where_a_git_file_points_is_a_change() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("store/hooks")).unwrap();
        std::fs::create_dir(top.join("linked")).unwrap();
        std::fs::write(top.join("linked/.git"), "gitdir: ../store\n").unwrap();
        let before = below(top);
        std::fs::write(top.join("store/config"), "[core]\n\tfsmonitor = x\n").unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("The Git repository at linked/.git was changed"),
            "{said}"
        );
    }

    /// macOS's filesystem ignores case, so git takes `.GIT` for `.git`.
    #[test]
    fn a_git_in_other_case_is_a_git() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        let before = below(top);
        std::fs::create_dir_all(top.join("x/.GIT")).unwrap();
        let said = appeared(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("A Git repository appeared at x/.GIT,"),
            "{said}"
        );
    }

    /// While the run goes, any `.git` new or changed since it started is
    /// a reason to ask, a submodule's among them, and so is a worktree
    /// too big to look through.
    #[test]
    fn unsettled_names_what_appeared_or_changed() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join(".git/modules/lib")).unwrap();
        std::fs::create_dir_all(top.join("old/.git")).unwrap();
        std::fs::write(
            top.join(".gitmodules"),
            "[submodule \"lib\"]\n\tpath = lib\n",
        )
        .unwrap();
        let before = below(top);
        assert_eq!(unsettled(top, &before, &below(top)), None);

        std::fs::create_dir(top.join("lib")).unwrap();
        std::fs::write(top.join("lib/.git"), "gitdir: ../.git/modules/lib\n").unwrap();
        let said = unsettled(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("A Git repository appeared at lib/.git during this run. Git outside"),
            "{said}"
        );
        std::fs::remove_dir_all(top.join("lib")).unwrap();

        std::fs::write(top.join("old/.git/config"), "[core]\n").unwrap();
        let said = unsettled(top, &before, &below(top)).unwrap();
        assert!(
            said.starts_with("The Git repository at old/.git changed during this run."),
            "{said}"
        );
        std::fs::remove_file(top.join("old/.git/config")).unwrap();
        assert_eq!(unsettled(top, &before, &below(top)), None);

        let said = unsettled(top, &before, &below_within(top, 1)).unwrap();
        assert!(said.starts_with("The worktree has more than"), "{said}");
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
        assert_eq!(held["config"], kept("link", b"/dev/zero"));
    }

    /// The run can leave a FIFO or a link to `/dev/zero` as `.gitmodules`
    /// too: it is not opened, and declares no submodules, so a `.git`
    /// file checked out under its name is named.
    #[cfg(unix)]
    #[test]
    fn a_gitmodules_that_is_not_a_plain_file_declares_nothing() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        let before = below(top);
        std::fs::create_dir_all(top.join("vendor/lib")).unwrap();
        std::fs::write(top.join("vendor/lib/.git"), "gitdir: /there\n").unwrap();
        fifo(&top.join(".gitmodules"));
        assert!(submodules(top).is_empty());
        assert!(appeared(top, &before, &below(top)).is_some());
        std::fs::remove_file(top.join(".gitmodules")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", top.join(".gitmodules")).unwrap();
        assert!(submodules(top).is_empty());
        std::fs::remove_file(top.join(".gitmodules")).unwrap();
        std::fs::write(top.join(".gitmodules"), vec![b'#'; KEPT_BYTES as usize + 1]).unwrap();
        assert!(submodules(top).is_empty());
    }

    /// Hooks count toward the most looked at, as folders do: a run that
    /// fills a nested `.git/hooks` with thousands, hard links to one
    /// long file say, takes the look past it rather than filling memory.
    #[test]
    fn hooks_count_toward_the_most_looked_at() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        std::fs::create_dir_all(top.join("x/.git/hooks")).unwrap();
        let before = below_within(top, 5);
        assert!(before.whole);
        for n in 0..5 {
            std::fs::write(top.join(format!("x/.git/hooks/h{n}")), "").unwrap();
        }
        let after = below_within(top, 5);
        assert!(!after.whole);
        let said = appeared(top, &before, &after).unwrap();
        assert!(
            said.starts_with("This Work run took the worktree past"),
            "{said}"
        );
    }

    #[cfg(unix)]
    fn fifo(path: &Path) {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }

    /// A long hook is known by its size and change time, not read whole,
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
        assert_ne!(
            held["hooks/pre-commit"],
            kept("file", &vec![b'a'; KEPT_BYTES as usize + 1])
        );
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
