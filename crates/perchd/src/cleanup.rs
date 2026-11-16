//! What a bridge the app installed clears from its directory: the app
//! installs each build of `perchd` as `$PERCH_HOME/bin/perchd-<build>`, so
//! updating it, or a dev binary rebuilt, leaves the old ones there. A
//! bridge running from that directory removes every other `perchd-*` but
//! the newest of them, which an app not yet updated on another Mac may
//! still ask for, and the temporary files of uploads cut short over an
//! hour ago. A running engine's binary can be removed on Linux: the file
//! goes once the engine does.
//!
//! None of it can fail the bridge, and none of it is said: the bridge's
//! stdout is the wire, and its stderr's last line is what the app shows
//! when a connection ends.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// How old an upload's temporary file is before it is taken as left over.
const LEFT_OVER: Duration = Duration::from_secs(60 * 60);

/// A file in the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub modified: SystemTime,
}

/// Clear the directory in the background, when this `perchd` is one the
/// app installed.
pub fn start() {
    let _ = std::thread::Builder::new()
        .name("cleanup".into())
        .spawn(|| {
            let _ = run();
        });
}

fn run() -> Option<()> {
    let exe = std::env::current_exe().ok()?;
    let own = exe.file_name()?.to_str()?.to_string();
    let dir = exe.parent()?;
    let bin = perch_github::config::perch_home().join("bin");
    if !own.starts_with("perchd-") || dir.canonicalize().ok()? != bin.canonicalize().ok()? {
        return None;
    }
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
    for name in to_remove(&own, &entries, SystemTime::now()) {
        let _ = std::fs::remove_file(Path::new(dir).join(name));
    }
    Some(())
}

/// What goes, of `entries`, for the bridge whose binary is `own`: each
/// other `perchd-*` but the newest, and each `.perchd-*` over an hour old.
pub fn to_remove(own: &str, entries: &[Entry], now: SystemTime) -> Vec<String> {
    let mut others: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.name.starts_with("perchd-") && entry.name != own)
        .collect();
    others.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.name.cmp(&a.name)));
    let mut gone: Vec<String> = others
        .into_iter()
        .skip(1)
        .map(|entry| entry.name.clone())
        .collect();
    gone.extend(
        entries
            .iter()
            .filter(|entry| entry.name.starts_with(".perchd-"))
            .filter(|entry| {
                now.duration_since(entry.modified)
                    .is_ok_and(|age| age > LEFT_OVER)
            })
            .map(|entry| entry.name.clone()),
    );
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_other_build_stays_and_old_uploads_go() {
        let now = SystemTime::now();
        let ago = |mins: u64| now - Duration::from_secs(mins * 60);
        let entry = |name: &str, mins| Entry {
            name: name.into(),
            modified: ago(mins),
        };
        let entries = [
            entry("perchd-0.9.0", 10),
            entry("perchd-0.8.0", 600),
            entry("perchd-0.8.1", 300),
            entry("perchd-0.9.0-0123456789ab", 30),
            entry(".perchd-0.9.0.4242", 5),
            entry(".perchd-0.8.1.17", 120),
            entry("notes.txt", 9000),
        ];
        let mut gone = to_remove("perchd-0.9.0", &entries, now);
        gone.sort();
        assert_eq!(gone, [".perchd-0.8.1.17", "perchd-0.8.0", "perchd-0.8.1"]);
        // Alone, or with one other, nothing goes.
        assert!(to_remove("perchd-0.9.0", &entries[..1], now).is_empty());
        assert!(to_remove("perchd-0.9.0", &entries[..2], now).is_empty());
        // A file dated ahead of the clock is not taken as old.
        let ahead = [Entry {
            name: ".perchd-x.1".into(),
            modified: now + Duration::from_secs(7200),
        }];
        assert!(to_remove("perchd-0.9.0", &ahead, now).is_empty());
    }
}
