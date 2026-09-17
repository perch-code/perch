//! Local SQLite cache.
//!
//! Exists so a cold start paints real rows before the network answers, and so
//! a per-thread detail is fetched once per `updated_at` rather than once per
//! launch. Avatars are cached as files rather than blobs, keyed by a hash of
//! their URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{SubjectDetail, Thread};

/// Perch's own opinion about a thread: bookmarked, dismissed, or both.
///
/// Local only. Neither flag exists in the REST API, so nothing here is ever
/// sent to GitHub and nothing GitHub sends can set it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub thread_id: String,
    pub saved: bool,
    /// The thread's own `updated_at` at the moment it was marked done, and
    /// `None` when it is not done.
    ///
    /// Storing the thread's timestamp rather than a boolean is what makes
    /// resurfacing one comparison: activity newer than this clears the mark.
    pub done_at: Option<Timestamp>,
    /// Wall clock, for ordering and for making sense of a stale database.
    pub marked_at: Timestamp,
    /// The thread as it was when last marked or reconciled.
    ///
    /// Saved and Done have to keep listing a thread after GitHub stops
    /// returning it, which it does after a few months. The snapshot is what
    /// they render then.
    pub snapshot: Thread,
}

impl Mark {
    pub fn is_done(&self) -> bool {
        self.done_at.is_some()
    }
}

/// Which flag a write is setting. The other one is left alone.
#[derive(Debug, Clone, Copy)]
enum Field {
    Saved(bool),
    Done(bool),
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Cache {
    conn: Connection,
    avatar_dir: Option<PathBuf>,
}

impl Cache {
    pub fn open(db_path: &Path, avatar_dir: Option<PathBuf>) -> Result<Self, CacheError> {
        let conn = Connection::open(db_path)?;
        Self::from_conn(conn, avatar_dir)
    }

    /// In-memory cache, used by tests.
    pub fn open_in_memory() -> Result<Self, CacheError> {
        Self::from_conn(Connection::open_in_memory()?, None)
    }

    fn from_conn(conn: Connection, avatar_dir: Option<PathBuf>) -> Result<Self, CacheError> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS threads (
                id          TEXT PRIMARY KEY,
                updated_at  TEXT NOT NULL,
                json        TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS subject_details (
                thread_id   TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                json        TEXT NOT NULL,
                PRIMARY KEY (thread_id, updated_at)
            );
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS marks (
                thread_id  TEXT PRIMARY KEY,
                saved      INTEGER NOT NULL DEFAULT 0,
                done_at    TEXT,
                marked_at  TEXT NOT NULL,
                json       TEXT NOT NULL
            );
            "#,
        )?;
        Ok(Self { conn, avatar_dir })
    }

