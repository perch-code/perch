//! Perch's log on disk: `$PERCH_HOME/logs/perch.<date>.log`, beside
//! stderr. A Finder launch throws stderr away, and a bug report wants the
//! minutes before the bug. `tracing-appender` starts a file each day and
//! keeps the newest [`KEPT`].

use std::fs;
use std::path::{Path, PathBuf};

use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// Today's file and the six days before it.
pub const KEPT: usize = 7;

const PREFIX: &str = "perch";
const SUFFIX: &str = "log";

/// Where the logs live.
pub fn dir() -> PathBuf {
    perch_github::config::perch_home().join("logs")
}

/// The file writer, written to as each event happens rather than from a
/// worker thread: a panic on the main thread aborts on its way out
/// through AppKit, and a line queued for a worker would go with it.
pub fn appender(dir: &Path) -> Result<RollingFileAppender, String> {
    // The builder prunes old files before it makes the directory, and
    // prints an error to stderr when there's no directory to read.
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(PREFIX)
        .filename_suffix(SUFFIX)
        .max_log_files(KEPT)
        .build(dir)
        .map_err(|e| e.to_string())
}

/// The last `lines` lines Perch logged, from the newest files: early in
/// the day, yesterday's end comes first.
pub fn recent(dir: &Path, lines: usize) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{PREFIX}.")) && name.ends_with(&format!(".{SUFFIX}"))
                })
        })
        .collect();
    // Dated names sort by day.
    files.sort();
    let mut kept: Vec<String> = Vec::new();
    for file in files.iter().rev() {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let mut older: Vec<String> = text.lines().map(str::to_string).collect();
        older.append(&mut kept);
        kept = older;
        if kept.len() >= lines {
            break;
        }
    }
    let from = kept.len().saturating_sub(lines);
    kept.split_off(from)
}

/// Log a panic before the default hook prints it, so a crash leaves its
/// cause in the file.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with no message".to_string());
        let location = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()))
            .unwrap_or_else(|| "an unknown place".to_string());
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        tracing::error!("panic in thread {thread} at {location}: {message}");
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    #[test]
    fn the_appender_writes_a_dated_file_in_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = appender(dir.path()).unwrap();
        file.write_all(b"line one\n").unwrap();
        file.flush().unwrap();
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(
            names[0].starts_with("perch.") && names[0].ends_with(".log"),
            "{names:?}"
        );
        assert_eq!(recent(dir.path(), 5), vec!["line one"]);
    }

    #[test]
    fn the_tail_reaches_back_into_yesterday() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("perch.2026-09-30.log"), "a\nb\nc\n").unwrap();
        fs::write(dir.path().join("perch.2026-10-01.log"), "d\ne\n").unwrap();
        fs::write(dir.path().join("other.txt"), "x\n").unwrap();
        assert_eq!(recent(dir.path(), 4), vec!["b", "c", "d", "e"]);
        assert_eq!(recent(dir.path(), 1), vec!["e"]);
        assert!(recent(&dir.path().join("missing"), 3).is_empty());
    }
}
