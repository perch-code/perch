//! Filesystem operations for the live knowledge archive.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags, CWD};
use rustix::io::Errno;

const DIRECTORY_MODE: Mode = Mode::from_bits_retain(0o700);
const FILE_MODE: Mode = Mode::from_bits_retain(0o600);
const TEMPORARY_PREFIX: &str = ".perch-tmp-";
const OPEN_DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW);

/// A readable, bounded path component for a branch. The caller appends the
/// item hash to distinguish branch names with the same spelling here.
pub fn safe_component(name: &str) -> String {
    let slug: String = name
        .chars()
        .take(128)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches(['.', '-']);
    if slug.is_empty() {
        "item".into()
    } else {
        slug.into()
    }
}

/// Keep ordinary repository components as named and escape other bytes so
/// arbitrary local repository names cannot change the archive layout.
pub fn encode_component(name: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'-' | b'.') && name != "." && name != ".."
        {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02x}").unwrap();
        }
    }
    if encoded.is_empty() {
        return "%".into();
    }
    if encoded.len() > 255 {
        // The prefix is ASCII. The extra '%' cannot occur in an escaped
        // name and separates this representation from lossless ones.
        encoded.truncate(230);
        let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        write!(encoded, "%%{hash:016x}").unwrap();
    }
    encoded
}

/// Make `dir` hold precisely `files`, whose keys are relative paths. Publish
/// each changed file atomically, the index last, then prune obsolete entries.
pub fn sync(dir: &Path, files: &BTreeMap<String, String>) -> io::Result<()> {
    split_target(dir)?;
    let mut directories = BTreeSet::new();
    for relative in files.keys() {
        if relative.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.contains(['\0', '\\'])
        }) {
            return Err(invalid_path(Path::new(relative)));
        }
        for parent in Path::new(relative).ancestors().skip(1) {
            if files.contains_key(parent.to_str().unwrap()) {
                return Err(invalid_path(parent));
            }
            directories.insert(parent);
        }
    }

    let dir = open_directory(dir, true)?;
    check_tree(&dir)?;
    set_mode(&dir, DIRECTORY_MODE)?;
    for (relative, contents) in files {
        if relative != "INDEX.md" {
            let relative = Path::new(relative);
            let mut parent = dir.try_clone()?;
            for part in relative.parent().unwrap().components() {
                parent = open_subdirectory(&parent, part.as_os_str(), true)?;
                set_mode(&parent, DIRECTORY_MODE)?;
            }
            write_at(&parent, relative.file_name().unwrap(), contents)?;
        }
    }
    if let Some(index) = files.get("INDEX.md") {
        write_at(&dir, OsStr::new("INDEX.md"), index)?;
    }
    prune(&dir, Path::new(""), files, &directories)
}

/// Create a private directory and any missing ancestors, refusing symlinks.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    split_target(dir)?;
    set_mode(&open_directory(dir, true)?, DIRECTORY_MODE)
}

/// Atomically write a private file, leaving matching contents untouched.
pub fn write_file(path: &Path, contents: &str) -> io::Result<()> {
    let (parent, name) = split_target(path)?;
    write_at(&open_directory(parent, true)?, name, contents)
}

/// Remove temporary files left by interrupted writes. Call only while no
/// writer can be publishing a file in this directory.
pub fn prune_temporary_files(path: &Path) -> io::Result<()> {
    let Some(dir) = existing_directory(path)? else {
        return Ok(());
    };
    for name in entry_names(&dir)? {
        if name.as_bytes().starts_with(TEMPORARY_PREFIX.as_bytes()) {
            remove_file_at(&dir, &name)?;
        }
    }
    Ok(())
}

/// List immediate subdirectories. Symlink entries are left alone, including
/// when pointing at a directory. A missing directory has no children.
pub fn child_dirs(path: &Path) -> io::Result<Vec<PathBuf>> {
    let Some(dir) = existing_directory(path)? else {
        return Ok(Vec::new());
    };
    let mut children = Vec::new();
    for name in entry_names(&dir)? {
        if kind(&dir, &name)? == Some(FileType::Directory) {
            children.push(path.join(name));
        }
    }
    children.sort_unstable();
    Ok(children)
}