    /// Replace the cached inbox with exactly this set.
    ///
    /// Threads absent from `threads` are deleted, so the cache mirrors the
    /// API rather than accumulating ghosts.
    ///
    /// Note what the API does *not* drop. Marking a thread Done on the web
    /// removes it from github.com's inbox, but the REST list has no concept
    /// of Done: the thread keeps coming back as an ordinary read
    /// notification for months. Perch therefore receives a superset of the
    /// web inbox, and narrows it with its own marks, which this table knows
    /// nothing about. `marks` is deliberately untouched here.
    pub fn store_threads(&mut self, threads: &[Thread]) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM threads", [])?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO threads (id, updated_at, json) VALUES (?1, ?2, ?3)")?;
            for t in threads {
                stmt.execute(params![
                    t.id,
                    t.updated_at.to_string(),
                    serde_json::to_string(t)?
                ])?;
            }
        }
        // Drop details whose thread is gone, and details for older revisions
        // of threads we kept.
        tx.execute(
            "DELETE FROM subject_details WHERE (thread_id, updated_at) NOT IN
             (SELECT id, updated_at FROM threads)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_threads(&self) -> Result<Vec<Thread>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT json FROM threads")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            match serde_json::from_str(&row?) {
                Ok(t) => out.push(t),
                // A cached row written by an older build is not worth failing
                // the whole load over.
                Err(e) => tracing::warn!(error = %e, "skipping unreadable cached thread"),
            }
        }
        Ok(out)
    }

    pub fn store_detail(
        &self,
        thread_id: &str,
        updated_at: &str,
        detail: &SubjectDetail,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO subject_details (thread_id, updated_at, json)
             VALUES (?1, ?2, ?3)",
            params![thread_id, updated_at, serde_json::to_string(detail)?],
        )?;
        Ok(())
    }

    pub fn load_detail(
        &self,
        thread_id: &str,
        updated_at: &str,
    ) -> Result<Option<SubjectDetail>, CacheError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM subject_details WHERE thread_id = ?1 AND updated_at = ?2",
                params![thread_id, updated_at],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(serde_json::from_str(&j).ok()),
            None => Ok(None),
        }
    }

    /// Every cached detail, keyed by thread id, for the threads we still hold.
    pub fn load_all_details(&self) -> Result<HashMap<String, SubjectDetail>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT d.thread_id, d.json FROM subject_details d
             JOIN threads t ON t.id = d.thread_id AND t.updated_at = d.updated_at",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, json) = row?;
            if let Ok(d) = serde_json::from_str(&json) {
                out.insert(id, d);
            }
        }
        Ok(out)
    }

    /// Every mark, including ones whose thread the API no longer returns.
    pub fn load_marks(&self) -> Result<Vec<Mark>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT thread_id, saved, done_at, marked_at, json FROM marks")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (thread_id, saved, done_at, marked_at, json) = row?;
            let snapshot: Thread = match serde_json::from_str(&json) {
                Ok(t) => t,
                // Same bargain as load_threads: a row written by an older
                // build should not fail the whole load.
                Err(e) => {
                    tracing::warn!(thread_id, error = %e, "skipping unreadable mark snapshot");
                    continue;
                }
            };
            out.push(Mark {
                thread_id,
                saved,
                done_at: done_at.and_then(|d| d.parse().ok()),
                marked_at: marked_at.parse().unwrap_or(snapshot.updated_at),
                snapshot,
            });
        }
        Ok(out)
    }

    /// Save or unsave every thread given, in one transaction.
    pub fn set_saved(&mut self, threads: &[&Thread], saved: bool) -> Result<(), CacheError> {
        self.write_marks(threads, Field::Saved(saved))
    }

    /// Mark done, or restore, every thread given, in one transaction.
    pub fn set_done(&mut self, threads: &[&Thread], done: bool) -> Result<(), CacheError> {
        self.write_marks(threads, Field::Done(done))
    }

    /// One flag, many threads, one transaction.
    ///
    /// Clearing the last flag on a row removes the row: a mark that says
    /// neither saved nor done is just a snapshot nobody asked for.
    fn write_marks(&mut self, threads: &[&Thread], field: Field) -> Result<(), CacheError> {
        if threads.is_empty() {
            return Ok(());
        }

        // One timestamp for the whole batch, so a bulk action reads as one
        // act rather than as n acts a microsecond apart.
        let now = Timestamp::now().to_string();
        let tx = self.conn.transaction()?;
        {
            // The insert carries both columns; the update touches only the
            // one being set, so saving a done thread leaves it done.
            let sql = match field {
                Field::Saved(_) => {
                    "INSERT INTO marks (thread_id, saved, done_at, marked_at, json)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(thread_id) DO UPDATE SET
                         saved = ?2, marked_at = ?4, json = ?5"
                }
                Field::Done(_) => {
                    "INSERT INTO marks (thread_id, saved, done_at, marked_at, json)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(thread_id) DO UPDATE SET
                         done_at = ?3, marked_at = ?4, json = ?5"
                }
            };
            let mut stmt = tx.prepare(sql)?;
            for t in threads {
                let saved = matches!(field, Field::Saved(true));
                let done_at = match field {
                    Field::Done(true) => Some(t.updated_at.to_string()),
                    _ => None,
                };
                stmt.execute(params![
                    t.id,
                    saved,
                    done_at,
                    now,
                    serde_json::to_string(t)?
                ])?;
            }
        }
        tx.execute("DELETE FROM marks WHERE saved = 0 AND done_at IS NULL", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Bring marks back in line with a freshly fetched inbox.
    ///
    /// Two jobs. A thread whose activity is newer than the moment it was
    /// marked done stops being done, which is how a dismissed pull request
    /// comes back when somebody comments on it. And any mark whose thread
    /// the API still returns gets a fresh snapshot, so the copy Saved and
    /// Done fall back on does not rot.
    ///
    /// Saved is left alone: bookmarks are not supposed to expire.
    ///
    /// Returns whether anything moved, so the caller can skip an event on
    /// the common poll where nothing did.
    pub fn reconcile_marks(&mut self, threads: &[Thread]) -> Result<bool, CacheError> {
        let live: HashMap<&str, &Thread> = threads.iter().map(|t| (t.id.as_str(), t)).collect();
        let mut changed = false;

        let tx = self.conn.transaction()?;
        {
            let mut existing = Vec::new();
            {
                let mut stmt = tx.prepare("SELECT thread_id, done_at, json FROM marks")?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
                for row in rows {
                    existing.push(row?);
                }
            }

            let mut update =
                tx.prepare("UPDATE marks SET done_at = ?2, json = ?3 WHERE thread_id = ?1")?;
            for (thread_id, done_at, json) in existing {
                let Some(thread) = live.get(thread_id.as_str()) else {
                    // Aged out of the API. The snapshot is all there is, and
                    // it is already correct.
                    continue;
                };

                let resurfaced = match done_at.as_deref() {
                    Some(d) => match d.parse::<Timestamp>() {
                        Ok(d) => thread.updated_at > d,
                        // An unreadable timestamp cannot be compared against,
                        // so let the thread back into the inbox rather than
                        // hiding it forever.
                        Err(_) => true,
                    },
                    None => false,
                };

                let fresh = serde_json::to_string(thread)?;
                if resurfaced || fresh != json {
                    let new_done = if resurfaced { None } else { done_at };
                    update.execute(params![thread_id, new_done, fresh])?;
                    changed = true;
                }
            }
        }

        // A thread that resurfaced and was never saved has nothing left.
        if tx.execute("DELETE FROM marks WHERE saved = 0 AND done_at IS NULL", [])? > 0 {
            changed = true;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, CacheError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn avatar_path(&self, url: &str) -> Option<PathBuf> {
        Some(self.avatar_dir.as_ref()?.join(format!("{}.img", hash(url))))
    }

    pub fn load_avatar(&self, url: &str) -> Option<Vec<u8>> {
        std::fs::read(self.avatar_path(url)?).ok()
    }

    pub fn store_avatar(&self, url: &str, bytes: &[u8]) -> Result<(), CacheError> {
        if let Some(path) = self.avatar_path(url) {
            std::fs::write(path, bytes)?;
        }
        Ok(())
    }
}

/// FNV-1a. Only needs to give each URL a stable, collision-unlikely filename.
fn hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Reason, Repository, Subject, SubjectKind, SubjectState};

    fn thread(id: &str, updated: &str) -> Thread {
        Thread {
            id: id.into(),
            unread: true,
            reason: Reason::Subscribed,
            updated_at: updated.parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: format!("title {id}"),
                kind: SubjectKind::PullRequest,
                url: Some(format!("https://api.github.com/repos/a/b/pulls/{id}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "a/b".into(),
                owner_avatar_url: "https://avatars/1".into(),
            },
        }
    }

    fn detail(n: u64) -> SubjectDetail {
        SubjectDetail {
            number: n,
            state: SubjectState::PrOpen,
            author_login: "octocat".into(),
            author_avatar_url: "https://avatars/2".into(),
        }
    }

    #[test]
    fn threads_round_trip() {
        let mut c = Cache::open_in_memory().unwrap();
        let threads = vec![thread("1", "2026-09-17T18:00:00Z"), thread("2", "2026-09-16T18:00:00Z")];
        c.store_threads(&threads).unwrap();

        let mut loaded = c.load_threads().unwrap();
        loaded.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(loaded, threads);
    }

    #[test]
    fn storing_threads_removes_ones_github_dropped() {
        let mut c = Cache::open_in_memory().unwrap();
        c.store_threads(&[thread("1", "2026-09-17T18:00:00Z"), thread("2", "2026-09-17T18:00:00Z")])
            .unwrap();
        c.store_threads(&[thread("1", "2026-09-17T18:00:00Z")]).unwrap();

        let loaded = c.load_threads().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "1");
    }

    #[test]
    fn detail_is_keyed_by_thread_revision() {
        let mut c = Cache::open_in_memory().unwrap();
        c.store_threads(&[thread("1", "2026-09-17T18:00:00Z")]).unwrap();
        c.store_detail("1", "2026-09-17T18:00:00Z", &detail(7)).unwrap();

        assert_eq!(
            c.load_detail("1", "2026-09-17T18:00:00Z").unwrap(),
            Some(detail(7))
        );
        // A newer revision of the same thread is a cache miss, which is what
        // forces a refetch when a PR's state may have changed.
        assert_eq!(c.load_detail("1", "2026-09-18T09:00:00Z").unwrap(), None);
    }

    #[test]
    fn stale_details_are_pruned_when_a_thread_moves_on() {
        let mut c = Cache::open_in_memory().unwrap();
        c.store_threads(&[thread("1", "2026-09-17T18:00:00Z")]).unwrap();
        c.store_detail("1", "2026-09-17T18:00:00Z", &detail(7)).unwrap();

        c.store_threads(&[thread("1", "2026-09-18T09:00:00Z")]).unwrap();
        assert!(c.load_all_details().unwrap().is_empty());
    }

    #[test]
    fn details_join_only_current_revisions() {
        let mut c = Cache::open_in_memory().unwrap();
        c.store_threads(&[thread("1", "2026-09-17T18:00:00Z")]).unwrap();
        c.store_detail("1", "2026-09-17T18:00:00Z", &detail(7)).unwrap();

        let all = c.load_all_details().unwrap();
        assert_eq!(all.get("1"), Some(&detail(7)));
    }

    fn marks_by_id(c: &Cache) -> HashMap<String, Mark> {
        c.load_marks()
            .unwrap()
            .into_iter()
            .map(|m| (m.thread_id.clone(), m))
            .collect()
    }

    #[test]
    fn saving_and_unsaving_a_thread() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");

        c.set_saved(&[&t], true).unwrap();
        let marks = c.load_marks().unwrap();
        assert_eq!(marks.len(), 1);
        assert!(marks[0].saved);
        assert!(!marks[0].is_done());
        assert_eq!(marks[0].snapshot, t);

        c.set_saved(&[&t], false).unwrap();
        assert!(c.load_marks().unwrap().is_empty());
    }

    #[test]
    fn marking_done_records_the_threads_own_timestamp() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");

        c.set_done(&[&t], true).unwrap();
        let marks = c.load_marks().unwrap();
        assert_eq!(marks[0].done_at, Some(t.updated_at));
        assert!(!marks[0].saved);

        c.set_done(&[&t], false).unwrap();
        assert!(c.load_marks().unwrap().is_empty());
    }

    #[test]
    fn the_two_flags_are_independent() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");

        c.set_saved(&[&t], true).unwrap();
        c.set_done(&[&t], true).unwrap();
        let m = &c.load_marks().unwrap()[0];
        assert!(m.saved && m.is_done());

        // Restoring leaves the bookmark behind.
        c.set_done(&[&t], false).unwrap();
        let m = &c.load_marks().unwrap()[0];
        assert!(m.saved && !m.is_done());
    }

    #[test]
    fn a_batch_marks_every_thread_in_it() {
        let mut c = Cache::open_in_memory().unwrap();
        let (a, b, d) = (
            thread("1", "2026-09-17T18:00:00Z"),
            thread("2", "2026-09-16T18:00:00Z"),
            thread("3", "2026-09-15T18:00:00Z"),
        );

        c.set_done(&[&a, &b, &d], true).unwrap();
        let marks = marks_by_id(&c);
        assert_eq!(marks.len(), 3);
        assert!(marks.values().all(|m| m.is_done()));
        // One batch, one moment.
        assert_eq!(marks["1"].marked_at, marks["3"].marked_at);
    }

    #[test]
    fn an_empty_batch_is_a_no_op() {
        let mut c = Cache::open_in_memory().unwrap();
        c.set_done(&[], true).unwrap();
        c.set_saved(&[], false).unwrap();
        assert!(c.load_marks().unwrap().is_empty());
    }

    #[test]
    fn marks_outlive_a_thread_github_stopped_returning() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.store_threads(std::slice::from_ref(&t)).unwrap();
        c.set_saved(&[&t], true).unwrap();

        // The inbox moves on without it.
        c.store_threads(&[]).unwrap();
        assert!(c.load_threads().unwrap().is_empty());

        let marks = c.load_marks().unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].snapshot, t);
    }

    #[test]
    fn reconcile_clears_done_when_the_thread_moves_on() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.set_done(&[&t], true).unwrap();

        let newer = thread("1", "2026-09-18T09:00:00Z");
        assert!(c.reconcile_marks(&[newer]).unwrap());
        // Nothing left to remember, so the row is gone.
        assert!(c.load_marks().unwrap().is_empty());
    }

    #[test]
    fn reconcile_leaves_saved_alone_when_a_thread_moves_on() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.set_saved(&[&t], true).unwrap();
        c.set_done(&[&t], true).unwrap();

        let newer = thread("1", "2026-09-18T09:00:00Z");
        assert!(c.reconcile_marks(std::slice::from_ref(&newer)).unwrap());

        let marks = c.load_marks().unwrap();
        assert_eq!(marks.len(), 1);
        assert!(marks[0].saved);
        assert!(!marks[0].is_done());
        assert_eq!(marks[0].snapshot, newer);
    }

    #[test]
    fn reconcile_keeps_done_when_nothing_happened() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.set_done(&[&t], true).unwrap();

        assert!(!c.reconcile_marks(std::slice::from_ref(&t)).unwrap());
        assert!(c.load_marks().unwrap()[0].is_done());
    }

    #[test]
    fn reconcile_refreshes_the_snapshot_of_a_live_thread() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.set_saved(&[&t], true).unwrap();

        // Same revision, but read on the web since.
        let mut read = t.clone();
        read.unread = false;
        assert!(c.reconcile_marks(std::slice::from_ref(&read)).unwrap());
        assert_eq!(c.load_marks().unwrap()[0].snapshot, read);
    }

    #[test]
    fn reconcile_leaves_a_thread_the_api_no_longer_returns() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.set_done(&[&t], true).unwrap();

        assert!(!c.reconcile_marks(&[]).unwrap());
        let marks = c.load_marks().unwrap();
        assert!(marks[0].is_done());
        assert_eq!(marks[0].snapshot, t);
    }

    #[test]
    fn marking_done_twice_keeps_the_first_timestamp_current() {
        let mut c = Cache::open_in_memory().unwrap();
        let t = thread("1", "2026-09-17T18:00:00Z");
        c.set_done(&[&t], true).unwrap();

        // Re-marking an already done thread against newer activity moves the
        // watermark forward, so it does not immediately resurface.
        let newer = thread("1", "2026-09-18T09:00:00Z");
        c.set_done(&[&newer], true).unwrap();
        assert_eq!(c.load_marks().unwrap()[0].done_at, Some(newer.updated_at));
        assert!(!c.reconcile_marks(&[newer]).unwrap());
    }

    #[test]
    fn meta_round_trips() {
        let c = Cache::open_in_memory().unwrap();
        assert_eq!(c.meta("last_modified").unwrap(), None);
        c.set_meta("last_modified", "Wed, 17 Sep 2026 18:00:00 GMT").unwrap();
        assert_eq!(
            c.meta("last_modified").unwrap().as_deref(),
            Some("Wed, 17 Sep 2026 18:00:00 GMT")
        );
    }

    #[test]
    fn avatar_hash_is_stable_and_distinct() {
        assert_eq!(hash("https://a"), hash("https://a"));
        assert_ne!(hash("https://a"), hash("https://b"));
    }
}
