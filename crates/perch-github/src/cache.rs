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

use crate::comments::{self, Anchor, Author, CommentMessage, CommentThread, RunRecord, RunStep};
use crate::items::{Checkout as ItemCheckout, Item, Project, Subject, SubjectKind};
use crate::model::{SubjectDetail, Thread};
use crate::pulls::PullRequest;

/// The shape of the tables. A database with an older number, or none, is
/// deleted and recreated: there is no migration, and nothing before the
/// number was written has shipped to anyone.
pub const SCHEMA_VERSION: u32 = 8;

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

/// A checkout Perch owns: a directory on a branch, with a record that
/// outlives the directory.
///
/// Keyed by repository and branch. A pull request maps to a branch, so it
/// is findable; a commit is a value the row carries, so a force-push updates
/// a row instead of making one. Archiving removes the directory and keeps
/// everything else, so restoring is a checkout rather than a clone.
#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    /// `<owner>/<repo>#<branch>`.
    pub id: String,
    pub repo_full_name: String,
    /// The local branch, such as `perch/pr-123`.
    pub branch: String,
    /// The branch the diff is taken against.
    pub base_ref: String,
    /// `None` for a worktree that has no pull request yet.
    pub pr_number: Option<u64>,
    /// What the branch pointed at the last time Perch looked.
    pub head_sha: String,
    /// Absolute. Where the checkout lives, or lived while archived. Stored
    /// rather than derived so the layout on disk can change without
    /// orphaning rows.
    pub path: PathBuf,
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
    /// `None` while the directory exists.
    pub archived_at: Option<Timestamp>,
    /// A snapshot, so the list can paint offline and before the network
    /// answers. Refreshed whenever the pull request is fetched.
    pub pull_request: Option<PullRequest>,
}

impl Worktree {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    /// The id of the worktree a pull request lives in.
    pub fn for_pull_request(repo_full_name: &str, number: u64) -> String {
        format!("{repo_full_name}#{}", Self::pr_branch(number))
    }

