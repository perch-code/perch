//! One engine to a Perch home. Two engines on one `perch.db` would each
//! take the other's runs for crashed ones and mark them Interrupted, and
//! both would sweep the same checkouts. Every engine, the app's or
//! `perchd`'s, holds an exclusive advisory lock on `$PERCH_HOME/engine.lock`
//! while it runs, and the file says which process holds it.
//!
//! The lock is the kernel's (`flock`), so it goes with its holder however
//! that ends, a crash or a kill included, and a stale pid in the file is
//! never taken for a live engine.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The lock's file in a Perch home.
pub fn path(home: &Path) -> PathBuf {
    home.join("engine.lock")
}

/// The lock, held until dropped.
#[derive(Debug)]
pub struct EngineLock {
    /// Closing it lets the lock go. Opened close-on-exec, as Rust opens
    /// every file, so the agents and servers the engine starts never
    /// inherit it and keep it past the engine.
    file: File,
}

/// The pid is taken out before the lock goes, so an engine refused
/// between the next one taking the lock and writing its own pid names
/// nobody rather than this process.
///
/// The lock is let go by name, not left to the file closing: it belongs
/// to the open file, which a child the engine has forked and not yet
/// exec'd still has a copy of, close-on-exec or not. Closing this copy
/// alone would leave the home held until that child execs.
impl Drop for EngineLock {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        // SAFETY: the descriptor is the open file's, alive for the call.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Why an engine could not take its home.
#[derive(Debug)]
pub enum LockError {
    /// Another engine has it: the process the file names, when it names one.
    Held { path: PathBuf, pid: Option<u32> },
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held { path, pid } => {
                let home = path.parent().unwrap_or(path);
                let holder = match pid {
                    Some(pid) => format!("another Perch engine (process {pid})"),
                    None => "another Perch engine".to_string(),
                };
                write!(
                    f,
                    "{holder} is running on {} and holds {}; only one engine may use a Perch home at a time",
                    home.display(),
                    path.display(),
                )
            }
            LockError::Io { path, error } => {
                write!(f, "could not lock {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for LockError {}

impl EngineLock {
    /// Take `home`'s lock without waiting, and write this process's pid
    /// into it.
    pub fn take(home: &Path) -> Result<EngineLock, LockError> {
        let path = path(home);
        let io = |error| LockError::Io {
            path: path.clone(),
            error,
        };
        std::fs::create_dir_all(home).map_err(io)?;
        // Not truncated on open: the holder's pid is what a refusal names.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(io)?;
        // SAFETY: the descriptor is the open file's, alive for the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(LockError::Held {
                    pid: holder(&mut file),
                    path,
                });
            }
            // A home on a file system that has no locks, some network
            // ones: the engine runs as it did before there was a lock,
            // rather than not at all.
            if unlockable(&error) {
                tracing::warn!(%error, path = %path.display(), "the Perch home cannot be locked; nothing stops a second engine using it");
                return Ok(EngineLock { file });
            }
            return Err(io(error));
        }
        let written = (|| {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            writeln!(file, "{}", std::process::id())?;
            file.flush()
        })();
        // The pid is only for the message a second engine gives; the lock
        // holds without it.
        if let Err(error) = written {
            tracing::warn!(%error, path = %path.display(), "could not write the engine's pid");
        }
        Ok(EngineLock { file })
    }

    /// Whether an engine holds `home`'s lock, and its pid when the file
    /// names one, without taking the lock or writing anything: `None` when
    /// none does.
    pub fn holder(home: &Path) -> Option<Option<u32>> {
        let mut file = File::open(path(home)).ok()?;
        // A shared lock is refused only while an engine has its own.
        // SAFETY: the descriptor is the open file's, alive for the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
            // Closing the file lets the shared lock go.
            return None;
        }
        let error = std::io::Error::last_os_error();
        (error.kind() == std::io::ErrorKind::WouldBlock).then(|| holder(&mut file))
    }
}

/// Whether `error` says the file system has no locks to give.
fn unlockable(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::ENOLCK || code == libc::ENOTSUP || code == libc::EOPNOTSUPP
    )
}

/// The pid the lock's file names. None while the holder is still writing
/// it, or when the file holds something else.
fn holder(file: &mut File) -> Option<u32> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_engine_on_the_home_is_refused_with_the_holder_named() {
        let home = tempfile::tempdir().unwrap();
        let held = EngineLock::take(home.path()).unwrap();
        let written = std::fs::read_to_string(path(home.path())).unwrap();
        assert_eq!(written.trim(), std::process::id().to_string());

        let refused = EngineLock::take(home.path()).unwrap_err();
        let LockError::Held { pid, .. } = &refused else {
            panic!("{refused:?}");
        };
        assert_eq!(*pid, Some(std::process::id()));
        let said = refused.to_string();
        assert!(
            said.contains(&format!("process {}", std::process::id())),
            "{said}"
        );
        assert!(said.contains("engine.lock"), "{said}");

        // Looked at, it is named, and still held.
        assert_eq!(
            EngineLock::holder(home.path()),
            Some(Some(std::process::id()))
        );
        assert!(EngineLock::take(home.path()).is_err());

        // Let go, it names nobody, and it is the next engine's.
        drop(held);
        assert_eq!(std::fs::read_to_string(path(home.path())).unwrap(), "");
        assert_eq!(EngineLock::holder(home.path()), None);
        EngineLock::take(home.path()).unwrap();
    }

    /// A child forked as the engine stops has the lock's file open until
    /// it execs, as a copy of the descriptor does here.
    #[test]
    fn the_home_is_let_go_while_a_copy_of_the_lock_s_file_is_still_open() {
        let home = tempfile::tempdir().unwrap();
        let held = EngineLock::take(home.path()).unwrap();
        let copy = held.file.try_clone().unwrap();
        drop(held);
        assert_eq!(EngineLock::holder(home.path()), None);
        EngineLock::take(home.path()).unwrap();
        drop(copy);
    }

    #[test]
    fn a_stale_pid_in_an_unheld_file_is_written_over() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(path(home.path()), "99999999\nleft over\n").unwrap();
        let _held = EngineLock::take(home.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(path(home.path())).unwrap(),
            format!("{}\n", std::process::id())
        );
    }
}
