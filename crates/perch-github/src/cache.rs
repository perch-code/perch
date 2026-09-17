//! Local SQLite cache.
//!
//! Exists so a cold start paints real rows before the network answers, and so
//! a per-thread detail is fetched once per `updated_at` rather than once per
//! launch. Avatars are cached as files rather than blobs, keyed by a hash of
//! their URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{SubjectDetail, Thread};

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
            "#,
        )?;
        Ok(Self { conn, avatar_dir })
    }

    /// Replace the cached inbox with exactly this set.
    ///
    /// Threads absent from `threads` are deleted: GitHub drops a thread from
    /// the list once it is done or unsubscribed, and the cache mirrors that
    /// rather than accumulating ghosts.
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