    /// The branch Perch puts a pull request on.
    pub fn pr_branch(number: u64) -> String {
        format!("perch/pr-{number}")
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
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

pub struct Cache {
    conn: Connection,
    avatar_dir: Option<PathBuf>,
}

impl Cache {
    pub fn open(db_path: &Path, avatar_dir: Option<PathBuf>) -> Result<Self, CacheError> {
        let (cache, _) = Self::open_checked(db_path, avatar_dir)?;
        Ok(cache)
    }

    /// Open, resetting a database whose schema is older than this build's.
    /// Returns whether it was reset, for the status line.
    pub fn open_checked(db_path: &Path, avatar_dir: Option<PathBuf>) -> Result<(Self, bool), CacheError> {
        let reset = db_path.exists() && !Self::is_current(db_path);
        if reset {
            tracing::info!(path = %db_path.display(), "schema is old; recreating the database");
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
            }
        }
        let conn = Connection::open(db_path)?;
        let cache = Self::from_conn(conn, avatar_dir)?;
        Ok((cache, reset))
    }

    /// Whether a database on disk carries this build's schema version.
    fn is_current(db_path: &Path) -> bool {
        let Ok(conn) = Connection::open(db_path) else {
            return false;
        };
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .optional()
            .unwrap_or(None);
        version.and_then(|v| v.parse::<u32>().ok()) == Some(SCHEMA_VERSION)
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
            CREATE TABLE IF NOT EXISTS worktrees (
                id              TEXT PRIMARY KEY,
                repo_full_name  TEXT NOT NULL,
                branch          TEXT NOT NULL,
                base_ref        TEXT NOT NULL,
                pr_number       INTEGER,
                head_sha        TEXT NOT NULL,
                path            TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                last_opened_at  TEXT NOT NULL,
                archived_at     TEXT,
                pr_json         TEXT
            );
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projects (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                repos_json    TEXT NOT NULL,
                roots_json    TEXT NOT NULL,
                implicit      INTEGER NOT NULL DEFAULT 0,
                created_at    TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS items (
                id               TEXT PRIMARY KEY,
                project_id       TEXT NOT NULL REFERENCES projects(id),
                repo_full_name   TEXT NOT NULL,
                subject_kind     TEXT,
                subject_number   INTEGER,
                subject_json     TEXT,
                branch           TEXT,
                base_ref         TEXT,
                head_sha         TEXT,
                checkout_path    TEXT,
                owned            INTEGER NOT NULL DEFAULT 1,
                on_disk          INTEGER NOT NULL DEFAULT 1,
                from_item_id     TEXT REFERENCES items(id),
                name             TEXT,
                saved            INTEGER NOT NULL DEFAULT 0,
                pinned_at        TEXT,
                done_at          TEXT,
                attention_at     TEXT,
                attention_reason TEXT,
                archived_at      TEXT,
                created_at       TEXT NOT NULL,
                last_opened_at   TEXT NOT NULL,
                additions        INTEGER,
                deletions        INTEGER,
                CHECK (subject_kind IS NOT NULL OR branch IS NOT NULL)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS items_subject
                ON items(repo_full_name, subject_kind, subject_number)
                WHERE subject_kind IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS items_branch
                ON items(repo_full_name, branch)
                WHERE branch IS NOT NULL;
            CREATE INDEX IF NOT EXISTS items_project ON items(project_id);
            CREATE TABLE IF NOT EXISTS comment_threads (
                id             TEXT PRIMARY KEY,
                worktree_id    TEXT NOT NULL,
                origin         TEXT NOT NULL,
                github_id      TEXT,
                path           TEXT,
                start_line     INTEGER,
                line           INTEGER,
                side           TEXT,
                commit_sha     TEXT,
                outdated       INTEGER NOT NULL DEFAULT 0,
                resolved       INTEGER NOT NULL DEFAULT 0,
                created_at     TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS comment_threads_worktree ON comment_threads(worktree_id);
            CREATE TABLE IF NOT EXISTS runs (
                id             TEXT PRIMARY KEY,
                thread_id      TEXT NOT NULL,
                backend        TEXT NOT NULL,
                model          TEXT NOT NULL,
                effort         TEXT NOT NULL,
                session_id     TEXT,
                status         TEXT NOT NULL,
                cost_usd       REAL,
                duration_ms    INTEGER,
                steps_json     TEXT NOT NULL DEFAULT '[]',
                error          TEXT,
                started_at     TEXT NOT NULL,
                ended_at       TEXT
            );
            CREATE TABLE IF NOT EXISTS comment_messages (
                id             TEXT PRIMARY KEY,
                thread_id      TEXT NOT NULL REFERENCES comment_threads(id) ON DELETE CASCADE,
                origin         TEXT NOT NULL,
                github_id      TEXT,
                author_kind    TEXT NOT NULL,
                author         TEXT,
                author_model   TEXT,
                body           TEXT NOT NULL,
                run_id         TEXT REFERENCES runs(id),
                created_at     TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS comment_messages_thread ON comment_messages(thread_id);
            CREATE TABLE IF NOT EXISTS marks (
                thread_id  TEXT PRIMARY KEY,
                saved      INTEGER NOT NULL DEFAULT 0,
                done_at    TEXT,
                marked_at  TEXT NOT NULL,
                json       TEXT NOT NULL
            );
            "#,
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
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

    pub fn load_worktrees(&self) -> Result<Vec<Worktree>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_full_name, branch, base_ref, pr_number, head_sha, path,
                    created_at, last_opened_at, archived_at, pr_json
             FROM worktrees",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (id, repo, branch, base_ref, pr_number, head_sha, path, created, opened, archived, pr_json) =
                row?;
            let (Ok(created_at), Ok(last_opened_at)) =
                (created.parse::<Timestamp>(), opened.parse::<Timestamp>())
            else {
                tracing::warn!(id, "skipping worktree row with unreadable timestamps");
                continue;
            };
            // A snapshot an older build wrote is not worth losing the row over.
            let pull_request = pr_json.and_then(|j| serde_json::from_str(&j).ok());
            out.push(Worktree {
                id,
                repo_full_name: repo,
                branch,
                base_ref,
                pr_number: pr_number.and_then(|n| u64::try_from(n).ok()),
                head_sha,
                path: PathBuf::from(path),
                created_at,
                last_opened_at,
                archived_at: archived.and_then(|a| a.parse().ok()),
                pull_request,
            });
        }
        Ok(out)
    }

    /// Insert a worktree, or replace every column of an existing one.
    pub fn upsert_worktree(&self, worktree: &Worktree) -> Result<(), CacheError> {
        let pr_json = match &worktree.pull_request {
            Some(pr) => Some(serde_json::to_string(pr)?),
            None => None,
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO worktrees
             (id, repo_full_name, branch, base_ref, pr_number, head_sha, path,
              created_at, last_opened_at, archived_at, pr_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                worktree.id,
                worktree.repo_full_name,
                worktree.branch,
                worktree.base_ref,
                worktree.pr_number.map(|n| n as i64),
                worktree.head_sha,
                worktree.path.to_string_lossy().into_owned(),
                worktree.created_at.to_string(),
                worktree.last_opened_at.to_string(),
                worktree.archived_at.map(|t| t.to_string()),
                pr_json,
            ],
        )?;
        Ok(())
    }

    pub fn touch_worktree(&self, id: &str, opened_at: Timestamp) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE worktrees SET last_opened_at = ?2 WHERE id = ?1",
            params![id, opened_at.to_string()],
        )?;
        Ok(())
    }

    pub fn set_worktree_archived(
        &self,
        id: &str,
        archived_at: Option<Timestamp>,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE worktrees SET archived_at = ?2 WHERE id = ?1",
            params![id, archived_at.map(|t| t.to_string())],
        )?;
        Ok(())
    }

    /// Record where the branch points now, and refresh the snapshot if the
    /// pull request was fetched along the way.
    pub fn set_worktree_head(
        &self,
        id: &str,
        head_sha: &str,
        pull_request: Option<&PullRequest>,
    ) -> Result<(), CacheError> {
        match pull_request {
            Some(pr) => {
                self.conn.execute(
                    "UPDATE worktrees SET head_sha = ?2, pr_json = ?3 WHERE id = ?1",
                    params![id, head_sha, serde_json::to_string(pr)?],
                )?;
            }
            None => {
                self.conn.execute(
                    "UPDATE worktrees SET head_sha = ?2 WHERE id = ?1",
                    params![id, head_sha],
                )?;
            }
        }
        Ok(())
    }

    /// Forget a worktree and everything said in it. Archiving keeps the
    /// threads; they are the point of keeping the record.
    pub fn forget_worktree(&self, id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        // Messages point at runs, so they go first.
        tx.execute(
            "DELETE FROM comment_messages WHERE thread_id IN (SELECT id FROM comment_threads WHERE worktree_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM runs WHERE thread_id IN (SELECT id FROM comment_threads WHERE worktree_id = ?1)",
            params![id],
        )?;
        tx.execute("DELETE FROM comment_threads WHERE worktree_id = ?1", params![id])?;
        tx.execute("DELETE FROM worktrees WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Comment threads
    // -----------------------------------------------------------------

    /// Every thread in a worktree, with its messages and their runs.
    pub fn load_comment_threads(&self, worktree_id: &str) -> Result<Vec<CommentThread>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, origin, path, start_line, line, side, commit_sha, outdated, resolved, created_at
             FROM comment_threads WHERE worktree_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![worktree_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, String>(9)?,
            ))
        })?;