/// Why an external source can't be shared with a run, without creating or
/// changing anything: it is missing or not a plain directory, something in
/// it is a symlink or a file an agent CLI would load as instructions, or it
/// holds more than `limit` entries, which stops the walk.
pub fn check_source(path: &Path, limit: usize) -> Result<(), String> {
    let dir = match open_directory(path, false) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err("the directory does not exist".into())
        }
        Err(error) => return Err(format!("the directory can't be opened: {error}")),
    };
    let mut seen = 0_usize;
    let mut refusal = None;
    let walked = walk_tree(&dir, Path::new(""), &mut |path, kind| {
        seen += 1;
        let why = if seen > limit {
            format!("it holds more than {limit} files and directories")
        } else if kind == Some(FileType::Symlink) {
            format!("{} is a symlink", path.display())
        } else if path.file_name().is_some_and(|name| {
            ["AGENTS.md", "CLAUDE.md", "GEMINI.md"]
                .iter()
                .any(|reserved| name.eq_ignore_ascii_case(reserved))
        }) {
            format!("{} is an agent instructions file", path.display())
        } else {
            return Ok(());
        };
        refusal = Some(why);
        Err(io::ErrorKind::InvalidInput.into())
    });
    match (refusal, walked) {
        (Some(why), _) => Err(why),
        (None, Err(error)) => Err(format!("the directory can't be read: {error}")),
        (None, Ok(())) => Ok(()),
    }
}

/// Remove a directory tree only if it contains no symlinks. A missing
/// directory needs no work.
pub fn remove_dir(path: &Path) -> io::Result<()> {
    let (parent, name) = split_target(path)?;
    let Some(parent) = existing_directory(parent)? else {
        return Ok(());
    };
    let dir = match open_subdirectory(&parent, name, false) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    check_tree(&dir)?;
    prune(&dir, Path::new(""), &BTreeMap::new(), &BTreeSet::new())?;
    Ok(fs::unlinkat(&parent, name, AtFlags::REMOVEDIR)?)
}

/// Remove a directory when empty, refusing symlinks. A missing or nonempty
/// directory needs no work.
pub fn remove_empty_dir(path: &Path) -> io::Result<()> {
    let (parent, name) = split_target(path)?;
    let Some(parent) = existing_directory(parent)? else {
        return Ok(());
    };
    match fs::unlinkat(&parent, name, AtFlags::REMOVEDIR) {
        Ok(()) | Err(Errno::NOENT | Errno::NOTEMPTY | Errno::EXIST) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Remove a plain file, refusing symlinks. A missing file needs no work.
pub fn remove_file(path: &Path) -> io::Result<()> {
    let (parent, name) = split_target(path)?;
    let Some(parent) = existing_directory(parent)? else {
        return Ok(());
    };
    remove_file_at(&parent, name)
}

fn remove_file_at(dir: &OwnedFd, name: &OsStr) -> io::Result<()> {
    match kind(dir, name)? {
        None => Ok(()),
        Some(FileType::RegularFile) => Ok(fs::unlinkat(dir, name, AtFlags::empty())?),
        Some(FileType::Symlink) => Err(refuse_symlink(name)),
        _ => Err(invalid_path(Path::new(name))),
    }
}

fn invalid_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid archive path: {}", path.display()),
    )
}

fn split_target(path: &Path) -> io::Result<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok((parent, name)),
        _ => Err(invalid_path(path)),
    }
}

// Descend one component at a time. Using a directory descriptor for each
// step also prevents a symlink swapped into an ancestor from redirecting a
// subsequent write or removal.
fn open_directory(path: &Path, create: bool) -> io::Result<OwnedFd> {
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(invalid_path(path));
    }
    let base = if path.is_absolute() { "/" } else { "." };
    let mut dir = fs::openat(CWD, base, OPEN_DIRECTORY, Mode::empty())?;
    for part in path.components() {
        if let Component::Normal(name) = part {
            dir = open_subdirectory(&dir, name, create)?;
        }
    }
    Ok(dir)
}

