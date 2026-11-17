//! What a bridge the app put there clears from its directory: the app
//! keeps each build of `perchd` as `$PERCH_HOME/bin/perchd-<build>`, so
//! updating it, or a dev binary rebuilt, leaves the old ones there. A
//! bridge running from that directory marks its own binary as used now,
//! and removes every other `perchd-*` that nothing has used in a week,
//! and the temporary files of uploads cut short over an hour ago. A build
//! another app still connects with is used each time it does, so it
//! stays: two apps of different builds do not send theirs in turn. A
//! running engine's binary can be removed: the file goes once the engine
//! does.
//!
//! When a build was last used is its file's modification time, which the
//! bridge sets as it starts and every few hours while it stays, and the
//! app's scripts set when they find the build already there. Access
//! times are not kept on many hosts.
//!
//! None of it can fail the bridge, and none of it is said: the bridge's
//! stdout is the wire, and its stderr's last line is what the app shows
//! when a connection ends.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How old an upload's temporary file is before it is taken as left over.
const LEFT_OVER: Duration = Duration::from_secs(60 * 60);

/// How long a build goes unused before it is removed.
const UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A file in the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub modified: SystemTime,
}

/// How often a bridge that stays marks its binary as used again: a
/// connection kept for weeks starts no other bridge to do it.
const MARK_AGAIN: Duration = Duration::from_secs(6 * 60 * 60);

/// Mark this `perchd` as used now, when it is one the app put there, and
/// clear the directory in the background. The mark is made before this
/// returns: a bridge that ends at once, turned away by a busy engine, has
/// still been used.
pub fn start() {
    let Some((exe, own)) = installed() else {
        return;
    };
    mark(&exe);
    let _ = std::thread::Builder::new()
        .name("cleanup".into())
        .spawn(move || {
            clear(&exe, &own);
            loop {
                std::thread::sleep(MARK_AGAIN);
                mark(&exe);
            }
        });
}

/// This `perchd` and its file's name, when it is one the app put in the
/// Perch home's `bin`.
fn installed() -> Option<(PathBuf, String)> {
    let exe = crate::own_exe().ok()?;
    let own = exe.file_name()?.to_str()?.to_string();
    let bin = perch_github::config::perch_home().join("bin");
    let there = exe.parent()?.canonicalize().ok()? == bin.canonicalize().ok()?;
    (own.starts_with("perchd-") && there).then_some((exe, own))
}

/// Used now. Opened to read: a binary that is running can't be opened to
/// write, and its owner may set its times either way.
fn mark(exe: &Path) {
    if let Ok(file) = std::fs::File::open(exe) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn clear(exe: &Path, own: &str) -> Option<()> {
    let dir = exe.parent()?;
    let entries: Vec<Entry> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            // Files only: never a link, or a directory someone put there.
            let meta = entry.metadata().ok()?;
            meta.file_type().is_file().then_some(())?;
            Some(Entry {
                name: entry.file_name().to_str()?.to_string(),
                modified: meta.modified().ok()?,
            })
        })
        .collect();
    for name in to_remove(own, &entries, SystemTime::now()) {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Some(())
}

/// What goes, of `entries`, for the bridge whose binary is `own`: each
/// other `perchd-*` unused for a week, and each `.perchd-*` over an hour
/// old.
pub fn to_remove(own: &str, entries: &[Entry], now: SystemTime) -> Vec<String> {
    let older_than = |entry: &Entry, age: Duration| {
        now.duration_since(entry.modified)
            .is_ok_and(|since| since > age)
    };
    entries
        .iter()
        .filter(|entry| {
            if entry.name.starts_with("perchd-") {
                entry.name != own && older_than(entry, UNUSED)
            } else {
                entry.name.starts_with(".perchd-") && older_than(entry, LEFT_OVER)
            }
        })
        .map(|entry| entry.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_unused_for_a_week_and_old_uploads_go() {
        let now = SystemTime::now();
        let ago = |mins: u64| now - Duration::from_secs(mins * 60);
        let (hour, day) = (60, 24 * 60);
        let entry = |name: &str, mins| Entry {
            name: name.into(),
            modified: ago(mins),
        };
        let entries = [
            // This bridge's own, whatever its file says: it is running.
            entry("perchd-0123456789ab", 30 * day),
            // Another app's, used yesterday, and one used six days ago.
            entry("perchd-ba9876543210", day),
            entry("perchd-111111111111", 6 * day),
            // Nothing has used these in over a week, one named as an app
            // from before builds were hashes named it.
            entry("perchd-222222222222", 8 * day),
            entry("perchd-0.8.0", 90 * day),
            entry(".perchd-0123456789ab.4242", 5),
            entry(".perchd-333333333333.17", 2 * hour),
            entry("notes.txt", 400 * day),
        ];
        let mut gone = to_remove("perchd-0123456789ab", &entries, now);
        gone.sort();
        assert_eq!(
            gone,
            [
                ".perchd-333333333333.17",
                "perchd-0.8.0",
                "perchd-222222222222"
            ]
        );
        // Alone, nothing goes.
        assert!(to_remove("perchd-0123456789ab", &entries[..1], now).is_empty());
        // A file dated ahead of the clock is not taken as old.
        let ahead = [
            Entry {
                name: ".perchd-x.1".into(),
                modified: now + Duration::from_secs(7200),
            },
            Entry {
                name: "perchd-444444444444".into(),
                modified: now + Duration::from_secs(7200),
            },
        ];
        assert!(to_remove("perchd-0123456789ab", &ahead, now).is_empty());
    }
}