        let mut threads = Vec::new();
        for row in rows {
            let (id, origin, path, start, line, side, sha, outdated, resolved, created) = row?;
            let Ok(created_at) = created.parse::<Timestamp>() else { continue };
            let anchor = match (path, start, line) {
                (Some(path), Some(start), Some(line)) => Some(Anchor {
                    path,
                    start_line: start as u32,
                    line: line as u32,
                    side: comments::sql::side(side.as_deref().unwrap_or("right")),
                    commit_sha: sha.unwrap_or_default(),
                }),
                _ => None,
            };
            let messages = self.load_messages(&id)?;
            threads.push(CommentThread {
                id,
                worktree_id: worktree_id.to_string(),
                origin: comments::sql::origin(&origin),
                anchor,
                outdated: outdated != 0,
                resolved: resolved != 0,
                created_at,
                messages,
            });
        }
        Ok(threads)
    }

    fn load_messages(&self, thread_id: &str) -> Result<Vec<CommentMessage>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, origin, author_kind, author, author_model, body, run_id, created_at
             FROM comment_messages WHERE thread_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map(params![thread_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, origin, kind, author, model, body, run_id, created) = row?;
            let Ok(created_at) = created.parse::<Timestamp>() else { continue };
            let run = match run_id {
                Some(run_id) => self.load_run(&run_id)?,
                None => None,
            };
            out.push(CommentMessage {
                id,
                thread_id: thread_id.to_string(),
                origin: comments::sql::origin(&origin),
                author: comments::sql::author(&kind, author, model),
                body,
                created_at,
                run,
            });
        }
        Ok(out)
    }

    pub fn load_run(&self, id: &str) -> Result<Option<RunRecord>, CacheError> {
        self.conn
            .query_row(
                "SELECT id, thread_id, backend, model, effort, session_id, status, cost_usd,
                        duration_ms, steps_json, error, started_at, ended_at
                 FROM runs WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<f64>>(7)?,
                        r.get::<_, Option<i64>>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                        r.get::<_, Option<String>>(12)?,
                    ))
                },
            )
            .optional()?
            .map(|(id, thread_id, backend, model, effort, session, status, cost, duration, steps, error, started, ended)| {
                let steps: Vec<RunStep> = serde_json::from_str(&steps).unwrap_or_default();
                Ok(RunRecord {
                    id,
                    thread_id,
                    backend,
                    model,
                    effort,
                    session_id: session,
                    status: comments::sql::status(&status),
                    cost_usd: cost,
                    duration_ms: duration.map(|d| d as u64),
                    steps,
                    error,
                    started_at: started.parse().map_err(|_| CacheError::Corrupt("run started_at".into()))?,
                    ended_at: ended.and_then(|e| e.parse().ok()),
                })
            })
            .transpose()
    }

    pub fn insert_comment_thread(&self, t: &CommentThread) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT INTO comment_threads
             (id, worktree_id, origin, path, start_line, line, side, commit_sha, outdated, resolved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                t.id,
                t.worktree_id,
                t.origin.name(),
                t.anchor.as_ref().map(|a| a.path.clone()),
                t.anchor.as_ref().map(|a| a.start_line as i64),
                t.anchor.as_ref().map(|a| a.line as i64),
                t.anchor.as_ref().map(|a| a.side.name()),
                t.anchor.as_ref().map(|a| a.commit_sha.clone()),
                t.outdated as i64,
                t.resolved as i64,
                t.created_at.to_string(),
            ],
        )?;
        for m in &t.messages {
            self.insert_comment_message(m)?;
        }
        Ok(())
    }

    /// Insert a message. A run it carries is inserted too if it is new.
    pub fn insert_comment_message(&self, m: &CommentMessage) -> Result<(), CacheError> {
        if let Some(run) = &m.run {
            if self.load_run(&run.id)?.is_none() {
                self.insert_run(run)?;
            }
        }
        let (kind, author) = comments::sql::author_columns(&m.author);
        let model = match &m.author {
            Author::Agent { model, .. } => Some(model.clone()),
            _ => None,
        };
        self.conn.execute(
            "INSERT INTO comment_messages
             (id, thread_id, origin, author_kind, author, author_model, body, run_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                m.id,
                m.thread_id,
                m.origin.name(),
                kind,
                author,
                model,
                m.body,
                m.run.as_ref().map(|r| r.id.clone()),
                m.created_at.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn insert_run(&self, r: &RunRecord) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT INTO runs
             (id, thread_id, backend, model, effort, session_id, status, cost_usd, duration_ms,
              steps_json, error, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                r.id,
                r.thread_id,
                r.backend,
                r.model,
                r.effort,
                r.session_id,
                r.status.name(),
                r.cost_usd,
                r.duration_ms.map(|d| d as i64),
                serde_json::to_string(&r.steps)?,
                r.error,
                r.started_at.to_string(),
                r.ended_at.map(|t| t.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Rewrite every column of a run that can move.
    pub fn update_run(&self, r: &RunRecord) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE runs SET session_id = ?2, status = ?3, cost_usd = ?4, duration_ms = ?5,
                             steps_json = ?6, error = ?7, ended_at = ?8
             WHERE id = ?1",
            params![
                r.id,
                r.session_id,
                r.status.name(),
                r.cost_usd,
                r.duration_ms.map(|d| d as i64),
                serde_json::to_string(&r.steps)?,
                r.error,
                r.ended_at.map(|t| t.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Point a message at the run that answered it, or clear it.
    pub fn set_message_run(&self, message_id: &str, run_id: Option<&str>) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE comment_messages SET run_id = ?2 WHERE id = ?1",
            params![message_id, run_id],
        )?;
        Ok(())
    }

    pub fn set_thread_outdated(&self, id: &str, outdated: bool) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE comment_threads SET outdated = ?2 WHERE id = ?1",
            params![id, outdated as i64],
        )?;
        Ok(())
    }

    pub fn delete_comment_thread(&self, id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM comment_messages WHERE thread_id = ?1", params![id])?;
        tx.execute("DELETE FROM runs WHERE thread_id = ?1", params![id])?;
        tx.execute("DELETE FROM comment_threads WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Mark every run still `running` as failed. For startup: such a row is
    /// a run Perch died in the middle of, and nothing reattaches.
    pub fn fail_running_runs(&self, reason: &str) -> Result<usize, CacheError> {
        let n = self.conn.execute(
            "UPDATE runs SET status = 'failed', error = ?1, ended_at = ?2 WHERE status = 'running'",
            params![reason, Timestamp::now().to_string()],
        )?;
        Ok(n)
    }

    // -----------------------------------------------------------------
    // Projects and items
    // -----------------------------------------------------------------

    pub fn load_projects(&self) -> Result<Vec<Project>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, repos_json, roots_json, implicit FROM projects ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, repos, roots, implicit) = row?;
            let repos: Vec<String> = serde_json::from_str(&repos).unwrap_or_default();
            let roots: Vec<String> = serde_json::from_str(&roots).unwrap_or_default();
            out.push(Project {
                id,
                name,
                repos,
                roots: roots.into_iter().map(PathBuf::from).collect(),
                implicit: implicit != 0,
            });
        }
        Ok(out)
    }

    pub fn upsert_project(&self, p: &Project) -> Result<(), CacheError> {
        let roots: Vec<String> = p.roots.iter().map(|r| r.to_string_lossy().into_owned()).collect();
        self.conn.execute(
            "INSERT INTO projects (id, name, repos_json, roots_json, implicit, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name, repos_json = excluded.repos_json,
                roots_json = excluded.roots_json, implicit = excluded.implicit",
            params![
                p.id,
                p.name,
                serde_json::to_string(&p.repos)?,
                serde_json::to_string(&roots)?,
                p.implicit as i64,
                Timestamp::now().to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<(), CacheError> {
        self.conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn load_items(&self) -> Result<Vec<Item>, CacheError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM items ORDER BY created_at"
        ))?;
        let rows = stmt.query_map([], read_item)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn load_item(&self, id: &str) -> Result<Option<Item>, CacheError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = ?1"),
                params![id],
                read_item,
            )
            .optional()?)
    }

    pub fn find_item_by_subject(
        &self,
        repo_full_name: &str,
        kind: SubjectKind,
        number: u64,
    ) -> Result<Option<Item>, CacheError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM items
                     WHERE repo_full_name = ?1 COLLATE NOCASE AND subject_kind = ?2 AND subject_number = ?3"
                ),
                params![repo_full_name, kind.column(), number as i64],
                read_item,
            )
            .optional()?)
    }

    pub fn find_item_by_branch(&self, repo_full_name: &str, branch: &str) -> Result<Option<Item>, CacheError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM items
                     WHERE repo_full_name = ?1 COLLATE NOCASE AND branch = ?2"
                ),
                params![repo_full_name, branch],
                read_item,
            )
            .optional()?)
    }

    /// Insert an item. One with neither a subject nor a checkout is
    /// refused, as is a second row for the same subject or branch.
    pub fn insert_item(&self, item: &Item) -> Result<(), CacheError> {
        if !item.is_valid() {
            return Err(CacheError::Corrupt("an item needs a subject or a checkout".into()));
        }
        let c = ItemColumns::of(item)?;
        self.conn.execute(
            "INSERT INTO items
             (id, project_id, repo_full_name, subject_kind, subject_number, subject_json,
              branch, base_ref, head_sha, checkout_path, owned, on_disk, from_item_id, name,
              saved, pinned_at, done_at, attention_at, attention_reason, archived_at,
              created_at, last_opened_at, additions, deletions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            params![
                item.id, item.project_id, item.repo_full_name,
                c.subject_kind, c.subject_number, c.subject_json,
                c.branch, c.base_ref, c.head_sha, c.checkout_path, c.owned, c.on_disk,
                item.from_item_id, item.name,
                item.saved as i64,
                item.pinned_at.map(|t| t.to_string()),
                item.done_at.map(|t| t.to_string()),
                item.attention_at.map(|t| t.to_string()),
                item.attention_reason,
                item.archived_at.map(|t| t.to_string()),
                item.created_at.to_string(),
                item.last_opened_at.to_string(),
                c.additions,
                c.deletions,
            ],
        )?;
        Ok(())
    }

    /// Rewrite every column of an item that can move.
    pub fn update_item(&self, item: &Item) -> Result<(), CacheError> {
        if !item.is_valid() {
            return Err(CacheError::Corrupt("an item needs a subject or a checkout".into()));
        }
        let c = ItemColumns::of(item)?;
        self.conn.execute(
            "UPDATE items SET
                project_id = ?2, repo_full_name = ?3, subject_kind = ?4, subject_number = ?5,
                subject_json = ?6, branch = ?7, base_ref = ?8, head_sha = ?9, checkout_path = ?10,
                owned = ?11, on_disk = ?12, from_item_id = ?13, name = ?14, saved = ?15,
                pinned_at = ?16, done_at = ?17, attention_at = ?18, attention_reason = ?19,
                archived_at = ?20, last_opened_at = ?21, additions = ?22, deletions = ?23
             WHERE id = ?1",
            params![
                item.id, item.project_id, item.repo_full_name,
                c.subject_kind, c.subject_number, c.subject_json,
                c.branch, c.base_ref, c.head_sha, c.checkout_path, c.owned, c.on_disk,
                item.from_item_id, item.name,
                item.saved as i64,
                item.pinned_at.map(|t| t.to_string()),
                item.done_at.map(|t| t.to_string()),
                item.attention_at.map(|t| t.to_string()),
                item.attention_reason,
                item.archived_at.map(|t| t.to_string()),
                item.last_opened_at.to_string(),
                c.additions,
                c.deletions,
            ],
        )?;
        Ok(())
    }

    /// Delete an item and everything said in it. Items cut from it keep
    /// their rows and lose the pointer.
    pub fn delete_item(&self, id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM comment_messages WHERE thread_id IN (SELECT id FROM comment_threads WHERE worktree_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM runs WHERE thread_id IN (SELECT id FROM comment_threads WHERE worktree_id = ?1)",
            params![id],
        )?;
        tx.execute("DELETE FROM comment_threads WHERE worktree_id = ?1", params![id])?;
        tx.execute("UPDATE items SET from_item_id = NULL WHERE from_item_id = ?1", params![id])?;
        tx.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_item_marks(&self, id: &str, saved: bool, done_at: Option<Timestamp>) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE items SET saved = ?2, done_at = ?3 WHERE id = ?1",
            params![id, saved as i64, done_at.map(|t| t.to_string())],
        )?;
        Ok(())
    }

    /// Set attention, only ever forward: an older notification arriving
    /// late cannot rewind it, and cannot change the reason either.
    pub fn touch_attention(&self, id: &str, at: Timestamp, reason: &str) -> Result<bool, CacheError> {
        let current: Option<String> = self
            .conn
            .query_row("SELECT attention_at FROM items WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?
            .flatten();
        let current = current.and_then(|c| c.parse::<Timestamp>().ok());
        if current.is_some_and(|c| c >= at) {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE items SET attention_at = ?2, attention_reason = ?3 WHERE id = ?1",
            params![id, at.to_string(), reason],
        )?;
        Ok(true)
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

const ITEM_COLUMNS: &str = "id, project_id, repo_full_name, subject_kind, subject_number, subject_json,
    branch, base_ref, head_sha, checkout_path, owned, on_disk, from_item_id, name, saved,
    pinned_at, done_at, attention_at, attention_reason, archived_at, created_at, last_opened_at,
    additions, deletions";

/// An item's optional parts as columns.
struct ItemColumns {
    subject_kind: Option<&'static str>,
    subject_number: Option<i64>,
    subject_json: Option<String>,
    branch: Option<String>,
    base_ref: Option<String>,
    head_sha: Option<String>,
    checkout_path: Option<String>,
    owned: i64,
    on_disk: i64,
    additions: Option<i64>,
    deletions: Option<i64>,
}

impl ItemColumns {
    fn of(item: &Item) -> Result<Self, CacheError> {
        let subject_json = match &item.subject {
            Some(s) => Some(serde_json::to_string(s)?),
            None => None,
        };
        let c = item.checkout.as_ref();
        Ok(Self {
            subject_kind: item.subject.as_ref().map(|s| s.kind().column()),
            subject_number: item.subject.as_ref().map(|s| s.number() as i64),
            subject_json,
            branch: c.map(|c| c.branch.clone()),
            base_ref: c.map(|c| c.base_ref.clone()),
            head_sha: c.map(|c| c.head_sha.clone()),
            checkout_path: c.map(|c| c.path.to_string_lossy().into_owned()),
            owned: c.map(|c| c.owned as i64).unwrap_or(1),
            on_disk: c.map(|c| c.on_disk as i64).unwrap_or(1),
            additions: c.and_then(|c| c.churn).map(|(a, _)| a as i64),
            deletions: c.and_then(|c| c.churn).map(|(_, d)| d as i64),
        })
    }
}

fn stamp(s: Option<String>) -> Option<Timestamp> {
    s.and_then(|s| s.parse().ok())
}

fn read_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let subject_json: Option<String> = r.get(5)?;
    // A snapshot an older build wrote is dropped rather than failing the
    // row; the item still has its branch, or is invalid and skipped later.
    let subject: Option<Subject> = subject_json.and_then(|j| serde_json::from_str(&j).ok());
    let branch: Option<String> = r.get(6)?;
    let checkout = branch.map(|branch| ItemCheckout {
        branch,
        base_ref: r.get::<_, Option<String>>(7).ok().flatten().unwrap_or_default(),
        head_sha: r.get::<_, Option<String>>(8).ok().flatten().unwrap_or_default(),
        path: PathBuf::from(r.get::<_, Option<String>>(9).ok().flatten().unwrap_or_default()),
        owned: r.get::<_, i64>(10).unwrap_or(1) != 0,
        on_disk: r.get::<_, i64>(11).unwrap_or(1) != 0,
        churn: match (r.get::<_, Option<i64>>(22).ok().flatten(), r.get::<_, Option<i64>>(23).ok().flatten()) {
            (Some(a), Some(d)) => Some((a as u32, d as u32)),
            _ => None,
        },
    });
    let created: String = r.get(20)?;
    let opened: String = r.get(21)?;
    Ok(Item {
        id: r.get(0)?,
        project_id: r.get(1)?,
        repo_full_name: r.get(2)?,
        subject,
        checkout,
        from_item_id: r.get(12)?,
        name: r.get(13)?,
        saved: r.get::<_, i64>(14)? != 0,
        pinned_at: stamp(r.get(15)?),
        done_at: stamp(r.get(16)?),
        attention_at: stamp(r.get(17)?),
        attention_reason: r.get(18)?,
        archived_at: stamp(r.get(19)?),
        created_at: created.parse().unwrap_or(Timestamp::UNIX_EPOCH),
        last_opened_at: opened.parse().unwrap_or(Timestamp::UNIX_EPOCH),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{RunStatus, Side, ThreadOrigin};
    use crate::items::{Subject as ItemSubject, SubjectKind as ItemKind};
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
            workflow: None,
            html_url: None,
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

    fn pull_request(number: u64) -> PullRequest {
        PullRequest {
            number,
            title: "Add a thing".into(),
            author_login: "octocat".into(),
            author_avatar_url: "https://avatars/1".into(),
            head_sha: "abc123".into(),
            head_ref: "feature".into(),
            base_sha: "def456".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 10,
            deletions: 2,
            changed_files: 3,
            repo_full_name: "acme/widgets".into(),
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
        }
    }

    fn worktree(number: u64) -> Worktree {
        let now: Timestamp = "2026-09-18T10:00:00Z".parse().unwrap();
        Worktree {
            id: Worktree::for_pull_request("acme/widgets", number),
            repo_full_name: "acme/widgets".into(),
            branch: Worktree::pr_branch(number),
            base_ref: "main".into(),
            pr_number: Some(number),
            head_sha: "abc123".into(),
            path: PathBuf::from("/tmp/perch/worktrees/acme__widgets__perch-pr-7"),
            created_at: now,
            last_opened_at: now,
            archived_at: None,
            pull_request: Some(pull_request(number)),
        }
    }

    #[test]
    fn the_id_is_repository_and_branch_not_number() {
        assert_eq!(Worktree::for_pull_request("acme/widgets", 7), "acme/widgets#perch/pr-7");
        assert_eq!(Worktree::pr_branch(7), "perch/pr-7");
    }

    #[test]
    fn worktrees_round_trip_through_upsert_and_load() {
        let c = Cache::open_in_memory().unwrap();
        let w = worktree(7);
        c.upsert_worktree(&w).unwrap();
        assert_eq!(c.load_worktrees().unwrap(), vec![w]);
    }

    #[test]
    fn upserting_the_same_id_replaces_rather_than_duplicates() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        let mut moved = worktree(7);
        moved.head_sha = "fff999".into();
        c.upsert_worktree(&moved).unwrap();

        let all = c.load_worktrees().unwrap();
        assert_eq!(all.len(), 1, "a force-push updates a row, it does not make one");
        assert_eq!(all[0].head_sha, "fff999");
    }

    #[test]
    fn archiving_is_a_nullable_timestamp() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        let when: Timestamp = "2026-09-19T12:00:00Z".parse().unwrap();

        c.set_worktree_archived(&worktree(7).id, Some(when)).unwrap();
        let w = &c.load_worktrees().unwrap()[0];
        assert!(w.is_archived());
        assert_eq!(w.archived_at, Some(when));

        // Restoring clears it; the row and everything else stay.
        c.set_worktree_archived(&worktree(7).id, None).unwrap();
        let w = &c.load_worktrees().unwrap()[0];
        assert!(!w.is_archived());
        assert_eq!(w.branch, "perch/pr-7");
    }

    #[test]
    fn touching_moves_only_the_opened_time() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        let later: Timestamp = "2026-09-20T08:00:00Z".parse().unwrap();
        c.touch_worktree(&worktree(7).id, later).unwrap();
        let w = &c.load_worktrees().unwrap()[0];
        assert_eq!(w.last_opened_at, later);
        assert_eq!(w.created_at, worktree(7).created_at);
    }

    #[test]
    fn setting_the_head_refreshes_the_snapshot_when_one_is_given() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        let mut pr = pull_request(7);
        pr.title = "Renamed on the web".into();
        pr.head_sha = "fff999".into();

        c.set_worktree_head(&worktree(7).id, "fff999", Some(&pr)).unwrap();
        let w = &c.load_worktrees().unwrap()[0];
        assert_eq!(w.head_sha, "fff999");
        assert_eq!(w.pull_request.as_ref().unwrap().title, "Renamed on the web");

        // Without a pull request the snapshot is left as it was.
        c.set_worktree_head(&worktree(7).id, "aaa111", None).unwrap();
        let w = &c.load_worktrees().unwrap()[0];
        assert_eq!(w.head_sha, "aaa111");
        assert_eq!(w.pull_request.as_ref().unwrap().title, "Renamed on the web");
    }

    #[test]
    fn forgetting_removes_the_row() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        c.upsert_worktree(&worktree(8)).unwrap();
        c.forget_worktree(&worktree(7).id).unwrap();
        let ids: Vec<String> = c.load_worktrees().unwrap().into_iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![worktree(8).id]);
    }

    #[test]
    fn the_snapshot_paints_with_no_network_and_no_threads() {
        // Nothing but the worktrees table is populated, as on an offline
        // launch: the row still carries enough to draw the list.
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        assert!(c.load_threads().unwrap().is_empty());
        let w = &c.load_worktrees().unwrap()[0];
        let pr = w.pull_request.as_ref().expect("snapshot");
        assert_eq!(pr.title, "Add a thing");
        assert_eq!(pr.number, 7);
    }

    #[test]
    fn a_worktree_without_a_pull_request_is_allowed() {
        let c = Cache::open_in_memory().unwrap();
        let mut w = worktree(7);
        w.id = "acme/widgets#perch/scratch".into();
        w.branch = "perch/scratch".into();
        w.pr_number = None;
        w.pull_request = None;
        c.upsert_worktree(&w).unwrap();
        assert_eq!(c.load_worktrees().unwrap(), vec![w]);
    }

    #[test]
    fn storing_threads_leaves_worktrees_alone() {
        let mut c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        c.store_threads(&[]).unwrap();
        assert_eq!(c.load_worktrees().unwrap().len(), 1);
    }

    fn stamp(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn run(id: &str, thread: &str, status: RunStatus) -> RunRecord {
        RunRecord {
            id: id.into(),
            thread_id: thread.into(),
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "medium".into(),
            session_id: Some("sess".into()),
            status,
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            steps: vec![
                RunStep { summary: "Read src/x.rs".into(), ok: true },
                RunStep { summary: "Asked to edit src/x.rs".into(), ok: false },
            ],
            error: None,
            started_at: stamp("2026-09-18T10:00:00Z"),
            ended_at: Some(stamp("2026-09-18T10:00:14Z")),
        }
    }

    fn comment_thread(id: &str, worktree: &str) -> CommentThread {
        CommentThread {
            id: id.into(),
            worktree_id: worktree.into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: 41,
                line: 58,
                side: Side::Right,
                commit_sha: "abc".into(),
            }),
            outdated: false,
            resolved: false,
            created_at: stamp("2026-09-18T09:59:00Z"),
            messages: vec![
                CommentMessage {
                    id: format!("{id}-m1"),
                    thread_id: id.into(),
                    origin: ThreadOrigin::Local,
                    author: Author::User,
                    body: "why?".into(),
                    created_at: stamp("2026-09-18T09:59:00Z"),
                    run: None,
                },
                CommentMessage {
                    id: format!("{id}-m2"),
                    thread_id: id.into(),
                    origin: ThreadOrigin::Local,
                    author: Author::Agent { backend: "claude-code".into(), model: "sonnet".into() },
                    body: "because".into(),
                    created_at: stamp("2026-09-18T10:00:14Z"),
                    run: Some(run(&format!("{id}-r1"), id, RunStatus::Finished)),
                },
            ],
        }
    }

    #[test]
    fn a_thread_with_messages_and_a_run_loads_back_whole() {
        let c = Cache::open_in_memory().unwrap();
        let t = comment_thread("t1", "acme/widgets#perch/pr-7");
        c.insert_comment_thread(&t).unwrap();
        let loaded = c.load_comment_threads("acme/widgets#perch/pr-7").unwrap();
        assert_eq!(loaded, vec![t]);
        assert!(c.load_comment_threads("other").unwrap().is_empty());
    }

    #[test]
    fn steps_round_trip_through_update() {
        let c = Cache::open_in_memory().unwrap();
        let t = comment_thread("t1", "w");
        c.insert_comment_thread(&t).unwrap();
        let mut r = run("t1-r1", "t1", RunStatus::Finished);
        r.steps.push(RunStep { summary: "Ran git log".into(), ok: true });
        r.cost_usd = Some(0.05);
        c.update_run(&r).unwrap();
        let loaded = c.load_run("t1-r1").unwrap().unwrap();
        assert_eq!(loaded.steps.len(), 3);
        assert_eq!(loaded, r);
    }

    #[test]
    fn a_thread_without_an_anchor_is_allowed() {
        let c = Cache::open_in_memory().unwrap();
        let mut t = comment_thread("t1", "w");
        t.anchor = None;
        t.messages.clear();
        c.insert_comment_thread(&t).unwrap();
        assert_eq!(c.load_comment_threads("w").unwrap()[0].anchor, None);
    }

    #[test]
    fn fail_running_runs_touches_only_running_rows() {
        let c = Cache::open_in_memory().unwrap();
        let mut t = comment_thread("t1", "w");
        t.messages[1].run = Some(run("r-running", "t1", RunStatus::Running));
        c.insert_comment_thread(&t).unwrap();
        c.insert_run(&run("r-done", "t1", RunStatus::Finished)).unwrap();

        assert_eq!(c.fail_running_runs("Perch quit").unwrap(), 1);
        let failed = c.load_run("r-running").unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("Perch quit"));
        assert!(failed.ended_at.is_some());
        assert_eq!(c.load_run("r-done").unwrap().unwrap().status, RunStatus::Finished);
    }

    #[test]
    fn forgetting_a_worktree_removes_its_threads_and_runs() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        c.insert_comment_thread(&comment_thread("t1", &worktree(7).id)).unwrap();
        c.insert_comment_thread(&comment_thread("t2", "elsewhere")).unwrap();

        c.forget_worktree(&worktree(7).id).unwrap();
        assert!(c.load_comment_threads(&worktree(7).id).unwrap().is_empty());
        assert!(c.load_run("t1-r1").unwrap().is_none());
        assert_eq!(c.load_comment_threads("elsewhere").unwrap().len(), 1);
    }

    #[test]
    fn archiving_keeps_threads() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_worktree(&worktree(7)).unwrap();
        c.insert_comment_thread(&comment_thread("t1", &worktree(7).id)).unwrap();
        c.set_worktree_archived(&worktree(7).id, Some(stamp("2026-09-19T00:00:00Z"))).unwrap();
        assert_eq!(c.load_comment_threads(&worktree(7).id).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_thread_takes_its_messages_and_runs() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        c.set_thread_outdated("t1", true).unwrap();
        assert!(c.load_comment_threads("w").unwrap()[0].outdated);
        c.delete_comment_thread("t1").unwrap();
        assert!(c.load_comment_threads("w").unwrap().is_empty());
        assert!(c.load_run("t1-r1").unwrap().is_none());
    }

    #[test]
    fn a_follow_up_resumes_the_last_finished_run() {
        let t = comment_thread("t1", "w");
        assert_eq!(t.resumable_session("claude-code").as_deref(), Some("sess"));
        let mut failed = t.clone();
        failed.messages[1].run.as_mut().unwrap().status = RunStatus::Failed;
        assert_eq!(failed.resumable_session("claude-code"), None);
    }

    fn project(name: &str) -> Project {
        Project {
            id: Project::id_for(name),
            name: name.into(),
            repos: vec!["acme/widgets".into()],
            roots: vec![PathBuf::from("/tmp/widgets")],
            implicit: false,
        }
    }

    fn pr_item(number: u64) -> Item {
        let mut i = Item::new("acme", "acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        i.id = format!("item-pr-{number}");
        i.subject = Some(ItemSubject::PullRequest(pull_request(number)));
        i
    }

    fn branch_item(branch: &str) -> Item {
        let mut i = Item::new("acme", "acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        i.id = format!("item-{branch}");
        i.checkout = Some(ItemCheckout {
            branch: branch.into(),
            base_ref: "main".into(),
            head_sha: "abc".into(),
            path: PathBuf::from(format!("/tmp/wt/{branch}")),
            owned: true,
            on_disk: true,
            churn: None,
        });
        i
    }

    fn stamp_of(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn a_fresh_database_carries_the_schema_version() {
        let c = Cache::open_in_memory().unwrap();
        assert_eq!(c.meta("schema_version").unwrap().as_deref(), Some(&*SCHEMA_VERSION.to_string()));
    }

    #[test]
    fn a_database_with_no_version_is_recreated_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE worktrees (id TEXT PRIMARY KEY);
                 INSERT INTO worktrees VALUES ('old');",
            )
            .unwrap();
        }
        let (c, reset) = Cache::open_checked(&path, None).unwrap();
        assert!(reset);
        let n: i64 = c
            .conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE name = 'items'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "the new tables exist");
        let old: i64 = c
            .conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE name = 'worktrees' AND sql LIKE '%id TEXT PRIMARY KEY)%'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(old, 0, "the old table is gone");
    }

    #[test]
    fn a_database_with_an_old_version_is_recreated_and_a_current_one_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let (c, reset) = Cache::open_checked(&path, None).unwrap();
            assert!(!reset, "a new file is not a reset");
            c.upsert_project(&project("acme")).unwrap();
            c.set_meta("schema_version", "6").unwrap();
        }
        let (c, reset) = Cache::open_checked(&path, None).unwrap();
        assert!(reset);
        assert!(c.load_projects().unwrap().is_empty());
        c.upsert_project(&project("acme")).unwrap();
        drop(c);
        let (c, reset) = Cache::open_checked(&path, None).unwrap();
        assert!(!reset, "a current database is kept");
        assert_eq!(c.load_projects().unwrap().len(), 1);
    }

    #[test]
    fn projects_round_trip_and_upsert_replaces() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_project(&project("Acme")).unwrap();
        let mut p = project("Acme");
        p.roots.push(PathBuf::from("/tmp/other"));
        p.implicit = true;
        c.upsert_project(&p).unwrap();
        let all = c.load_projects().unwrap();
        assert_eq!(all, vec![p]);
        c.delete_project("acme").unwrap();
        assert!(c.load_projects().unwrap().is_empty());
    }

    #[test]
    fn items_round_trip_whole() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_project(&project("acme")).unwrap();
        let mut i = pr_item(7);
        i.checkout = Some(ItemCheckout {
            branch: "perch/pr-7".into(),
            base_ref: "main".into(),
            head_sha: "abc".into(),
            path: PathBuf::from("/tmp/wt/pr-7"),
            owned: true,
            on_disk: false,
            churn: None,
        });
        i.name = Some("the fence".into());
        i.saved = true;
        i.pinned_at = Some(stamp_of("2026-09-18T11:00:00Z"));
        i.attention_at = Some(stamp_of("2026-09-18T12:00:00Z"));
        i.attention_reason = Some("review_requested".into());
        c.insert_item(&i).unwrap();
        assert_eq!(c.load_items().unwrap(), vec![i.clone()]);
        assert_eq!(c.load_item(&i.id).unwrap(), Some(i.clone()));
        assert_eq!(c.find_item_by_subject("ACME/widgets", ItemKind::PullRequest, 7).unwrap(), Some(i.clone()));
        assert_eq!(c.find_item_by_branch("acme/widgets", "perch/pr-7").unwrap(), Some(i.clone()));
        assert_eq!(c.find_item_by_subject("acme/widgets", ItemKind::Issue, 7).unwrap(), None);

        i.archived_at = Some(stamp_of("2026-09-19T00:00:00Z"));
        i.project_id = "other".into();
        c.upsert_project(&project("other")).unwrap();
        c.update_item(&i).unwrap();
        assert_eq!(c.load_items().unwrap(), vec![i]);
    }

    #[test]
    fn a_second_row_per_subject_or_branch_is_refused() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_project(&project("acme")).unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut dup = pr_item(7);
        dup.id = "another".into();
        assert!(c.insert_item(&dup).is_err(), "one row per pull request");

        c.insert_item(&branch_item("feature")).unwrap();
        let mut dup = branch_item("feature");
        dup.id = "another-branch".into();
        assert!(c.insert_item(&dup).is_err(), "one row per branch");

        // A row may be both, and a different branch is fine.
        let mut both = pr_item(8);
        both.checkout = branch_item("feature-8").checkout;
        c.insert_item(&both).unwrap();
    }

    #[test]
    fn an_item_with_neither_part_is_refused() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_project(&project("acme")).unwrap();
        let empty = Item::new("acme", "acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        assert!(c.insert_item(&empty).is_err());
        assert!(c.load_items().unwrap().is_empty());
    }

    #[test]
    fn touch_attention_only_moves_forward() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_project(&project("acme")).unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let id = pr_item(7).id;
        assert!(c.touch_attention(&id, stamp_of("2026-09-18T12:00:00Z"), "mention").unwrap());
        assert!(!c.touch_attention(&id, stamp_of("2026-09-18T11:00:00Z"), "comment").unwrap(), "older is ignored");
        assert!(!c.touch_attention(&id, stamp_of("2026-09-18T12:00:00Z"), "comment").unwrap(), "equal is ignored");
        let i = c.load_item(&id).unwrap().unwrap();
        assert_eq!(i.attention_at, Some(stamp_of("2026-09-18T12:00:00Z")));
        assert_eq!(i.attention_reason.as_deref(), Some("mention"));
        assert!(c.touch_attention(&id, stamp_of("2026-09-18T13:00:00Z"), "comment").unwrap());
        assert_eq!(c.load_item(&id).unwrap().unwrap().attention_reason.as_deref(), Some("comment"));
    }

    #[test]
    fn marks_and_deletion() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_project(&project("acme")).unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut cut = branch_item("perch/pr-7-2");
        cut.from_item_id = Some(pr_item(7).id);
        c.insert_item(&cut).unwrap();
        c.set_item_marks(&pr_item(7).id, true, Some(stamp_of("2026-09-18T12:00:00Z"))).unwrap();
        let i = c.load_item(&pr_item(7).id).unwrap().unwrap();
        assert!(i.saved);
        assert_eq!(i.done_at, Some(stamp_of("2026-09-18T12:00:00Z")));

        c.delete_item(&pr_item(7).id).unwrap();
        assert!(c.load_item(&pr_item(7).id).unwrap().is_none());
        let cut = c.load_item(&cut.id).unwrap().unwrap();
        assert_eq!(cut.from_item_id, None, "the pointer is cleared, the row stays");
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