fn existing_directory(path: &Path) -> io::Result<Option<OwnedFd>> {
    match open_directory(path, false) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn open_subdirectory(parent: &OwnedFd, name: &OsStr, create: bool) -> io::Result<OwnedFd> {
    match fs::openat(parent, name, OPEN_DIRECTORY, Mode::empty()) {
        Err(Errno::NOENT) if create => {
            match fs::mkdirat(parent, name, DIRECTORY_MODE) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            Ok(fs::openat(parent, name, OPEN_DIRECTORY, Mode::empty())?)
        }
        result => Ok(result?),
    }
}

fn kind(dir: &OwnedFd, name: &OsStr) -> io::Result<Option<FileType>> {
    match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(FileType::from_raw_mode(stat.st_mode))),
        Err(Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn refuse_symlink(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("symlink in archive: {}", Path::new(name).display()),
    )
}

fn check_tree(dir: &OwnedFd) -> io::Result<()> {
    walk_tree(dir, Path::new(""), &mut |path, kind| match kind {
        Some(FileType::Symlink) => Err(refuse_symlink(path.as_os_str())),
        _ => Ok(()),
    })
}

/// Visit every entry under `dir`, by its path from it, before descending
/// into it. A symlink is visited and never followed.
fn walk_tree(
    dir: &OwnedFd,
    prefix: &Path,
    visit: &mut impl FnMut(&Path, Option<FileType>) -> io::Result<()>,
) -> io::Result<()> {
    for name in entry_names(dir)? {
        let path = prefix.join(&name);
        let kind = kind(dir, &name)?;
        visit(&path, kind)?;
        if kind == Some(FileType::Directory) {
            walk_tree(&open_subdirectory(dir, &name, false)?, &path, visit)?;
        }
    }
    Ok(())
}

// Collect names before any unlink: changing a directory during iteration
// would leave which entries the iterator returns up to the filesystem.
fn entry_names(dir: &OwnedFd) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in Dir::read_from(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(OsStr::from_bytes(name.to_bytes()).to_owned());
        }
    }
    Ok(names)
}

fn set_mode(fd: &OwnedFd, mode: Mode) -> io::Result<()> {
    if Mode::from_raw_mode(fs::fstat(fd)?.st_mode) != mode {
        fs::fchmod(fd, mode)?;
    }
    Ok(())
}

fn write_at(dir: &OwnedFd, name: &OsStr, contents: &str) -> io::Result<()> {
    match kind(dir, name)? {
        Some(FileType::RegularFile) => {
            let existing = fs::openat(
                dir,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )?;
            let stat = fs::fstat(&existing)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(invalid_path(Path::new(name)));
            }
            if stat.st_nlink == 1 && stat.st_size as u64 == contents.len() as u64 {
                let mut bytes = Vec::with_capacity(contents.len());
                let existing = File::from(existing);
                (&existing)
                    .take(contents.len() as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes == contents.as_bytes() {
                    if Mode::from_raw_mode(stat.st_mode) != FILE_MODE {
                        fs::fchmod(&existing, FILE_MODE)?;
                    }
                    return Ok(());
                }
            }
        }
        None => {}
        Some(FileType::Symlink) => return Err(refuse_symlink(name)),
        _ => return Err(invalid_path(Path::new(name))),
    }

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = format!("{TEMPORARY_PREFIX}{}-{sequence}", std::process::id());
        let mut file = match fs::openat(
            dir,
            &temporary,
            OFlags::WRONLY | OFlags::CLOEXEC | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
            FILE_MODE,
        ) {
            Ok(fd) => File::from(fd),
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| {
            fs::fchmod(&file, FILE_MODE)?;
            file.write_all(contents.as_bytes())?;
            if kind(dir, name)? == Some(FileType::Symlink) {
                return Err(refuse_symlink(name));
            }
            Ok(fs::renameat(dir, &temporary, dir, name)?)
        })();
        if result.is_err() {
            let _ = fs::unlinkat(dir, &temporary, AtFlags::empty());
        }
        return result;
    }
}

