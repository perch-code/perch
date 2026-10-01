//! Perch's log on disk: `$PERCH_HOME/logs/perch.log`, beside stderr. A
//! Finder launch throws stderr away, and a bug report wants the minutes
//! before the bug. The file turns over at the first line of a new day,
//! the old one renamed `perch.<date>.log`, and only the newest
//! [`KEPT`] files stay.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jiff::civil::Date;

/// The current file and the six days before it.
pub const KEPT: usize = 7;

const NAME: &str = "perch.log";

/// Where the logs live.
pub fn dir() -> PathBuf {
    perch_github::config::perch_home().join("logs")
}

/// The file being written now.
pub fn current() -> PathBuf {
    dir().join(NAME)
}

/// The last `lines` lines of the current log, or none when there isn't
/// one yet.
pub fn tail(path: &Path, lines: usize) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

/// A writer the subscriber's file layer clones per event. Each event is
/// one `write_all` of a whole line, under the lock.
#[derive(Clone)]
pub struct DailyFile {
    inner: Arc<Mutex<State>>,
}

struct State {
    dir: PathBuf,
    day: Date,
    file: Option<File>,
}

impl DailyFile {
    /// Open, or start, today's file in `dir`. A file left from an earlier
    /// day is turned over first. Fails only when the directory can't be
    /// made.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let today = today();
        let path = dir.join(NAME);
        if let Some(day) = modified_day(&path) {
            if day != today {
                turn_over(dir, day);
            }
        }
        let file = append(&path).ok();
        Ok(Self {
            inner: Arc::new(Mutex::new(State {
                dir: dir.to_path_buf(),
                day: today,
                file,
            })),
        })
    }
}

impl Write for DailyFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Ok(mut state) = self.inner.lock() else {
            return Ok(buf.len());
        };
        let today = today();
        if today != state.day {
            state.file = None;
            let previous = state.day;
            turn_over(&state.dir, previous);
            state.day = today;
        }
        if state.file.is_none() {
            state.file = append(&state.dir.join(NAME)).ok();
        }
        // A log that can't be written never takes Perch down with it.
        if let Some(file) = state.file.as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Ok(mut state) = self.inner.lock() {
            if let Some(file) = state.file.as_mut() {
                let _ = file.flush();
            }
        }
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DailyFile {
    type Writer = DailyFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn today() -> Date {
    jiff::Zoned::now().date()
}

fn modified_day(path: &Path) -> Option<Date> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let ts = jiff::Timestamp::try_from(modified).ok()?;
    Some(ts.to_zoned(jiff::tz::TimeZone::system()).date())
}

/// Rename the current file after the day it holds, then drop the oldest
/// past [`KEPT`].
fn turn_over(dir: &Path, day: Date) {
    let from = dir.join(NAME);
    if from.exists() {
        let mut to = dir.join(format!("perch.{day}.log"));
        // Two launches on one day, each turning over an older file, never
        // overwrite each other.
        let mut n = 1;
        while to.exists() {
            to = dir.join(format!("perch.{day}.{n}.log"));
            n += 1;
        }
        let _ = fs::rename(&from, &to);
    }
    prune(dir);
}

fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut old: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("perch.") && name.ends_with(".log") && name != NAME
                })
        })
        .collect();
    // Dated names sort by day.
    old.sort();
    let keep_old = KEPT - 1;
    if old.len() > keep_old {
        for path in &old[..old.len() - keep_old] {
            let _ = fs::remove_file(path);
        }
    }
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

    #[test]
    fn writes_lines_and_reads_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = DailyFile::open(dir.path()).unwrap();
        for i in 0..5 {
            file.write_all(format!("line {i}\n").as_bytes()).unwrap();
        }
        assert_eq!(tail(&dir.path().join(NAME), 2), vec!["line 3", "line 4"]);
        assert!(tail(&dir.path().join("missing.log"), 2).is_empty());
    }

    #[test]
    fn turning_over_keeps_the_newest_files() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=9 {
            fs::write(dir.path().join(NAME), format!("day {day}\n")).unwrap();
            turn_over(dir.path(), Date::new(2026, 9, day).unwrap());
        }
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), KEPT - 1, "{names:?}");
        assert_eq!(names.first().unwrap(), "perch.2026-09-04.log");
        assert_eq!(names.last().unwrap(), "perch.2026-09-09.log");
    }

    #[test]
    fn a_second_turn_over_on_one_day_keeps_both() {
        let dir = tempfile::tempdir().unwrap();
        let day = Date::new(2026, 9, 30).unwrap();
        fs::write(dir.path().join(NAME), "first\n").unwrap();
        turn_over(dir.path(), day);
        fs::write(dir.path().join(NAME), "second\n").unwrap();
        turn_over(dir.path(), day);
        assert!(dir.path().join("perch.2026-09-30.log").exists());
        assert!(dir.path().join("perch.2026-09-30.1.log").exists());
    }
}
