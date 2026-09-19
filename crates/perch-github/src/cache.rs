//! Local SQLite cache.
//!
//! Exists so a cold start paints real rows before the network answers, and
//! so what the reader marked and what was fetched for an item survive a
//! launch. Avatars are cached as files rather than blobs, keyed by a hash
//! of their URL.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::{params, Connection, OptionalExtension};

use crate::comments::{self, Anchor, Author, CommentMessage, CommentThread, RunRecord, RunStep};
use crate::discussion::Discussion;
use crate::items::{Checkout as ItemCheckout, Item, Subject, SubjectKind};
use crate::workflow_runs::Checks;

/// The shape of the tables. A database with an older number, or none, is
/// deleted and recreated: there is no migration, and nothing before the
/// number was written has shipped to anyone.
pub const SCHEMA_VERSION: u32 = 13;

/// An item's reveals as the store holds them: the head line ranges the
/// reader opened out of a fold, inclusive, by `(path, blob)`.
pub type StoredReveals = HashMap<(String, String), Vec<(u32, u32)>>;

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
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS items (
                id               TEXT PRIMARY KEY,
                repo_full_name   TEXT NOT NULL,
                subject_kind     TEXT,
                subject_number   INTEGER,
                subject_json     TEXT,
                subject_repo     TEXT,
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
                last_seen_head   TEXT,
                CHECK (subject_kind IS NOT NULL OR branch IS NOT NULL)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS items_subject
                ON items(subject_repo, subject_kind, subject_number)
                WHERE subject_kind IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS items_branch
                ON items(repo_full_name, branch)
                WHERE branch IS NOT NULL;
            CREATE INDEX IF NOT EXISTS items_repo ON items(repo_full_name COLLATE NOCASE);
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
            CREATE TABLE IF NOT EXISTS discussions (
                item_id     TEXT PRIMARY KEY,
                fetched_at  TEXT NOT NULL,
                json        TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS checks (
                item_id     TEXT PRIMARY KEY,
                head_sha    TEXT NOT NULL,
                fetched_at  TEXT NOT NULL,
                json        TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS viewed (
                item_id    TEXT NOT NULL,
                path       TEXT NOT NULL,
                blob       TEXT NOT NULL,
                viewed_at  TEXT NOT NULL,
                PRIMARY KEY (item_id, path, blob)
            );
            CREATE TABLE IF NOT EXISTS reveals (
                item_id    TEXT NOT NULL,
                path       TEXT NOT NULL,
                blob       TEXT NOT NULL,
                first      INTEGER NOT NULL,
                last       INTEGER NOT NULL,
                PRIMARY KEY (item_id, path, blob, first, last)
            );
            "#,
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(Self { conn, avatar_dir })
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
    // Items
    // -----------------------------------------------------------------

    /// Every repository with an item in it, archived ones included: one
    /// entry per repository whatever the case its items spell it in, in the
    /// spelling of the oldest item, ordered by name without regard to case.
    /// With the declared list this is what the sidebar sections over.
    pub fn repos(&self) -> Result<Vec<String>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT repo_full_name FROM items ORDER BY created_at, rowid")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            let name = row?;
            if seen.insert(name.to_lowercase()) {
                out.push(name);
            }
        }
        out.sort_by_cached_key(|n| n.to_lowercase());
        Ok(out)
    }

    /// The kind, number, and item id of every item whose subject is open
    /// in a repository, by number. The list pass compares this with what
    /// GitHub lists as open to learn what closed. The repository is the
    /// subject's, as `find_item_by_subject` takes it, so a branch on a fork
    /// counts for the upstream its pull request is in. Openness is read
    /// from the parsed subject rather than asked of the JSON, so the rule
    /// stays `Subject::is_open`'s.
    pub fn open_subjects_in(&self, repo_full_name: &str) -> Result<Vec<(SubjectKind, u64, String)>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject_json FROM items
             WHERE subject_repo = ?1 COLLATE NOCASE AND subject_kind IS NOT NULL
             ORDER BY subject_number",
        )?;
        let rows = stmt.query_map(params![repo_full_name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, json) = row?;
            let Some(subject) = json.and_then(|j| serde_json::from_str::<Subject>(&j).ok()) else {
                tracing::warn!(id, "skipping an item with an unreadable subject");
                continue;
            };
            if subject.is_open() {
                out.push((subject.kind(), subject.number(), id));
            }
        }
        Ok(out)
    }

    /// The items whose kept discussion is missing, or was fetched before the
    /// subject's own last activity, newest subject first, so the backfill
    /// archives what is moving before what is not. A pull request whose
    /// `updated_at` is unknown is stale only when there is no row: with
    /// nothing to compare against, no fetch could ever be the last one.
    /// The comparison is made on parsed timestamps: `fetched_at` carries
    /// fractions of a second and GitHub's times do not, and as strings the
    /// two misorder within a second.
    pub fn discussion_stale(&self) -> Result<Vec<String>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT i.id, i.subject_json, d.fetched_at FROM items i
             LEFT JOIN discussions d ON d.item_id = i.id
             WHERE i.subject_kind IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut stale: Vec<(Option<Timestamp>, String)> = Vec::new();
        for row in rows {
            let (id, json, fetched_at) = row?;
            let Some(subject) = json.and_then(|j| serde_json::from_str::<Subject>(&j).ok()) else {
                continue;
            };
            let updated_at = subject.updated_at();
            // An unreadable `fetched_at` is treated as no row: refetching is
            // the only way to get one that reads.
            let fetched_at = fetched_at.and_then(|f| f.parse::<Timestamp>().ok());
            let is_stale = match (fetched_at, updated_at) {
                (None, _) => true,
                (Some(fetched), Some(updated)) => fetched < updated,
                (Some(_), None) => false,
            };
            if is_stale {
                stale.push((updated_at, id));
            }
        }
        // Newest first, an unknown time last, and by id between equals so
        // the order is the same on every call.
        stale.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(stale.into_iter().map(|(_, id)| id).collect())
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
                     WHERE subject_repo = ?1 COLLATE NOCASE AND subject_kind = ?2 AND subject_number = ?3"
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
             (id, repo_full_name, subject_kind, subject_number, subject_json,
              branch, base_ref, head_sha, checkout_path, owned, on_disk, from_item_id, name,
              saved, pinned_at, done_at, attention_at, attention_reason, archived_at,
              created_at, last_opened_at, additions, deletions, subject_repo, last_seen_head)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            params![
                item.id, item.repo_full_name,
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
                c.subject_repo,
                c.last_seen_head,
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
                repo_full_name = ?2, subject_kind = ?3, subject_number = ?4,
                subject_json = ?5, branch = ?6, base_ref = ?7, head_sha = ?8, checkout_path = ?9,
                owned = ?10, on_disk = ?11, from_item_id = ?12, name = ?13, saved = ?14,
                pinned_at = ?15, done_at = ?16, attention_at = ?17, attention_reason = ?18,
                archived_at = ?19, last_opened_at = ?20, additions = ?21, deletions = ?22,
                subject_repo = ?23, last_seen_head = ?24
             WHERE id = ?1",
            params![
                item.id, item.repo_full_name,
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
                c.subject_repo,
                c.last_seen_head,
            ],
        )?;
        Ok(())
    }

    /// Delete an item and everything said in it, and what was fetched and
    /// marked for it. Items cut from it keep their rows and lose the
    /// pointer.
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
        tx.execute("DELETE FROM discussions WHERE item_id = ?1", params![id])?;
        tx.execute("DELETE FROM checks WHERE item_id = ?1", params![id])?;
        tx.execute("DELETE FROM viewed WHERE item_id = ?1", params![id])?;
        tx.execute("DELETE FROM reveals WHERE item_id = ?1", params![id])?;
        tx.execute("UPDATE items SET from_item_id = NULL WHERE from_item_id = ?1", params![id])?;
        tx.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // What was fetched for an item, and what the reader marked on it
    // -----------------------------------------------------------------

    /// Keep the discussion, replacing whatever was kept before. One row per
    /// item, so Conversation can paint before the network answers.
    pub fn store_discussion(&self, item_id: &str, discussion: &Discussion) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO discussions (item_id, fetched_at, json) VALUES (?1, ?2, ?3)",
            params![item_id, Timestamp::now().to_string(), serde_json::to_string(discussion)?],
        )?;
        Ok(())
    }

    /// The kept discussion and when it was fetched. A row an older build
    /// wrote that no longer reads is `None`, the same as no row.
    pub fn load_discussion(&self, item_id: &str) -> Result<Option<(Timestamp, Discussion)>, CacheError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT fetched_at, json FROM discussions WHERE item_id = ?1",
                params![item_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((fetched_at, json)) = row else { return Ok(None) };
        let (Ok(fetched_at), Ok(discussion)) =
            (fetched_at.parse::<Timestamp>(), serde_json::from_str::<Discussion>(&json))
        else {
            tracing::warn!(item_id, "skipping unreadable cached discussion");
            return Ok(None);
        };
        Ok(Some((fetched_at, discussion)))
    }

    /// Keep the checks for the head they were read at, replacing whatever
    /// was kept before.
    pub fn store_checks(&self, item_id: &str, head_sha: &str, checks: &Checks) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO checks (item_id, head_sha, fetched_at, json) VALUES (?1, ?2, ?3, ?4)",
            params![item_id, head_sha, Timestamp::now().to_string(), serde_json::to_string(checks)?],
        )?;
        Ok(())
    }

    /// The kept checks, only if they were read at `head_sha`. Checks for
    /// another head say nothing about this one, so they are a miss and the
    /// caller fetches.
    pub fn load_checks(&self, item_id: &str, head_sha: &str) -> Result<Option<Checks>, CacheError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM checks WHERE item_id = ?1 AND head_sha = ?2",
                params![item_id, head_sha],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Mark a file's content viewed, or not. The key is the blob, not the
    /// commit: a push that changes the file gives it a blob with no row,
    /// and a file that comes back to a viewed blob is viewed again.
    pub fn set_viewed(&self, item_id: &str, path: &str, blob: &str, viewed: bool) -> Result<(), CacheError> {
        if viewed {
            self.conn.execute(
                "INSERT OR REPLACE INTO viewed (item_id, path, blob, viewed_at) VALUES (?1, ?2, ?3, ?4)",
                params![item_id, path, blob, Timestamp::now().to_string()],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM viewed WHERE item_id = ?1 AND path = ?2 AND blob = ?3",
                params![item_id, path, blob],
            )?;
        }
        Ok(())
    }

    /// Every `(path, blob)` marked viewed on an item.
    pub fn load_viewed(&self, item_id: &str) -> Result<HashSet<(String, String)>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT path, blob FROM viewed WHERE item_id = ?1")?;
        let rows = stmt.query_map(params![item_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Remember that the reader opened head lines `first` to `last` of a
    /// file's content, inclusive. Keyed by the blob as Viewed is, for the
    /// same reason: a push that changes the file starts it folded again,
    /// and a file that comes back to the blob opens as it was left. Asking
    /// for the same range twice is one row.
    pub fn set_reveal(&self, item_id: &str, path: &str, blob: &str, first: u32, last: u32) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO reveals (item_id, path, blob, first, last) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![item_id, path, blob, first, last],
        )?;
        Ok(())
    }

    /// Forget every reveal on a file's content that overlaps `first` to
    /// `last`, inclusive: a divider's Reset, which folds its run back to
    /// the default by taking every reveal the run has a piece of.
    pub fn remove_reveals(&self, item_id: &str, path: &str, blob: &str, first: u32, last: u32) -> Result<(), CacheError> {
        self.conn.execute(
            "DELETE FROM reveals WHERE item_id = ?1 AND path = ?2 AND blob = ?3 AND first <= ?5 AND last >= ?4",
            params![item_id, path, blob, first, last],
        )?;
        Ok(())
    }

    /// Every reveal on an item, by `(path, blob)`, each file's in order.
    pub fn load_reveals(&self, item_id: &str) -> Result<StoredReveals, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, blob, first, last FROM reveals WHERE item_id = ?1 ORDER BY path, blob, first, last")?;
        let rows = stmt.query_map(params![item_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?, r.get::<_, u32>(3)?))
        })?;
        let mut out = StoredReveals::new();
        for row in rows {
            let (path, blob, first, last) = row?;
            out.entry((path, blob)).or_default().push((first, last));
        }
        Ok(out)
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

    /// Forget a key. Absent reads the same as never set, which is what a
    /// list pass wants when an endpoint stops sending an ETag.
    pub fn delete_meta(&self, key: &str) -> Result<(), CacheError> {
        self.conn.execute("DELETE FROM meta WHERE key = ?1", params![key])?;
        Ok(())
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

const ITEM_COLUMNS: &str = "id, repo_full_name, subject_kind, subject_number, subject_json,
    branch, base_ref, head_sha, checkout_path, owned, on_disk, from_item_id, name, saved,
    pinned_at, done_at, attention_at, attention_reason, archived_at, created_at, last_opened_at,
    additions, deletions, last_seen_head";

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
    subject_repo: Option<String>,
    last_seen_head: Option<String>,
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
            subject_repo: item.subject.as_ref().map(|s| s.repo_full_name().to_lowercase()),
            last_seen_head: c.and_then(|c| c.last_seen_head.clone()),
        })
    }
}