fn prune(
    dir: &OwnedFd,
    path: &Path,
    files: &BTreeMap<String, String>,
    directories: &BTreeSet<&Path>,
) -> io::Result<()> {
    for name in entry_names(dir)? {
        let name = name.as_os_str();
        let relative = path.join(name);
        match kind(dir, name)? {
            Some(FileType::Symlink) => return Err(refuse_symlink(name)),
            Some(FileType::Directory) => {
                prune(
                    &open_subdirectory(dir, name, false)?,
                    &relative,
                    files,
                    directories,
                )?;
                if !directories.contains(relative.as_path()) {
                    fs::unlinkat(dir, name, AtFlags::REMOVEDIR)?;
                }
            }
            Some(_)
                if !relative
                    .to_str()
                    .is_some_and(|path| files.contains_key(path)) =>
            {
                fs::unlinkat(dir, name, AtFlags::empty())?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, FileTimes};
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::time::{Duration, SystemTime};

    #[test]
    fn a_source_says_why_it_cannot_be_shared() {
        let (_scratch, root) = scratch();
        assert_eq!(
            check_source(&root.join("missing"), 10),
            Err("the directory does not exist".into())
        );

        let source = root.join("discord");
        fs::create_dir_all(source.join("2026/11")).unwrap();
        fs::write(source.join("INDEX.md"), "# Index").unwrap();
        fs::write(source.join("2026/11/general.md"), "hello").unwrap();
        assert_eq!(check_source(&source, 10), Ok(()));
        assert_eq!(
            check_source(&source, 3),
            Err("it holds more than 3 files and directories".into())
        );

        fs::write(source.join("2026/11/claude.md"), "instructions").unwrap();
        assert_eq!(
            check_source(&source, 10),
            Err("2026/11/claude.md is an agent instructions file".into())
        );
        fs::remove_file(source.join("2026/11/claude.md")).unwrap();

        symlink(source.join("2026/11"), source.join("latest")).unwrap();
        assert_eq!(check_source(&source, 10), Err("latest is a symlink".into()));
        fs::remove_file(source.join("latest")).unwrap();

        // The source itself, or a directory on the way to it, a link.
        symlink(&source, root.join("alias")).unwrap();
        assert!(check_source(&root.join("alias"), 10).is_err());
        assert_eq!(check_source(&source, 10), Ok(()));
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let scratch = tempfile::tempdir().unwrap();
        // macOS puts the default temp directory below the /var symlink.
        let path = scratch.path().canonicalize().unwrap();
        (scratch, path)
    }

    fn documents(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, body)| (path.to_string(), body.to_string()))
            .collect()
    }

    #[test]
    fn sync_keeps_unchanged_files_and_replaces_changed_ones_atomically() {
        let (_scratch, root) = scratch();
        let item = root.join("repos/acme/widgets/pulls/17");
        let index = item.join("INDEX.md");
        let conversation = item.join("conversation.md");
        let files = documents(&[
            ("INDEX.md", "index"),
            ("conversation.md", "before"),
            ("private/thread.md", "private"),
        ]);
        sync(&item, &files).unwrap();
        let original_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for (relative, body) in &files {
            let path = item.join(relative);
            assert_eq!(fs::read_to_string(&path).unwrap(), *body);
            File::open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(original_time))
                .unwrap();
        }

        sync(&item, &files).unwrap();
        for relative in files.keys() {
            let metadata = fs::metadata(item.join(relative)).unwrap();
            assert_eq!(metadata.modified().unwrap(), original_time);
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(
            fs::metadata(&item).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(item.join("private"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join("repos"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let mut before = File::open(&conversation).unwrap();
        sync(
            &item,
            &documents(&[("INDEX.md", "index"), ("conversation.md", "after!")]),
        )
        .unwrap();
        let mut old_contents = String::new();
        before.read_to_string(&mut old_contents).unwrap();
        assert_eq!(old_contents, "before");
        assert_ne!(
            before.metadata().unwrap().ino(),
            fs::metadata(&conversation).unwrap().ino()
        );
        assert_eq!(fs::read_to_string(&conversation).unwrap(), "after!");
        assert_eq!(
            fs::metadata(index).unwrap().modified().unwrap(),
            original_time
        );
        assert!(!item.join("private").exists());
        assert_eq!(fs::read_dir(&item).unwrap().count(), 2);
    }

    #[test]
    fn sync_validates_paths_before_writing() {
        let (_scratch, root) = scratch();
        let item = root.join("item");
        let outside = root.join("outside");
        for relative in [
            "../outside",
            "a/../../outside",
            "a/./b",
            "a//b",
            "/absolute",
        ] {
            let result = sync(
                &item,
                &documents(&[("INDEX.md", "index"), (relative, "escaped")]),
            );
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
        assert!(!item.exists());
        assert!(!outside.exists());
        assert!(sync(Path::new(""), &BTreeMap::new()).is_err());
        assert!(sync(Path::new("/"), &BTreeMap::new()).is_err());
    }

    #[test]
    fn writing_and_removing_refuse_symlinks() {
        let (_scratch, root) = scratch();
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        let secret = outside.join("secret");
        fs::write(&secret, "leave this alone").unwrap();
        let link = root.join("link");
        symlink(&outside, &link).unwrap();

        assert!(sync(&link, &documents(&[("new.md", "new")])).is_err());
        assert!(ensure_dir(&link.join("subdir")).is_err());
        assert!(write_file(&link.join("secret"), "replace").is_err());
        assert!(remove_file(&link.join("secret")).is_err());
        assert!(remove_dir(&link).is_err());
        assert!(remove_empty_dir(&link).is_err());
        assert!(child_dirs(&link).is_err());

        let item = root.join("item");
        sync(&item, &documents(&[("INDEX.md", "index")])).unwrap();
        let planted = item.join("private.md");
        symlink(&secret, &planted).unwrap();
        assert!(write_file(&planted, "replace").is_err());
        assert!(remove_file(&planted).is_err());
        assert!(sync(&item, &documents(&[("INDEX.md", "new index")])).is_err());
        assert_eq!(fs::read_to_string(item.join("INDEX.md")).unwrap(), "index");
        assert!(remove_dir(&item).is_err());
        assert!(planted.is_symlink());
        assert_eq!(fs::read_to_string(&secret).unwrap(), "leave this alone");
        assert!(!outside.join("new.md").exists());
        assert_eq!(child_dirs(&root).unwrap(), vec![item, outside]);
    }

    #[test]
    fn directory_and_file_removals_can_be_retried() {
        let (_scratch, root) = scratch();
        let archive = root.join("archive");
        let item = archive.join("pulls/1");
        let index = archive.join("INDEX.md");
        sync(&item, &documents(&[("private/1.md", "body")])).unwrap();
        write_file(&index, "index").unwrap();
        let interrupted_write = archive.join(".perch-tmp-123-4");
        fs::write(&interrupted_write, "partial").unwrap();
        prune_temporary_files(&archive).unwrap();
        assert!(!interrupted_write.exists());
        assert_eq!(fs::read_to_string(&index).unwrap(), "index");

        remove_empty_dir(&archive).unwrap();
        assert!(archive.is_dir());
        for _ in 0..2 {
            remove_dir(&item).unwrap();
            remove_file(&index).unwrap();
            remove_empty_dir(&archive.join("pulls")).unwrap();
            remove_empty_dir(&archive).unwrap();
        }
        assert!(root.is_dir());
        assert!(!archive.exists());
    }

    #[test]
    fn components_are_bounded_and_keep_ordinary_names() {
        assert_eq!(safe_component("fix/wal"), "fix-wal");
        assert_eq!(safe_component("../.."), "item");
        assert_eq!(encode_component("widgets.git"), "widgets.git");
        assert_eq!(encode_component(".."), "%2e%2e");
        assert_ne!(encode_component("repo 1"), encode_component("repo%201"));
        assert!(encode_component(&"è".repeat(125)).len() <= 255);
        assert!(safe_component(&"x".repeat(255)).len() < 255);
    }
}