fn stamp(s: Option<String>) -> Option<Timestamp> {
    s.and_then(|s| s.parse().ok())
}

fn read_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let subject_json: Option<String> = r.get(4)?;
    // A snapshot an older build wrote is dropped rather than failing the
    // row; the item still has its branch, or is invalid and skipped later.
    let subject: Option<Subject> = subject_json.and_then(|j| serde_json::from_str(&j).ok());
    let branch: Option<String> = r.get(5)?;
    let checkout = branch.map(|branch| ItemCheckout {
        branch,
        base_ref: r.get::<_, Option<String>>(6).ok().flatten().unwrap_or_default(),
        head_sha: r.get::<_, Option<String>>(7).ok().flatten().unwrap_or_default(),
        path: PathBuf::from(r.get::<_, Option<String>>(8).ok().flatten().unwrap_or_default()),
        owned: r.get::<_, i64>(9).unwrap_or(1) != 0,
        on_disk: r.get::<_, i64>(10).unwrap_or(1) != 0,
        churn: match (r.get::<_, Option<i64>>(21).ok().flatten(), r.get::<_, Option<i64>>(22).ok().flatten()) {
            (Some(a), Some(d)) => Some((a as u32, d as u32)),
            _ => None,
        },
        last_seen_head: r.get::<_, Option<String>>(23).ok().flatten(),
    });
    let created: String = r.get(19)?;
    let opened: String = r.get(20)?;
    Ok(Item {
        id: r.get(0)?,
        repo_full_name: r.get(1)?,
        subject,
        checkout,
        from_item_id: r.get(11)?,
        name: r.get(12)?,
        saved: r.get::<_, i64>(13)? != 0,
        pinned_at: stamp(r.get(14)?),
        done_at: stamp(r.get(15)?),
        attention_at: stamp(r.get(16)?),
        attention_reason: r.get(17)?,
        archived_at: stamp(r.get(18)?),
        created_at: created.parse().unwrap_or(Timestamp::UNIX_EPOCH),
        last_opened_at: opened.parse().unwrap_or(Timestamp::UNIX_EPOCH),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{RunStatus, Side, ThreadOrigin};
    use crate::items::{Subject as ItemSubject, SubjectKind as ItemKind};
    use crate::pulls::PullRequest;

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
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        }
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
    fn deleting_an_item_takes_its_threads_and_runs() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let id = pr_item(7).id;
        c.insert_comment_thread(&comment_thread("t1", &id)).unwrap();
        c.insert_comment_thread(&comment_thread("t2", "elsewhere")).unwrap();

        c.delete_item(&id).unwrap();
        assert!(c.load_comment_threads(&id).unwrap().is_empty());
        assert!(c.load_run("t1-r1").unwrap().is_none());
        assert_eq!(c.load_comment_threads("elsewhere").unwrap().len(), 1);
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

    fn pr_item(number: u64) -> Item {
        let mut i = Item::new("acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        i.id = format!("item-pr-{number}");
        i.subject = Some(ItemSubject::PullRequest(pull_request(number)));
        i
    }

    fn branch_item(branch: &str) -> Item {
        let mut i = Item::new("acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        i.id = format!("item-{branch}");
        i.checkout = Some(ItemCheckout {
            branch: branch.into(),
            base_ref: "main".into(),
            head_sha: "abc".into(),
            path: PathBuf::from(format!("/tmp/wt/{branch}")),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        i
    }

    fn issue_item(number: u64, state: &str, updated: &str) -> Item {
        let mut i = Item::new("acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        i.id = format!("item-issue-{number}");
        i.subject = Some(ItemSubject::Issue(crate::items::Issue {
            number,
            title: "t".into(),
            body: String::new(),
            state: state.into(),
            state_reason: None,
            author_login: String::new(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: stamp_of(updated),
            repo_full_name: "acme/widgets".into(),
        }));
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
                 CREATE TABLE threads (id TEXT PRIMARY KEY);
                 INSERT INTO threads VALUES ('old');",
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
            .query_row("SELECT count(*) FROM sqlite_master WHERE name = 'threads'", [], |r| r.get(0))
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
            c.insert_item(&pr_item(7)).unwrap();
            c.set_meta("schema_version", "6").unwrap();
        }
        let (c, reset) = Cache::open_checked(&path, None).unwrap();
        assert!(reset);
        assert!(c.load_items().unwrap().is_empty());
        c.insert_item(&pr_item(7)).unwrap();
        drop(c);
        let (c, reset) = Cache::open_checked(&path, None).unwrap();
        assert!(!reset, "a current database is kept");
        assert_eq!(c.load_items().unwrap().len(), 1);
    }

    #[test]
    fn the_schema_has_no_project_table_and_indexes_items_by_repository() {
        let c = Cache::open_in_memory().unwrap();
        let count = |sql: &str| -> i64 { c.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT count(*) FROM sqlite_master WHERE name = 'projects'"), 0);
        assert_eq!(count("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'items_project'"), 0);
        assert_eq!(count("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'items_repo'"), 1);
        assert_eq!(count("SELECT count(*) FROM pragma_table_info('items') WHERE name = 'project_id'"), 0);
    }

    #[test]
    fn repos_lists_each_repository_with_an_item_once_in_name_order() {
        let c = Cache::open_in_memory().unwrap();
        assert!(c.repos().unwrap().is_empty());
        c.insert_item(&pr_item(7)).unwrap();
        let mut fork = branch_item("feature");
        fork.repo_full_name = "Zed/widgets".into();
        c.insert_item(&fork).unwrap();
        let mut archived = branch_item("old");
        archived.repo_full_name = "beta/things".into();
        archived.archived_at = Some(stamp_of("2026-09-19T00:00:00Z"));
        c.insert_item(&archived).unwrap();
        let mut spelled = branch_item("other");
        spelled.repo_full_name = "ACME/Widgets".into();
        spelled.created_at = stamp_of("2026-09-18T11:00:00Z");
        c.insert_item(&spelled).unwrap();
        assert_eq!(
            c.repos().unwrap(),
            vec!["acme/widgets", "beta/things", "Zed/widgets"],
            "one entry per repository in its oldest item's spelling, archived included, ordered without regard to case"
        );
    }

    #[test]
    fn items_round_trip_whole() {
        let c = Cache::open_in_memory().unwrap();
        let mut i = pr_item(7);
        i.checkout = Some(ItemCheckout {
            branch: "perch/pr-7".into(),
            base_ref: "main".into(),
            head_sha: "abc".into(),
            path: PathBuf::from("/tmp/wt/pr-7"),
            owned: true,
            on_disk: false,
            churn: None,
            last_seen_head: None,
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
        i.repo_full_name = "acme/widgets-fork".into();
        c.update_item(&i).unwrap();
        assert_eq!(c.load_items().unwrap(), vec![i]);
    }

    #[test]
    fn a_subject_is_found_by_its_own_repository_not_the_checkouts() {
        // A branch on a fork with a pull request against the upstream: the
        // item is the fork's, the pull request is upstream's, and a
        // notification names the upstream.
        let c = Cache::open_in_memory().unwrap();
        let mut i = branch_item("feature");
        i.repo_full_name = "chris/widgets".into();
        i.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        c.insert_item(&i).unwrap();
        assert_eq!(c.find_item_by_subject("acme/widgets", ItemKind::PullRequest, 7).unwrap().map(|i| i.id), Some(i.id.clone()));
        assert_eq!(c.find_item_by_subject("chris/widgets", ItemKind::PullRequest, 7).unwrap(), None);
    }

    #[test]
    fn a_second_row_per_subject_or_branch_is_refused() {
        let c = Cache::open_in_memory().unwrap();
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
        let empty = Item::new("acme/widgets", stamp_of("2026-09-18T10:00:00Z"));
        assert!(c.insert_item(&empty).is_err());
        assert!(c.load_items().unwrap().is_empty());
    }

    #[test]
    fn touch_attention_only_moves_forward() {
        let c = Cache::open_in_memory().unwrap();
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
        c.insert_item(&pr_item(7)).unwrap();
        let mut cut = branch_item("perch/pr-7-2");
        cut.from_item_id = Some(pr_item(7).id);
        c.insert_item(&cut).unwrap();
        // Saved and Done are columns of the row, written the way every
        // other edit is.
        let mut marked = pr_item(7);
        marked.saved = true;
        marked.done_at = Some(stamp_of("2026-09-18T12:00:00Z"));
        c.update_item(&marked).unwrap();
        let i = c.load_item(&pr_item(7).id).unwrap().unwrap();
        assert!(i.saved);
        assert_eq!(i.done_at, Some(stamp_of("2026-09-18T12:00:00Z")));

        c.delete_item(&pr_item(7).id).unwrap();
        assert!(c.load_item(&pr_item(7).id).unwrap().is_none());
        let cut = c.load_item(&cut.id).unwrap().unwrap();
        assert_eq!(cut.from_item_id, None, "the pointer is cleared, the row stays");
    }

    #[test]
    fn the_last_seen_head_is_a_column_of_its_own() {
        let c = Cache::open_in_memory().unwrap();
        let mut i = branch_item("feature");
        c.insert_item(&i).unwrap();
        assert_eq!(c.load_item(&i.id).unwrap().unwrap().checkout.unwrap().last_seen_head, None);

        i.checkout.as_mut().unwrap().last_seen_head = Some("abc".into());
        i.checkout.as_mut().unwrap().head_sha = "def".into();
        c.update_item(&i).unwrap();
        let checkout = c.load_item(&i.id).unwrap().unwrap().checkout.unwrap();
        assert_eq!(checkout.last_seen_head.as_deref(), Some("abc"));
        assert_eq!(checkout.head_sha, "def");
    }

    fn discussion() -> Discussion {
        use crate::discussion::{IssueComment, PullEvent, PullEventKind, Review, ReviewComment};
        Discussion {
            opened_at: Some(stamp_of("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![IssueComment {
                id: 1,
                user: "chris".into(),
                user_avatar_url: "https://avatars/1".into(),
                body: "q".into(),
                created_at: stamp_of("2026-09-15T09:14:00Z"),
                updated_at: None,
                html_url: "https://github.com/acme/widgets/pull/7#issuecomment-1".into(),
            }],
            reviews: vec![Review {
                id: 9,
                user: "rohan".into(),
                body: "nice".into(),
                state: "APPROVED".into(),
                submitted_at: Some(stamp_of("2026-09-16T11:40:00Z")),
                html_url: String::new(),
            }],
            review_comments: vec![ReviewComment {
                id: 3,
                pull_request_review_id: Some(9),
                in_reply_to_id: None,
                user: "rohan".into(),
                body: "epoch".into(),
                path: "src/manifest.rs".into(),
                line: Some(41),
                original_line: None,
                diff_hunk: "@@ -40,3 +40,3 @@".into(),
                side: Side::Right,
                created_at: stamp_of("2026-09-16T11:38:00Z"),
                updated_at: None,
                html_url: String::new(),
            }],
            events: vec![
                PullEvent { kind: PullEventKind::Opened, at: stamp_of("2026-09-14T14:02:00Z") },
                PullEvent { kind: PullEventKind::Merged { by: Some("rohan".into()) }, at: stamp_of("2026-09-17T15:00:00Z") },
            ],
        }
    }

    #[test]
    fn a_discussion_round_trips_through_its_row_and_is_replaced_whole() {
        let c = Cache::open_in_memory().unwrap();
        assert_eq!(c.load_discussion("item-pr-7").unwrap(), None);

        let before = Timestamp::now();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        let (fetched_at, loaded) = c.load_discussion("item-pr-7").unwrap().unwrap();
        assert_eq!(loaded, discussion());
        assert!(fetched_at >= before);
        assert_eq!(c.load_discussion("item-pr-8").unwrap(), None, "keyed by item");

        // A second fetch replaces the row rather than adding to it.
        let mut newer = discussion();
        newer.comments.clear();
        c.store_discussion("item-pr-7", &newer).unwrap();
        assert_eq!(c.load_discussion("item-pr-7").unwrap().unwrap().1, newer);
    }

    #[test]
    fn checks_are_only_good_for_the_head_they_were_read_at() {
        use crate::workflow_runs::CheckState;
        let c = Cache::open_in_memory().unwrap();
        let checks = Checks {
            state: CheckState::Failing { workflow: "CI".into() },
            url: Some("https://github.com/acme/widgets/actions/runs/1".into()),
        };
        c.store_checks("item-pr-7", "abc123", &checks).unwrap();
        assert_eq!(c.load_checks("item-pr-7", "abc123").unwrap(), Some(checks.clone()));
        assert_eq!(c.load_checks("item-pr-7", "fff999").unwrap(), None, "a push makes them a miss");
        assert_eq!(c.load_checks("item-pr-8", "abc123").unwrap(), None);

        // Reading them at the new head replaces the row.
        let green = Checks { state: CheckState::Passing, url: None };
        c.store_checks("item-pr-7", "fff999", &green).unwrap();
        assert_eq!(c.load_checks("item-pr-7", "fff999").unwrap(), Some(green));
        assert_eq!(c.load_checks("item-pr-7", "abc123").unwrap(), None);
    }

    #[test]
    fn viewed_is_keyed_by_blob_so_the_same_path_can_be_viewed_at_one_and_not_another() {
        let c = Cache::open_in_memory().unwrap();
        assert!(c.load_viewed("item-pr-7").unwrap().is_empty());

        c.set_viewed("item-pr-7", "src/x.rs", "blob-a", true).unwrap();
        c.set_viewed("item-pr-7", "src/y.rs", "blob-y", true).unwrap();
        let viewed = c.load_viewed("item-pr-7").unwrap();
        assert!(viewed.contains(&("src/x.rs".into(), "blob-a".into())));
        assert!(viewed.contains(&("src/y.rs".into(), "blob-y".into())));
        // The file was pushed to: a new blob, and no row for it.
        assert!(!viewed.contains(&("src/x.rs".into(), "blob-b".into())));

        // Viewing the new blob leaves the old one's row alone, so a revert
        // to it is still viewed.
        c.set_viewed("item-pr-7", "src/x.rs", "blob-b", true).unwrap();
        let viewed = c.load_viewed("item-pr-7").unwrap();
        assert!(viewed.contains(&("src/x.rs".into(), "blob-a".into())));
        assert!(viewed.contains(&("src/x.rs".into(), "blob-b".into())));

        // Unviewing is by blob too, and marking twice is one row.
        c.set_viewed("item-pr-7", "src/x.rs", "blob-a", true).unwrap();
        c.set_viewed("item-pr-7", "src/x.rs", "blob-a", false).unwrap();
        let viewed = c.load_viewed("item-pr-7").unwrap();
        assert!(!viewed.contains(&("src/x.rs".into(), "blob-a".into())));
        assert_eq!(viewed.len(), 2);
        assert!(c.load_viewed("item-pr-8").unwrap().is_empty(), "keyed by item");
    }

    #[test]
    fn reveals_round_trip_by_path_and_blob_and_a_new_blob_has_none() {
        let c = Cache::open_in_memory().unwrap();
        assert!(c.load_reveals("item-pr-7").unwrap().is_empty());

        c.set_reveal("item-pr-7", "src/x.rs", "blob-a", 40, 59).unwrap();
        c.set_reveal("item-pr-7", "src/x.rs", "blob-a", 4, 23).unwrap();
        c.set_reveal("item-pr-7", "src/y.rs", "blob-y", 0, 19).unwrap();
        // Asking for the same range twice is one row.
        c.set_reveal("item-pr-7", "src/x.rs", "blob-a", 4, 23).unwrap();
        let reveals = c.load_reveals("item-pr-7").unwrap();
        assert_eq!(reveals.len(), 2);
        assert_eq!(reveals[&("src/x.rs".into(), "blob-a".into())], vec![(4, 23), (40, 59)], "each file's in order");
        assert_eq!(reveals[&("src/y.rs".into(), "blob-y".into())], vec![(0, 19)]);
        // The file was pushed to: a new blob, and no rows for it.
        assert!(!reveals.contains_key(&("src/x.rs".to_string(), "blob-b".to_string())));
        assert!(c.load_reveals("item-pr-8").unwrap().is_empty(), "keyed by item");
    }

    #[test]
    fn removing_reveals_takes_those_overlapping_the_range_and_no_other() {
        let c = Cache::open_in_memory().unwrap();
        let key = ("src/x.rs".to_string(), "blob-a".to_string());
        for (first, last) in [(0, 9), (10, 29), (30, 30), (31, 50), (60, 79)] {
            c.set_reveal("item-pr-7", "src/x.rs", "blob-a", first, last).unwrap();
        }
        c.set_reveal("item-pr-7", "src/x.rs", "blob-b", 10, 29).unwrap();
        c.set_reveal("other", "src/x.rs", "blob-a", 10, 29).unwrap();

        // A run of 10 to 30: the reveal that ends on its first line and the
        // one that begins on its last both overlap it; the ones either side
        // of those do not.
        c.remove_reveals("item-pr-7", "src/x.rs", "blob-a", 10, 30).unwrap();
        let reveals = c.load_reveals("item-pr-7").unwrap();
        assert_eq!(reveals[&key], vec![(0, 9), (31, 50), (60, 79)]);
        assert_eq!(reveals[&("src/x.rs".into(), "blob-b".into())], vec![(10, 29)], "another blob's rows stay");
        assert_eq!(c.load_reveals("other").unwrap().len(), 1, "another item's rows stay");

        // A range that touches a reveal's edge takes it.
        c.remove_reveals("item-pr-7", "src/x.rs", "blob-a", 50, 60).unwrap();
        let reveals = c.load_reveals("item-pr-7").unwrap();
        assert_eq!(reveals[&key], vec![(0, 9)]);
        // A file with nothing left has no entry at all.
        c.remove_reveals("item-pr-7", "src/x.rs", "blob-a", 0, 0).unwrap();
        assert!(!c.load_reveals("item-pr-7").unwrap().contains_key(&key));
    }

    #[test]
    fn deleting_an_item_takes_what_was_fetched_and_marked_for_it() {
        use crate::workflow_runs::CheckState;
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let id = pr_item(7).id;
        c.store_discussion(&id, &discussion()).unwrap();
        c.store_checks(&id, "abc123", &Checks { state: CheckState::Passing, url: None }).unwrap();
        c.set_viewed(&id, "src/x.rs", "blob-a", true).unwrap();
        c.set_viewed("other", "src/x.rs", "blob-a", true).unwrap();
        c.set_reveal(&id, "src/x.rs", "blob-a", 4, 23).unwrap();
        c.set_reveal("other", "src/x.rs", "blob-a", 4, 23).unwrap();

        c.delete_item(&id).unwrap();
        assert_eq!(c.load_discussion(&id).unwrap(), None);
        assert_eq!(c.load_checks(&id, "abc123").unwrap(), None);
        assert!(c.load_viewed(&id).unwrap().is_empty());
        assert_eq!(c.load_viewed("other").unwrap().len(), 1, "another item's rows stay");
        assert!(c.load_reveals(&id).unwrap().is_empty());
        assert_eq!(c.load_reveals("other").unwrap().len(), 1, "and its reveals");
    }

    #[test]
    fn open_subjects_in_a_repository_are_what_the_list_pass_compares_against() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut merged = pr_item(8);
        if let Some(ItemSubject::PullRequest(pr)) = merged.subject.as_mut() {
            pr.state = "closed".into();
            pr.merged = true;
        }
        c.insert_item(&merged).unwrap();
        c.insert_item(&issue_item(42, "open", "2026-09-18T10:00:00Z")).unwrap();
        c.insert_item(&issue_item(43, "closed", "2026-09-18T10:00:00Z")).unwrap();
        c.insert_item(&branch_item("feature")).unwrap();
        // A branch on a fork with an open pull request upstream counts for
        // the upstream, where the list pass will look for it.
        let mut fork = branch_item("fork-branch");
        fork.repo_full_name = "chris/widgets".into();
        fork.subject = Some(ItemSubject::PullRequest(pull_request(9)));
        c.insert_item(&fork).unwrap();

        assert_eq!(
            c.open_subjects_in("ACME/widgets").unwrap(),
            vec![
                (ItemKind::PullRequest, 7, "item-pr-7".to_string()),
                (ItemKind::PullRequest, 9, "item-fork-branch".to_string()),
                (ItemKind::Issue, 42, "item-issue-42".to_string()),
            ]
        );
        assert!(c.open_subjects_in("chris/widgets").unwrap().is_empty(), "the subject's repository, not the checkout's");
        assert!(c.open_subjects_in("nobody/here").unwrap().is_empty());
    }

    #[test]
    fn discussion_stale_lists_missing_and_outdated_rows_newest_subject_first() {
        let c = Cache::open_in_memory().unwrap();
        let fetched_at = |id: &str, at: &str| {
            c.conn
                .execute("UPDATE discussions SET fetched_at = ?2 WHERE item_id = ?1", params![id, at])
                .unwrap();
        };
        // No row at all.
        c.insert_item(&issue_item(1, "open", "2026-09-18T09:00:00Z")).unwrap();
        // A row older than the subject.
        c.insert_item(&issue_item(2, "open", "2026-09-18T11:00:00Z")).unwrap();
        c.store_discussion("item-issue-2", &discussion()).unwrap();
        fetched_at("item-issue-2", "2026-09-18T10:00:00Z");
        // A row from the same second as the subject, a fraction later: as
        // strings the fraction would sort it before, but it is current.
        c.insert_item(&issue_item(3, "open", "2026-09-18T10:00:00Z")).unwrap();
        c.store_discussion("item-issue-3", &discussion()).unwrap();
        fetched_at("item-issue-3", "2026-09-18T10:00:00.250Z");
        // A pull request whose updated_at is unknown: stale without a row,
        // never with one.
        c.insert_item(&pr_item(7)).unwrap();
        c.insert_item(&pr_item(8)).unwrap();
        c.store_discussion("item-pr-8", &discussion()).unwrap();
        fetched_at("item-pr-8", "2000-01-01T00:00:00Z");
        // A checkout with no subject has no discussion to be stale.
        c.insert_item(&branch_item("feature")).unwrap();

        assert_eq!(c.discussion_stale().unwrap(), vec!["item-issue-2", "item-issue-1", "item-pr-7"]);

        // Storing the discussion again makes the row current.
        c.store_discussion("item-issue-2", &discussion()).unwrap();
        fetched_at("item-issue-2", "2026-09-18T11:00:00Z");
        assert_eq!(c.discussion_stale().unwrap(), vec!["item-issue-1", "item-pr-7"]);
    }

    #[test]
    fn deleting_a_meta_key_makes_it_absent_and_is_fine_when_it_already_is() {
        let c = Cache::open_in_memory().unwrap();
        c.set_meta("etag:pulls:acme/widgets", "\"abc\"").unwrap();
        c.delete_meta("etag:pulls:acme/widgets").unwrap();
        assert_eq!(c.meta("etag:pulls:acme/widgets").unwrap(), None);
        c.delete_meta("etag:pulls:acme/widgets").unwrap();
        assert!(c.meta("schema_version").unwrap().is_some(), "other keys stay");
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
