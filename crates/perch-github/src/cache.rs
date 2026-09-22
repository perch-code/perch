//! Local SQLite cache.
//!
//! Exists so a cold start paints real rows before the network answers, and
//! so what the reader marked and what was fetched for an item survive a
//! launch. Avatars are cached as files rather than blobs, keyed by a hash
//! of their URL.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::comments::{
    self, Anchor, Author, CommentMessage, CommentThread, NewRunEvent, RevisionContext, RunEvent,
    RunEventRecord, RunHistoryCursor, RunHistoryFilter, RunHistoryPage, RunLabels, RunOwner,
    RunPurpose, RunRecord, RunStatus, RunStep, RunSummary, TerminalReason, ThreadOrigin,
};
use crate::discussion::Discussion;
use crate::items::{Checkout as ItemCheckout, Item, Subject, SubjectKind};
use crate::model::Thread;
use crate::reactions::Reactions;
use crate::workflow_runs::Checks;

mod review_store;

/// The shape of the tables. Version 18 adds durable worktree-review history
/// and repeat-review reconciliation records on top of the run schema. Version
/// 19 persists the exact finding cohort used by verifier attempts. Version 20
/// remembers notifications marked Done in Perch until their next revision.
/// Version 21 keeps GitHub's review threads as comment threads, with the
/// node id, link, hunk and reactions of each comment, and fills them from
/// the discussions already kept. Schemas 13 through 20 migrate in place so
/// local conversations and agent history are never discarded.
pub const SCHEMA_VERSION: u32 = 21;

/// Metadata key holding the validator for the cached notifications snapshot.
pub const NOTIFICATIONS_LAST_MODIFIED_KEY: &str = "notifications_last_modified";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOpen {
    Created,
    Current,
    Migrated { from: u32, to: u32 },
}

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
    #[error("database has no supported schema version; its contents were left unchanged")]
    MissingSchemaVersion,
    #[error("database schema {found} is older than the oldest supported schema 13; its contents were left unchanged")]
    UnsupportedSchema { found: u32 },
    #[error("database schema {found} is newer than this build's schema {current}; its contents were left unchanged")]
    NewerSchema { found: u32, current: u32 },
    #[error("invalid run transition from {from:?} to {to:?}")]
    InvalidRunTransition { from: RunStatus, to: RunStatus },
    #[error("run {0} is already terminal")]
    TerminalRun(String),
    #[error("review conflict: {0}")]
    ReviewConflict(String),
    #[error("invalid review transition: {0}")]
    InvalidReviewTransition(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEventAppend {
    Appended(RunEventRecord),
    Duplicate(RunEventRecord),
}

pub struct Cache {
    conn: Connection,
    avatar_dir: Option<PathBuf>,
}

const BASE_SCHEMA: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE threads (
    position    INTEGER NOT NULL UNIQUE,
    id          TEXT PRIMARY KEY,
    updated_at  TEXT NOT NULL,
    json        TEXT NOT NULL
);
CREATE TABLE notification_dismissals (
    thread_id   TEXT PRIMARY KEY,
    updated_at  TEXT NOT NULL
);
CREATE TABLE items (
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
    archived_at      TEXT,
    created_at       TEXT NOT NULL,
    last_opened_at   TEXT NOT NULL,
    additions        INTEGER,
    deletions        INTEGER,
    last_seen_head   TEXT,
    CHECK (subject_kind IS NOT NULL OR branch IS NOT NULL)
);
CREATE UNIQUE INDEX items_subject ON items(subject_repo, subject_kind, subject_number)
    WHERE subject_kind IS NOT NULL;
CREATE UNIQUE INDEX items_branch ON items(repo_full_name, branch) WHERE branch IS NOT NULL;
CREATE INDEX items_repo ON items(repo_full_name COLLATE NOCASE);
CREATE TABLE comment_threads (
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
CREATE INDEX comment_threads_worktree ON comment_threads(worktree_id);
CREATE TABLE discussions (
    item_id     TEXT PRIMARY KEY,
    fetched_at  TEXT NOT NULL,
    json        TEXT NOT NULL
);
CREATE TABLE checks (
    item_id     TEXT PRIMARY KEY,
    head_sha    TEXT NOT NULL,
    fetched_at  TEXT NOT NULL,
    json        TEXT NOT NULL
);
CREATE TABLE viewed (
    item_id    TEXT NOT NULL,
    path       TEXT NOT NULL,
    blob       TEXT NOT NULL,
    viewed_at  TEXT NOT NULL,
    PRIMARY KEY (item_id, path, blob)
);
CREATE TABLE reveals (
    item_id    TEXT NOT NULL,
    path       TEXT NOT NULL,
    blob       TEXT NOT NULL,
    first      INTEGER NOT NULL,
    last       INTEGER NOT NULL,
    PRIMARY KEY (item_id, path, blob, first, last)
);
"#;

const NOTIFICATION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS threads (
    position    INTEGER NOT NULL UNIQUE,
    id          TEXT PRIMARY KEY,
    updated_at  TEXT NOT NULL,
    json        TEXT NOT NULL
);
"#;

const NOTIFICATION_DISMISSAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS notification_dismissals (
    thread_id   TEXT PRIMARY KEY,
    updated_at  TEXT NOT NULL
);
"#;

const RUN_SCHEMA: &str = r#"
CREATE TABLE runs (
    id                     TEXT PRIMARY KEY,
    owner_kind             TEXT NOT NULL,
    owner_item_id          TEXT,
    owner_thread_id        TEXT,
    owner_request_id       TEXT,
    owner_reviewer_run_id  TEXT,
    purpose                TEXT NOT NULL,
    backend                TEXT NOT NULL,
    executable             TEXT,
    executable_version     TEXT,
    requested_model        TEXT NOT NULL,
    effective_model        TEXT NOT NULL,
    requested_effort       TEXT NOT NULL,
    effective_effort       TEXT NOT NULL,
    session_id             TEXT,
    original_request       TEXT NOT NULL,
    prompt_snapshot        TEXT,
    working_directory      TEXT,
    revision_json          TEXT NOT NULL,
    repository_label       TEXT NOT NULL,
    item_label             TEXT NOT NULL,
    branch_label           TEXT,
    status                 TEXT NOT NULL,
    terminal_reason        TEXT,
    cost_usd               REAL,
    duration_ms            INTEGER,
    usage_json             TEXT,
    retry_of_id            TEXT,
    error                  TEXT,
    queued_at              TEXT NOT NULL,
    started_at             TEXT,
    ended_at               TEXT,
    final_result           TEXT,
    final_result_truncated INTEGER NOT NULL DEFAULT 0,
    latest_activity        TEXT,
    last_event_sequence    INTEGER NOT NULL DEFAULT 0,
    CHECK (owner_kind IN ('discussion', 'review', 'unavailable')),
    CHECK (
        (owner_kind = 'discussion' AND owner_item_id IS NOT NULL AND owner_thread_id IS NOT NULL
             AND owner_request_id IS NOT NULL AND owner_reviewer_run_id IS NULL)
        OR (owner_kind = 'review' AND owner_item_id IS NOT NULL AND owner_reviewer_run_id IS NOT NULL
             AND owner_thread_id IS NULL AND owner_request_id IS NULL)
        OR (owner_kind = 'unavailable' AND owner_item_id IS NULL AND owner_request_id IS NULL
             AND owner_reviewer_run_id IS NULL)
    ),
    CHECK (status IN ('queued','starting','running','stopping','completed','failed','stopped','interrupted'))
);
CREATE INDEX runs_active ON runs(status, queued_at, id)
    WHERE status IN ('queued','starting','running','stopping');
CREATE INDEX runs_history ON runs(ended_at DESC, id DESC)
    WHERE status IN ('completed','failed','stopped','interrupted');
CREATE INDEX runs_item ON runs(owner_item_id, queued_at DESC, id DESC);
CREATE INDEX runs_request ON runs(owner_thread_id, owner_request_id, queued_at, id);
CREATE INDEX runs_backend_history ON runs(backend, status, ended_at DESC, id DESC);
CREATE INDEX runs_retry ON runs(retry_of_id);
CREATE UNIQUE INDEX runs_one_active_request ON runs(owner_thread_id, owner_request_id)
    WHERE owner_kind = 'discussion' AND status IN ('queued','starting','running','stopping');
CREATE TABLE run_events (
    run_id       TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    sequence     INTEGER NOT NULL,
    event_id     TEXT NOT NULL,
    occurred_at  TEXT NOT NULL,
    kind         TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    truncated    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, sequence),
    UNIQUE (run_id, event_id)
);
CREATE TABLE comment_messages (
    id             TEXT PRIMARY KEY,
    thread_id      TEXT NOT NULL REFERENCES comment_threads(id) ON DELETE CASCADE,
    origin         TEXT NOT NULL,
    github_id      TEXT,
    author_kind    TEXT NOT NULL,
    author         TEXT,
    author_model   TEXT,
    body           TEXT NOT NULL,
    run_id         TEXT REFERENCES runs(id) ON DELETE SET NULL,
    created_at     TEXT NOT NULL,
    node_id        TEXT,
    html_url       TEXT,
    diff_hunk      TEXT,
    reactions_json TEXT
);
CREATE INDEX comment_messages_thread ON comment_messages(thread_id);
"#;

/// The columns schema 21 adds to a comment message: what a comment from
/// GitHub carries besides its text.
const GITHUB_MESSAGE_COLUMNS: [&str; 4] = ["node_id", "html_url", "diff_hunk", "reactions_json"];

fn initialize_schema(conn: &mut Connection) -> Result<CacheOpen, CacheError> {
    let objects: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if objects == 0 {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(BASE_SCHEMA)?;
        tx.execute_batch(RUN_SCHEMA)?;
        tx.execute_batch(review_store::REVIEW_SCHEMA)?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        tx.commit()?;
        return Ok(CacheOpen::Created);
    }

    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        return Err(CacheError::MissingSchemaVersion);
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let version = raw
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or(CacheError::MissingSchemaVersion)?;
    if version > SCHEMA_VERSION {
        return Err(CacheError::NewerSchema {
            found: version,
            current: SCHEMA_VERSION,
        });
    }
    if version < 13 {
        return Err(CacheError::UnsupportedSchema { found: version });
    }
    if version == SCHEMA_VERSION {
        return Ok(CacheOpen::Current);
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut from = version;
    while from < SCHEMA_VERSION {
        match from {
            13 => migrate_13_to_14(&tx)?,
            14 => tx.execute_batch(NOTIFICATION_SCHEMA)?,
            15 => migrate_15_to_16(&tx)?,
            16 => migrate_16_to_17(&tx)?,
            17 => review_store::migrate_17_to_18(&tx)?,
            18 => review_store::migrate_18_to_19(&tx)?,
            19 => tx.execute_batch(NOTIFICATION_DISMISSAL_SCHEMA)?,
            20 => migrate_20_to_21(&tx)?,
            other => return Err(CacheError::UnsupportedSchema { found: other }),
        }
        from += 1;
    }
    tx.execute(
        "UPDATE meta SET value=?1 WHERE key='schema_version'",
        params![SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(CacheOpen::Migrated {
        from: version,
        to: SCHEMA_VERSION,
    })
}

/// Schema 15 existed on main with the legacy run table, while rebased
/// development builds could already have the richer run table. Accept both:
/// migrate the former and leave the latter intact, and ensure the notification
/// cache exists in either case.
fn migrate_15_to_16(tx: &Transaction<'_>) -> Result<(), CacheError> {
    tx.execute_batch(NOTIFICATION_SCHEMA)?;
    let has_run_owner: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('runs') WHERE name='owner_kind'
         )",
        [],
        |row| row.get(0),
    )?;
    if !has_run_owner {
        migrate_13_to_14(tx)?;
    }
    Ok(())
}

fn migrate_16_to_17(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let has_prompt_snapshot: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('runs') WHERE name='prompt_snapshot'
         )",
        [],
        |row| row.get(0),
    )?;
    if !has_prompt_snapshot {
        tx.execute("ALTER TABLE runs ADD COLUMN prompt_snapshot TEXT", [])?;
    }
    tx.execute(
        "UPDATE runs SET prompt_snapshot=original_request WHERE prompt_snapshot IS NULL",
        [],
    )?;

    let has_result_truncated: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('runs') WHERE name='final_result_truncated'
         )",
        [],
        |row| row.get(0),
    )?;
    if !has_result_truncated {
        tx.execute(
            "ALTER TABLE runs ADD COLUMN final_result_truncated INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    let results = {
        let mut stmt = tx.prepare(
            "SELECT id, final_result FROM runs
             WHERE final_result IS NOT NULL AND length(CAST(final_result AS BLOB)) > ?1",
        )?;
        let rows = stmt.query_map(params![MAX_RUN_RESULT_BYTES as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, mut result) in results {
        truncate_string(&mut result, MAX_RUN_RESULT_BYTES);
        tx.execute(
            "UPDATE runs SET final_result=?2, final_result_truncated=1 WHERE id=?1",
            params![id, result],
        )?;
    }
    Ok(())
}

/// GitHub's review threads become comment threads. The columns a comment
/// from GitHub needs are added, and every discussion already kept is
/// ingested so an upgraded install shows its threads before its first
/// fetch. A discussion row that no longer reads is passed over.
fn migrate_20_to_21(tx: &Transaction<'_>) -> Result<(), CacheError> {
    for column in GITHUB_MESSAGE_COLUMNS {
        let present: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM pragma_table_info('comment_messages') WHERE name=?1
             )",
            params![column],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute(
                &format!("ALTER TABLE comment_messages ADD COLUMN {column} TEXT"),
                [],
            )?;
        }
    }
    let discussions = {
        let mut stmt = tx.prepare("SELECT item_id, json FROM discussions")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (item_id, json) in discussions {
        let Ok(discussion) = serde_json::from_str::<Discussion>(&json) else {
            tracing::warn!(item_id, "skipping an unreadable cached discussion");
            continue;
        };
        let head_sha = item_head_sha_on(tx, &item_id)?;
        let threads = comments::review_threads_of(&discussion, &item_id, &head_sha);
        ingest_review_threads(tx, &item_id, &threads)?;
    }
    Ok(())
}

/// The head of an item's pull request, or nothing when the item has none.
fn item_head_sha_on(conn: &Connection, item_id: &str) -> Result<String, CacheError> {
    let item = conn
        .query_row(
            &format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = ?1"),
            params![item_id],
            read_item,
        )
        .optional()?;
    Ok(item
        .and_then(|item| item.pull_request().map(|pr| pr.head_sha.clone()))
        .unwrap_or_default())
}

/// Put GitHub's review threads into the cache beside Perch's own,
/// replacing what was kept of them before. A thread keeps its id, and so
/// everything local under it: the reader's questions to an agent and the
/// answers, and a draft not yet posted. Only comments that are on GitHub
/// are replaced. A thread no longer on GitHub keeps its row while anything
/// else is under it, and goes when nothing is. The one that stays has no
/// comment on GitHub to point at any more, so it loses its `github_id`
/// and is a thread on its way there, as a draft's is.
pub(crate) fn ingest_review_threads(
    conn: &Connection,
    item_id: &str,
    incoming: &[CommentThread],
) -> Result<(), CacheError> {
    for thread in incoming {
        let anchor = thread.anchor.as_ref();
        conn.execute(
            "INSERT INTO comment_threads
             (id, worktree_id, origin, github_id, path, start_line, line, side, commit_sha,
              outdated, resolved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
                worktree_id=excluded.worktree_id, origin=excluded.origin,
                github_id=excluded.github_id, path=excluded.path,
                start_line=excluded.start_line, line=excluded.line, side=excluded.side,
                commit_sha=excluded.commit_sha, outdated=excluded.outdated,
                resolved=excluded.resolved, created_at=excluded.created_at",
            params![
                thread.id,
                item_id,
                thread.origin.name(),
                thread.github_id.map(|id| id.to_string()),
                anchor.map(|a| a.path.as_str()),
                anchor.map(|a| a.start_line as i64),
                anchor.map(|a| a.line as i64),
                anchor.map(|a| a.side.name()),
                anchor.map(|a| a.commit_sha.as_str()),
                thread.outdated as i64,
                thread.resolved as i64,
                thread.created_at.to_string(),
            ],
        )?;
        for message in &thread.messages {
            let (kind, author) = comments::sql::author_columns(&message.author);
            conn.execute(
                "INSERT INTO comment_messages
                 (id, thread_id, origin, github_id, author_kind, author, author_model, body,
                  run_id, created_at, node_id, html_url, diff_hunk, reactions_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, NULL, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    thread_id=excluded.thread_id, origin=excluded.origin,
                    github_id=excluded.github_id, author_kind=excluded.author_kind,
                    author=excluded.author, body=excluded.body, created_at=excluded.created_at,
                    node_id=excluded.node_id, html_url=excluded.html_url,
                    diff_hunk=excluded.diff_hunk, reactions_json=excluded.reactions_json",
                params![
                    message.id,
                    thread.id,
                    message.origin.name(),
                    message.github_id.map(|id| id.to_string()),
                    kind,
                    author,
                    message.body,
                    message.created_at.to_string(),
                    message.node_id,
                    message.html_url,
                    message.diff_hunk,
                    reactions_json(&message.reactions),
                ],
            )?;
        }
    }

    let keep: HashSet<&str> = incoming
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.id.as_str()))
        .collect();
    let stale = {
        let mut stmt = conn.prepare(
            "SELECT m.id FROM comment_messages m
             JOIN comment_threads t ON t.id = m.thread_id
             WHERE t.worktree_id = ?1 AND m.origin = 'github' AND m.github_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![item_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|id| !keep.contains(id.as_str()))
            .collect::<Vec<_>>()
    };
    for id in stale {
        conn.execute("DELETE FROM comment_messages WHERE id = ?1", params![id])?;
    }
    conn.execute(
        "DELETE FROM comment_threads
         WHERE worktree_id = ?1 AND origin = 'github'
           AND NOT EXISTS (SELECT 1 FROM comment_messages WHERE thread_id = comment_threads.id)",
        params![item_id],
    )?;

    let keep: HashSet<&str> = incoming.iter().map(|t| t.id.as_str()).collect();
    let orphaned = {
        let mut stmt = conn.prepare(
            "SELECT id FROM comment_threads
             WHERE worktree_id = ?1 AND origin = 'github' AND github_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![item_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|id| !keep.contains(id.as_str()))
            .collect::<Vec<_>>()
    };
    for id in orphaned {
        conn.execute(
            "UPDATE comment_threads SET github_id = NULL WHERE id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

/// The reactions column: the groups as JSON, or nothing when there are
/// none.
fn reactions_json(reactions: &Reactions) -> Option<String> {
    if reactions.is_empty() {
        return None;
    }
    serde_json::to_string(reactions).ok()
}

/// The reactions a column holds; a value that does not read is none.
fn reactions_of(json: Option<String>) -> Reactions {
    json.and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// A GitHub id as the column keeps it, and back.
fn github_id_of(column: Option<String>) -> Option<u64> {
    column.and_then(|id| id.parse().ok())
}

#[derive(Debug)]
struct LegacyRun {
    id: String,
    thread_id: String,
    backend: String,
    model: String,
    effort: String,
    session_id: Option<String>,
    status: String,
    cost_usd: Option<f64>,
    duration_ms: Option<i64>,
    steps_json: String,
    error: Option<String>,
    started_at: String,
    ended_at: Option<String>,
}

#[derive(Debug, Clone)]
struct LegacyMessage {
    rowid: i64,
    id: String,
    thread_id: String,
    author_kind: String,
    body: String,
    run_id: Option<String>,
    created_at: String,
}

fn migrate_13_to_14(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let runs = {
        let mut stmt = tx.prepare(
            "SELECT id, thread_id, backend, model, effort, session_id, status, cost_usd,
                    duration_ms, steps_json, error, started_at, ended_at FROM runs ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LegacyRun {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                backend: row.get(2)?,
                model: row.get(3)?,
                effort: row.get(4)?,
                session_id: row.get(5)?,
                status: row.get(6)?,
                cost_usd: row.get(7)?,
                duration_ms: row.get(8)?,
                steps_json: row.get(9)?,
                error: row.get(10)?,
                started_at: row.get(11)?,
                ended_at: row.get(12)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let messages = {
        let mut stmt = tx.prepare(
            "SELECT rowid, id, thread_id, author_kind, body, run_id, created_at
             FROM comment_messages ORDER BY thread_id, created_at, rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LegacyMessage {
                rowid: row.get(0)?,
                id: row.get(1)?,
                thread_id: row.get(2)?,
                author_kind: row.get(3)?,
                body: row.get(4)?,
                run_id: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let threads: HashMap<String, String> = {
        let mut stmt = tx.prepare("SELECT id, worktree_id FROM comment_threads")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<HashMap<_, _>, _>>()?
    };
    let labels: HashMap<String, RunLabels> = {
        let mut stmt =
            tx.prepare("SELECT id, repo_full_name, name, subject_json, branch FROM items")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let repository: String = row.get(1)?;
            let name: Option<String> = row.get(2)?;
            let subject_json: Option<String> = row.get(3)?;
            let branch: Option<String> = row.get(4)?;
            let subject_title = subject_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Subject>(json).ok())
                .map(|subject| subject.title().to_string());
            let item = name
                .filter(|name| !name.trim().is_empty())
                .or(subject_title)
                .or_else(|| branch.clone())
                .unwrap_or_else(|| repository.clone());
            Ok((
                id,
                RunLabels {
                    repository,
                    item,
                    branch,
                },
            ))
        })?;
        rows.collect::<Result<HashMap<_, _>, _>>()?
    };

    tx.execute_batch(
        "ALTER TABLE comment_messages RENAME TO comment_messages_v13;
         DROP INDEX IF EXISTS comment_messages_thread;
         ALTER TABLE runs RENAME TO runs_v13;",
    )?;
    tx.execute_batch(RUN_SCHEMA)?;

    for legacy in runs {
        let status = RunStatus::parse(&legacy.status)
            .ok_or_else(|| CacheError::Corrupt(format!("legacy run status {}", legacy.status)))?;
        let started_at = legacy
            .started_at
            .parse::<Timestamp>()
            .map_err(|_| CacheError::Corrupt("legacy run started_at".into()))?;
        let mut ended_at = legacy
            .ended_at
            .as_deref()
            .and_then(|value| value.parse().ok());
        let linked = messages
            .iter()
            .find(|message| message.run_id.as_deref() == Some(&legacy.id));
        let request = match linked {
            Some(message) if message.author_kind == "user" => Some(message),
            Some(message) => messages.iter().rfind(|candidate| {
                candidate.thread_id == legacy.thread_id
                    && candidate.author_kind == "user"
                    && candidate.rowid < message.rowid
            }),
            None => messages.iter().rfind(|candidate| {
                candidate.thread_id == legacy.thread_id
                    && candidate.author_kind == "user"
                    && candidate.created_at <= legacy.started_at
            }),
        };
        let item_id = threads.get(&legacy.thread_id);
        let owner = match (item_id, request) {
            (Some(item_id), Some(request)) if labels.contains_key(item_id) => {
                RunOwner::Discussion {
                    item_id: item_id.clone(),
                    thread_id: legacy.thread_id.clone(),
                    request_id: request.id.clone(),
                }
            }
            _ => RunOwner::Unavailable {
                legacy_thread_id: Some(legacy.thread_id.clone()),
            },
        };
        let purpose = request
            .map(|request| {
                let earlier = messages.iter().any(|candidate| {
                    candidate.thread_id == legacy.thread_id
                        && candidate.author_kind == "user"
                        && candidate.rowid < request.rowid
                });
                if earlier {
                    RunPurpose::FollowUp
                } else {
                    RunPurpose::Question
                }
            })
            .unwrap_or(RunPurpose::Question);
        let labels = item_id
            .and_then(|id| labels.get(id))
            .cloned()
            .unwrap_or_default();
        let final_result = linked
            .filter(|message| message.author_kind == "agent")
            .map(|message| message.body.clone());
        let terminal_reason = match status {
            RunStatus::Completed => Some(TerminalReason::Completed),
            RunStatus::Stopped => Some(TerminalReason::UserStopped),
            RunStatus::Failed => Some(TerminalReason::Other),
            _ => None,
        };
        if status.is_terminal() && ended_at.is_none() {
            ended_at = Some(started_at);
        }
        let steps: Vec<RunStep> = serde_json::from_str(&legacy.steps_json)?;
        let record = RunRecord {
            id: legacy.id.clone(),
            owner,
            purpose,
            backend: legacy.backend,
            executable: None,
            executable_version: None,
            requested_model: legacy.model.clone(),
            model: legacy.model,
            requested_effort: legacy.effort.clone(),
            effort: legacy.effort,
            session_id: legacy.session_id,
            original_request: request
                .map(|message| message.body.clone())
                .unwrap_or_default(),
            prompt_snapshot: request.map(|message| message.body.clone()),
            working_directory: None,
            revision: RevisionContext::Unknown,
            labels,
            status,
            terminal_reason,
            cost_usd: legacy.cost_usd,
            duration_ms: legacy.duration_ms.map(|value| value as u64),
            usage: None,
            retry_of: None,
            last_event_sequence: steps.len() as u64,
            steps: steps.clone(),
            error: legacy.error,
            queued_at: started_at,
            started_at: Some(started_at),
            ended_at,
            final_result,
            final_result_truncated: false,
            latest_activity: steps.last().map(|step| step.summary.clone()),
        };
        insert_run_row(tx, &record)?;
        for (index, step) in steps.into_iter().enumerate() {
            let event = RunEvent::LegacyStep {
                summary: step.summary,
                ok: step.ok,
            };
            tx.execute(
                "INSERT INTO run_events
                 (run_id, sequence, event_id, occurred_at, kind, payload_json, truncated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                params![
                    record.id,
                    index as i64 + 1,
                    format!("legacy-step-{}", index + 1),
                    started_at.to_string(),
                    event.kind(),
                    serde_json::to_string(&event)?,
                ],
            )?;
        }
    }

    tx.execute(
        "INSERT INTO comment_messages
         (id, thread_id, origin, github_id, author_kind, author, author_model, body, run_id, created_at)
         SELECT id, thread_id, origin, github_id, author_kind, author, author_model, body, run_id, created_at
         FROM comment_messages_v13",
        [],
    )?;
    tx.execute_batch("DROP TABLE comment_messages_v13; DROP TABLE runs_v13;")?;
    Ok(())
}

const RUN_COLUMNS: &str = "id, owner_kind, owner_item_id, owner_thread_id, owner_request_id,
    owner_reviewer_run_id, purpose, backend, executable, executable_version, requested_model,
    effective_model, requested_effort, effective_effort, session_id, original_request,
    prompt_snapshot, working_directory, revision_json, repository_label, item_label, branch_label,
    status, terminal_reason, cost_usd, duration_ms, usage_json, retry_of_id, error, queued_at,
    started_at, ended_at, final_result, final_result_truncated, latest_activity,
    last_event_sequence";

#[derive(Debug)]
struct RunDbRow {
    id: String,
    owner_kind: String,
    owner_item_id: Option<String>,
    owner_thread_id: Option<String>,
    owner_request_id: Option<String>,
    owner_reviewer_run_id: Option<String>,
    purpose: String,
    backend: String,
    executable: Option<String>,
    executable_version: Option<String>,
    requested_model: String,
    model: String,
    requested_effort: String,
    effort: String,
    session_id: Option<String>,
    original_request: String,
    prompt_snapshot: Option<String>,
    working_directory: Option<String>,
    revision_json: String,
    repository: String,
    item: String,
    branch: Option<String>,
    status: String,
    terminal_reason: Option<String>,
    cost_usd: Option<f64>,
    duration_ms: Option<i64>,
    usage_json: Option<String>,
    retry_of: Option<String>,
    error: Option<String>,
    queued_at: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    final_result: Option<String>,
    final_result_truncated: bool,
    latest_activity: Option<String>,
    last_event_sequence: i64,
}

fn read_run_db_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunDbRow> {
    Ok(RunDbRow {
        id: row.get(0)?,
        owner_kind: row.get(1)?,
        owner_item_id: row.get(2)?,
        owner_thread_id: row.get(3)?,
        owner_request_id: row.get(4)?,
        owner_reviewer_run_id: row.get(5)?,
        purpose: row.get(6)?,
        backend: row.get(7)?,
        executable: row.get(8)?,
        executable_version: row.get(9)?,
        requested_model: row.get(10)?,
        model: row.get(11)?,
        requested_effort: row.get(12)?,
        effort: row.get(13)?,
        session_id: row.get(14)?,
        original_request: row.get(15)?,
        prompt_snapshot: row.get(16)?,
        working_directory: row.get(17)?,
        revision_json: row.get(18)?,
        repository: row.get(19)?,
        item: row.get(20)?,
        branch: row.get(21)?,
        status: row.get(22)?,
        terminal_reason: row.get(23)?,
        cost_usd: row.get(24)?,
        duration_ms: row.get(25)?,
        usage_json: row.get(26)?,
        retry_of: row.get(27)?,
        error: row.get(28)?,
        queued_at: row.get(29)?,
        started_at: row.get(30)?,
        ended_at: row.get(31)?,
        final_result: row.get(32)?,
        final_result_truncated: row.get::<_, i64>(33)? != 0,
        latest_activity: row.get(34)?,
        last_event_sequence: row.get(35)?,
    })
}

fn parse_optional_timestamp(
    value: Option<String>,
    field: &str,
) -> Result<Option<Timestamp>, CacheError> {
    value
        .map(|value| value.parse().map_err(|_| CacheError::Corrupt(field.into())))
        .transpose()
}

fn run_from_db(row: RunDbRow, steps: Vec<RunStep>) -> Result<RunRecord, CacheError> {
    let owner = match row.owner_kind.as_str() {
        "discussion" => RunOwner::Discussion {
            item_id: row
                .owner_item_id
                .ok_or_else(|| CacheError::Corrupt("discussion item id".into()))?,
            thread_id: row
                .owner_thread_id
                .ok_or_else(|| CacheError::Corrupt("discussion thread id".into()))?,
            request_id: row
                .owner_request_id
                .ok_or_else(|| CacheError::Corrupt("discussion request id".into()))?,
        },
        "review" => RunOwner::Review {
            item_id: row
                .owner_item_id
                .ok_or_else(|| CacheError::Corrupt("review item id".into()))?,
            reviewer_run_id: row
                .owner_reviewer_run_id
                .ok_or_else(|| CacheError::Corrupt("reviewer run id".into()))?,
        },
        "unavailable" => RunOwner::Unavailable {
            legacy_thread_id: row.owner_thread_id,
        },
        _ => return Err(CacheError::Corrupt("run owner kind".into())),
    };
    let purpose =
        RunPurpose::parse(&row.purpose).ok_or_else(|| CacheError::Corrupt("run purpose".into()))?;
    let status =
        RunStatus::parse(&row.status).ok_or_else(|| CacheError::Corrupt("run status".into()))?;
    let terminal_reason = row
        .terminal_reason
        .map(|reason| {
            TerminalReason::parse(&reason)
                .ok_or_else(|| CacheError::Corrupt("terminal reason".into()))
        })
        .transpose()?;
    Ok(RunRecord {
        id: row.id,
        owner,
        purpose,
        backend: row.backend,
        executable: row.executable,
        executable_version: row.executable_version,
        requested_model: row.requested_model,
        model: row.model,
        requested_effort: row.requested_effort,
        effort: row.effort,
        session_id: row.session_id,
        original_request: row.original_request,
        prompt_snapshot: row.prompt_snapshot,
        working_directory: row.working_directory,
        revision: serde_json::from_str(&row.revision_json)?,
        labels: RunLabels {
            repository: row.repository,
            item: row.item,
            branch: row.branch,
        },
        status,
        terminal_reason,
        cost_usd: row.cost_usd,
        duration_ms: row.duration_ms.map(|value| value as u64),
        usage: row
            .usage_json
            .map(|json| serde_json::from_str(&json))
            .transpose()?,
        retry_of: row.retry_of,
        steps,
        error: row.error,
        queued_at: row
            .queued_at
            .parse()
            .map_err(|_| CacheError::Corrupt("run queued_at".into()))?,
        started_at: parse_optional_timestamp(row.started_at, "run started_at")?,
        ended_at: parse_optional_timestamp(row.ended_at, "run ended_at")?,
        final_result: row.final_result,
        final_result_truncated: row.final_result_truncated,
        latest_activity: row.latest_activity,
        last_event_sequence: row.last_event_sequence.max(0) as u64,
    })
}

const RUN_SUMMARY_COLUMNS: &str = "id, owner_kind, owner_item_id, owner_thread_id,
    owner_request_id, owner_reviewer_run_id, purpose, backend, effective_model,
    effective_effort, original_request, repository_label, item_label, branch_label, status,
    terminal_reason, cost_usd, duration_ms, usage_json, retry_of_id, error, queued_at,
    started_at, ended_at, latest_activity, last_event_sequence";

#[derive(Debug)]
struct RunSummaryDbRow {
    id: String,
    owner_kind: String,
    owner_item_id: Option<String>,
    owner_thread_id: Option<String>,
    owner_request_id: Option<String>,
    owner_reviewer_run_id: Option<String>,
    purpose: String,
    backend: String,
    model: String,
    effort: String,
    original_request: String,
    repository: String,
    item: String,
    branch: Option<String>,
    status: String,
    terminal_reason: Option<String>,
    cost_usd: Option<f64>,
    duration_ms: Option<i64>,
    usage_json: Option<String>,
    retry_of: Option<String>,
    error: Option<String>,
    queued_at: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    latest_activity: Option<String>,
    last_event_sequence: i64,
}

fn read_run_summary_db_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunSummaryDbRow> {
    Ok(RunSummaryDbRow {
        id: row.get(0)?,
        owner_kind: row.get(1)?,
        owner_item_id: row.get(2)?,
        owner_thread_id: row.get(3)?,
        owner_request_id: row.get(4)?,
        owner_reviewer_run_id: row.get(5)?,
        purpose: row.get(6)?,
        backend: row.get(7)?,
        model: row.get(8)?,
        effort: row.get(9)?,
        original_request: row.get(10)?,
        repository: row.get(11)?,
        item: row.get(12)?,
        branch: row.get(13)?,
        status: row.get(14)?,
        terminal_reason: row.get(15)?,
        cost_usd: row.get(16)?,
        duration_ms: row.get(17)?,
        usage_json: row.get(18)?,
        retry_of: row.get(19)?,
        error: row.get(20)?,
        queued_at: row.get(21)?,
        started_at: row.get(22)?,
        ended_at: row.get(23)?,
        latest_activity: row.get(24)?,
        last_event_sequence: row.get(25)?,
    })
}

fn summary_from_db(row: RunSummaryDbRow) -> Result<RunSummary, CacheError> {
    let owner = match row.owner_kind.as_str() {
        "discussion" => RunOwner::Discussion {
            item_id: row
                .owner_item_id
                .ok_or_else(|| CacheError::Corrupt("discussion item id".into()))?,
            thread_id: row
                .owner_thread_id
                .ok_or_else(|| CacheError::Corrupt("discussion thread id".into()))?,
            request_id: row
                .owner_request_id
                .ok_or_else(|| CacheError::Corrupt("discussion request id".into()))?,
        },
        "review" => RunOwner::Review {
            item_id: row
                .owner_item_id
                .ok_or_else(|| CacheError::Corrupt("review item id".into()))?,
            reviewer_run_id: row
                .owner_reviewer_run_id
                .ok_or_else(|| CacheError::Corrupt("reviewer run id".into()))?,
        },
        "unavailable" => RunOwner::Unavailable {
            legacy_thread_id: row.owner_thread_id,
        },
        _ => return Err(CacheError::Corrupt("run owner kind".into())),
    };
    let purpose =
        RunPurpose::parse(&row.purpose).ok_or_else(|| CacheError::Corrupt("run purpose".into()))?;
    let status =
        RunStatus::parse(&row.status).ok_or_else(|| CacheError::Corrupt("run status".into()))?;
    let terminal_reason = row
        .terminal_reason
        .map(|reason| {
            TerminalReason::parse(&reason)
                .ok_or_else(|| CacheError::Corrupt("terminal reason".into()))
        })
        .transpose()?;
    Ok(RunSummary {
        id: row.id,
        owner,
        purpose,
        backend: row.backend,
        model: row.model,
        effort: row.effort,
        original_request: row.original_request,
        labels: RunLabels {
            repository: row.repository,
            item: row.item,
            branch: row.branch,
        },
        status,
        terminal_reason,
        cost_usd: row.cost_usd,
        duration_ms: row.duration_ms.map(|value| value as u64),
        usage: row
            .usage_json
            .map(|json| serde_json::from_str(&json))
            .transpose()?,
        retry_of: row.retry_of,
        error: row.error,
        queued_at: row
            .queued_at
            .parse()
            .map_err(|_| CacheError::Corrupt("run queued_at".into()))?,
        started_at: parse_optional_timestamp(row.started_at, "run started_at")?,
        ended_at: parse_optional_timestamp(row.ended_at, "run ended_at")?,
        latest_activity: row.latest_activity,
        last_event_sequence: row.last_event_sequence.max(0) as u64,
    })
}

fn owner_columns(
    owner: &RunOwner,
) -> (
    &'static str,
    Option<&str>,
    Option<&str>,
    Option<&str>,
    Option<&str>,
) {
    match owner {
        RunOwner::Discussion {
            item_id,
            thread_id,
            request_id,
        } => (
            "discussion",
            Some(item_id),
            Some(thread_id),
            Some(request_id),
            None,
        ),
        RunOwner::Review {
            item_id,
            reviewer_run_id,
        } => ("review", Some(item_id), None, None, Some(reviewer_run_id)),
        RunOwner::Unavailable { legacy_thread_id } => {
            ("unavailable", None, legacy_thread_id.as_deref(), None, None)
        }
    }
}

fn insert_run_row(conn: &Connection, run: &RunRecord) -> Result<(), CacheError> {
    let (owner_kind, item_id, thread_id, request_id, reviewer_run_id) = owner_columns(&run.owner);
    let (final_result, result_was_truncated) = run
        .final_result
        .as_deref()
        .map(|result| bounded_string(result, MAX_RUN_RESULT_BYTES))
        .map_or((None, false), |(result, truncated)| {
            (Some(result), truncated)
        });
    let latest_activity = run
        .latest_activity
        .as_deref()
        .map(|activity| bounded_string(activity, MAX_RUN_ACTIVITY_BYTES).0);
    conn.execute(
        "INSERT INTO runs
         (id, owner_kind, owner_item_id, owner_thread_id, owner_request_id, owner_reviewer_run_id,
          purpose, backend, executable, executable_version, requested_model, effective_model,
          requested_effort, effective_effort, session_id, original_request, prompt_snapshot,
          working_directory, revision_json, repository_label, item_label, branch_label, status,
          terminal_reason, cost_usd, duration_ms, usage_json, retry_of_id, error, queued_at,
          started_at, ended_at, final_result, final_result_truncated, latest_activity,
          last_event_sequence)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,
                 ?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,
                 ?35,?36)",
        params![
            run.id,
            owner_kind,
            item_id,
            thread_id,
            request_id,
            reviewer_run_id,
            run.purpose.name(),
            run.backend,
            run.executable,
            run.executable_version,
            run.requested_model,
            run.model,
            run.requested_effort,
            run.effort,
            run.session_id,
            run.original_request,
            run.prompt_snapshot,
            run.working_directory,
            serde_json::to_string(&run.revision)?,
            run.labels.repository,
            run.labels.item,
            run.labels.branch,
            run.status.name(),
            run.terminal_reason.map(TerminalReason::name),
            run.cost_usd,
            run.duration_ms.map(|value| value as i64),
            run.usage.as_ref().map(serde_json::to_string).transpose()?,
            run.retry_of,
            run.error,
            run.queued_at.to_string(),
            run.started_at.map(|value| value.to_string()),
            run.ended_at.map(|value| value.to_string()),
            final_result,
            (run.final_result_truncated || result_was_truncated) as i64,
            latest_activity,
            run.last_event_sequence as i64,
        ],
    )?;
    Ok(())
}

fn update_run_row(conn: &Connection, run: &RunRecord) -> Result<(), CacheError> {
    let (final_result, result_was_truncated) = run
        .final_result
        .as_deref()
        .map(|result| bounded_string(result, MAX_RUN_RESULT_BYTES))
        .map_or((None, false), |(result, truncated)| {
            (Some(result), truncated)
        });
    let latest_activity = run
        .latest_activity
        .as_deref()
        .map(|activity| bounded_string(activity, MAX_RUN_ACTIVITY_BYTES).0);
    conn.execute(
        "UPDATE runs SET effective_model=?2, effective_effort=?3, session_id=?4,
         prompt_snapshot=?5, status=?6, terminal_reason=?7, cost_usd=?8, duration_ms=?9,
         usage_json=?10, error=?11, started_at=?12, ended_at=?13, final_result=?14,
         final_result_truncated=?15, latest_activity=?16, last_event_sequence=?17 WHERE id=?1",
        params![
            run.id,
            run.model,
            run.effort,
            run.session_id,
            run.prompt_snapshot,
            run.status.name(),
            run.terminal_reason.map(TerminalReason::name),
            run.cost_usd,
            run.duration_ms.map(|value| value as i64),
            run.usage.as_ref().map(serde_json::to_string).transpose()?,
            run.error,
            run.started_at.map(|value| value.to_string()),
            run.ended_at.map(|value| value.to_string()),
            final_result,
            (run.final_result_truncated || result_was_truncated) as i64,
            latest_activity,
            run.last_event_sequence as i64,
        ],
    )?;
    Ok(())
}

fn load_run_on(conn: &Connection, id: &str) -> Result<Option<RunRecord>, CacheError> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1");
    let row = conn
        .query_row(&sql, params![id], read_run_db_row)
        .optional()?;
    row.map(|row| {
        let steps = load_steps_on(conn, &row.id)?;
        run_from_db(row, steps)
    })
    .transpose()
}

/// Load only the mutable run projection. Event append and lifecycle updates
/// must not rescan the historical tool stream on every write.
fn load_run_projection_on(conn: &Connection, id: &str) -> Result<Option<RunRecord>, CacheError> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1");
    conn.query_row(&sql, params![id], read_run_db_row)
        .optional()?
        .map(|row| run_from_db(row, Vec::new()))
        .transpose()
}

fn read_event_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, i64, String, String, String, bool)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get::<_, i64>(5)? != 0,
    ))
}

fn event_from_row(
    (run_id, sequence, event_id, occurred_at, payload, truncated): (
        String,
        i64,
        String,
        String,
        String,
        bool,
    ),
) -> Result<RunEventRecord, CacheError> {
    Ok(RunEventRecord {
        run_id,
        sequence: sequence.max(0) as u64,
        event_id,
        occurred_at: occurred_at
            .parse()
            .map_err(|_| CacheError::Corrupt("event occurred_at".into()))?,
        event: serde_json::from_str(&payload)?,
        truncated,
    })
}

fn load_run_events_on(
    conn: &Connection,
    run_id: &str,
    before_sequence: Option<u64>,
    limit: usize,
) -> Result<Vec<RunEventRecord>, CacheError> {
    let limit = limit.clamp(1, 500) as i64;
    let rows = {
        let mut stmt = conn.prepare(
            "SELECT run_id, sequence, event_id, occurred_at, payload_json, truncated
             FROM run_events WHERE run_id=?1 AND (?2 IS NULL OR sequence < ?2)
             ORDER BY sequence DESC LIMIT ?3",
        )?;
        let mapped = stmt.query_map(
            params![run_id, before_sequence.map(|value| value as i64), limit],
            read_event_row,
        )?;
        mapped.collect::<Result<Vec<_>, _>>()?
    };
    let mut events = rows
        .into_iter()
        .map(event_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    events.reverse();
    Ok(events)
}

fn load_event_by_id(
    conn: &Connection,
    run_id: &str,
    event_id: &str,
) -> Result<Option<RunEventRecord>, CacheError> {
    conn.query_row(
        "SELECT run_id, sequence, event_id, occurred_at, payload_json, truncated
         FROM run_events WHERE run_id=?1 AND event_id=?2",
        params![run_id, event_id],
        read_event_row,
    )
    .optional()?
    .map(event_from_row)
    .transpose()
}

fn load_steps_on(conn: &Connection, run_id: &str) -> Result<Vec<RunStep>, CacheError> {
    let mut stmt = conn.prepare(
        "SELECT payload_json FROM run_events
         WHERE run_id=?1 AND kind IN ('tool_call','tool_result','legacy_step') ORDER BY sequence",
    )?;
    let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
    let mut steps = Vec::new();
    let mut tools: HashMap<String, usize> = HashMap::new();
    for payload in rows {
        match serde_json::from_str::<RunEvent>(&payload?)? {
            RunEvent::LegacyStep { summary, ok } => steps.push(RunStep { summary, ok }),
            RunEvent::ToolCall { id, summary, .. } => {
                tools.insert(id, steps.len());
                steps.push(RunStep { summary, ok: true });
            }
            RunEvent::ToolResult { id, ok, summary } => {
                if let Some(step) = tools.get(&id).and_then(|index| steps.get_mut(*index)) {
                    step.ok = ok;
                    if !ok && !summary.is_empty() {
                        step.summary = format!("{}: {summary}", step.summary);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(steps)
}

fn insert_event_row(conn: &Connection, event: &RunEventRecord) -> Result<(), CacheError> {
    conn.execute(
        "INSERT INTO run_events
         (run_id, sequence, event_id, occurred_at, kind, payload_json, truncated)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            event.run_id,
            event.sequence as i64,
            event.event_id,
            event.occurred_at.to_string(),
            event.event.kind(),
            serde_json::to_string(&event.event)?,
            event.truncated as i64,
        ],
    )?;
    Ok(())
}

fn apply_event_to_run(
    run: &mut RunRecord,
    event: &RunEvent,
    at: Timestamp,
    source_truncated: bool,
) -> Result<(), CacheError> {
    let transition = |run: &mut RunRecord, next: RunStatus| -> Result<(), CacheError> {
        if !run.status.can_transition_to(next) {
            return Err(CacheError::InvalidRunTransition {
                from: run.status,
                to: next,
            });
        }
        run.status = next;
        if matches!(next, RunStatus::Starting | RunStatus::Running) && run.started_at.is_none() {
            run.started_at = Some(at);
        }
        if next.is_terminal() {
            run.ended_at = Some(at);
        }
        Ok(())
    };
    match event {
        RunEvent::State { status } => transition(run, *status)?,
        RunEvent::Started { session_id, model } => {
            // Cancellation can win after spawn but before the adapter emits
            // Started. Keep the committed Stopping state while retaining the
            // useful process metadata carried by this late event.
            if run.status != RunStatus::Stopping {
                transition(run, RunStatus::Running)?;
            }
            run.session_id = Some(session_id.clone());
            if let Some(model) = model.as_ref().filter(|model| !model.is_empty()) {
                run.model = model.clone();
            }
            run.latest_activity = Some("Started".into());
        }
        RunEvent::Text { text } => {
            let projection_truncated = append_bounded_result(&mut run.final_result, text);
            run.final_result_truncated |= source_truncated || projection_truncated;
            run.latest_activity = Some("Responding".into());
        }
        RunEvent::ToolCall { summary, .. } => run.latest_activity = Some(summary.clone()),
        RunEvent::ToolResult { summary, .. } if !summary.is_empty() => {
            run.latest_activity = Some(summary.clone())
        }
        RunEvent::ToolResult { .. } => {}
        RunEvent::PermissionDenied { tool, message } => {
            run.latest_activity = Some(match tool.as_deref().filter(|tool| !tool.is_empty()) {
                Some(tool) if !message.is_empty() => format!("{tool}: {message}"),
                Some(tool) => tool.to_string(),
                None => message.clone(),
            });
        }
        RunEvent::Usage {
            usage,
            cost_usd,
            duration_ms,
        } => {
            run.usage = Some(usage.clone());
            run.cost_usd = *cost_usd;
            run.duration_ms = *duration_ms;
        }
        RunEvent::Completed {
            session_id,
            result,
            cost_usd,
            duration_ms,
            usage,
        } => {
            transition(run, RunStatus::Completed)?;
            if let Some(session_id) = session_id {
                run.session_id = Some(session_id.clone());
            }
            let (result, projection_truncated) = bounded_string(result, MAX_RUN_RESULT_BYTES);
            run.final_result = Some(result);
            run.final_result_truncated = source_truncated || projection_truncated;
            run.cost_usd = *cost_usd;
            run.duration_ms = *duration_ms;
            run.usage = usage.clone();
            run.terminal_reason = Some(TerminalReason::Completed);
            run.latest_activity = Some("Completed".into());
        }
        RunEvent::Terminated {
            status,
            reason,
            message,
        } => {
            if !matches!(
                status,
                RunStatus::Failed | RunStatus::Stopped | RunStatus::Interrupted
            ) {
                return Err(CacheError::InvalidRunTransition {
                    from: run.status,
                    to: *status,
                });
            }
            transition(run, *status)?;
            run.terminal_reason = Some(*reason);
            run.error = Some(message.clone());
            run.latest_activity = Some(
                match status {
                    RunStatus::Stopped => "Stopped",
                    RunStatus::Interrupted => "Interrupted",
                    _ => "Failed",
                }
                .into(),
            );
        }
        RunEvent::LegacyStep { summary, .. } => run.latest_activity = Some(summary.clone()),
    }
    Ok(())
}

fn append_run_event_on(
    conn: &Connection,
    run_id: &str,
    event: &NewRunEvent,
) -> Result<RunEventAppend, CacheError> {
    if let Some(existing) = load_event_by_id(conn, run_id, &event.event_id)? {
        return Ok(RunEventAppend::Duplicate(existing));
    }
    let mut run = load_run_projection_on(conn, run_id)?
        .ok_or_else(|| CacheError::Corrupt(format!("no run {run_id}")))?;
    if run.status.is_terminal() {
        return Err(CacheError::TerminalRun(run_id.to_string()));
    }
    let mut event = event.clone();
    truncate_event(&mut event);
    apply_event_to_run(&mut run, &event.event, event.occurred_at, event.truncated)?;
    let sequence = run.last_event_sequence + 1;
    run.last_event_sequence = sequence;
    let stored = RunEventRecord {
        run_id: run_id.to_string(),
        sequence,
        event_id: event.event_id,
        occurred_at: event.occurred_at,
        event: event.event,
        truncated: event.truncated,
    };
    insert_event_row(conn, &stored)?;
    update_run_row(conn, &run)?;
    Ok(RunEventAppend::Appended(stored))
}

const MAX_RUN_EVENT_BYTES: usize = 64 * 1024;
const MAX_RUN_RESULT_BYTES: usize = 64 * 1024;
const MAX_RUN_ACTIVITY_BYTES: usize = 4 * 1024;
const TRUNCATION_MARKER: &str = "\n[…truncated…]";

fn bounded_string(value: &str, max: usize) -> (String, bool) {
    let mut value = value.to_string();
    let truncated = value.len() > max;
    truncate_string(&mut value, max);
    (value, truncated)
}

fn append_bounded_result(result: &mut Option<String>, text: &str) -> bool {
    let current = result.get_or_insert_with(String::new);
    if current.ends_with(TRUNCATION_MARKER) {
        return true;
    }
    let separator = if current.is_empty() { "" } else { "\n\n" };
    if current.len() + separator.len() + text.len() <= MAX_RUN_RESULT_BYTES {
        current.push_str(separator);
        current.push_str(text);
        return false;
    }
    current.push_str(separator);
    current.push_str(text);
    truncate_string(current, MAX_RUN_RESULT_BYTES);
    true
}

fn truncate_string(value: &mut String, max: usize) {
    if value.len() <= max {
        return;
    }
    let prefix_limit = max.saturating_sub(TRUNCATION_MARKER.len());
    let mut end = prefix_limit.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    if max >= TRUNCATION_MARKER.len() {
        value.push_str(TRUNCATION_MARKER);
    }
}

fn truncate_event(event: &mut NewRunEvent) {
    if serde_json::to_vec(&event.event).is_ok_and(|json| json.len() <= MAX_RUN_EVENT_BYTES) {
        return;
    }
    match &mut event.event {
        RunEvent::Text { text } => truncate_string(text, 60 * 1024),
        RunEvent::ToolCall { summary, .. }
        | RunEvent::ToolResult { summary, .. }
        | RunEvent::LegacyStep { summary, .. } => truncate_string(summary, 60 * 1024),
        RunEvent::Completed { result, .. } => truncate_string(result, 60 * 1024),
        RunEvent::Terminated { message, .. } => truncate_string(message, 60 * 1024),
        RunEvent::PermissionDenied { message, .. } => truncate_string(message, 60 * 1024),
        _ => {}
    }
    // JSON escaping can make a byte-bounded string several times larger.
    // Bound every string too, shrinking again only when the encoded payload
    // still exceeds the storage contract.
    for max in [8 * 1024, 2 * 1024, 512] {
        if serde_json::to_vec(&event.event).is_ok_and(|json| json.len() <= MAX_RUN_EVENT_BYTES) {
            break;
        }
        truncate_event_strings(&mut event.event, max);
    }
    event.truncated = true;
}

fn truncate_event_strings(event: &mut RunEvent, max: usize) {
    match event {
        RunEvent::State { .. } | RunEvent::Usage { .. } => {}
        RunEvent::Started { session_id, model } => {
            truncate_string(session_id, max);
            if let Some(model) = model {
                truncate_string(model, max);
            }
        }
        RunEvent::Text { text } => truncate_string(text, max),
        RunEvent::ToolCall {
            id,
            name,
            summary,
            parent_id,
        } => {
            truncate_string(id, max);
            truncate_string(name, max);
            truncate_string(summary, max);
            if let Some(parent_id) = parent_id {
                truncate_string(parent_id, max);
            }
        }
        RunEvent::ToolResult { id, summary, .. } => {
            truncate_string(id, max);
            truncate_string(summary, max);
        }
        RunEvent::Completed {
            session_id, result, ..
        } => {
            if let Some(session_id) = session_id {
                truncate_string(session_id, max);
            }
            truncate_string(result, max);
        }
        RunEvent::Terminated { message, .. } => truncate_string(message, max),
        RunEvent::PermissionDenied { tool, message } => {
            if let Some(tool) = tool {
                truncate_string(tool, max);
            }
            truncate_string(message, max);
        }
        RunEvent::LegacyStep { summary, .. } => truncate_string(summary, max),
    }
}

/// Bridge for the pre-event app while it is moved to `append_run_event`.
fn sync_legacy_steps(conn: &Connection, run: &RunRecord) -> Result<(), CacheError> {
    conn.execute(
        "DELETE FROM run_events WHERE run_id=?1 AND kind='legacy_step'",
        params![run.id],
    )?;
    let mut sequence: u64 = conn.query_row(
        "SELECT COALESCE(max(sequence), 0) FROM run_events WHERE run_id=?1",
        params![run.id],
        |row| row.get::<_, i64>(0),
    )? as u64;
    for (index, step) in run.steps.iter().enumerate() {
        sequence += 1;
        insert_event_row(
            conn,
            &RunEventRecord {
                run_id: run.id.clone(),
                sequence,
                event_id: format!("compat-step-{sequence}-{index}"),
                occurred_at: run.started_at.unwrap_or(run.queued_at),
                event: RunEvent::LegacyStep {
                    summary: step.summary.clone(),
                    ok: step.ok,
                },
                truncated: false,
            },
        )?;
    }
    conn.execute(
        "UPDATE runs SET last_event_sequence=?2, latest_activity=COALESCE(?3, latest_activity) WHERE id=?1",
        params![run.id, sequence as i64, run.steps.last().map(|step| step.summary.as_str())],
    )?;
    Ok(())
}

/// Insert a message on `conn`, and the run it carries when that is new.
fn insert_comment_message_on(conn: &Connection, m: &CommentMessage) -> Result<(), CacheError> {
    if let Some(run) = &m.run {
        if load_run_on(conn, &run.id)?.is_none() {
            insert_run_row(conn, run)?;
            sync_legacy_steps(conn, run)?;
        }
    }
    insert_comment_message_row(conn, m)
}

fn insert_comment_message_row(
    conn: &Connection,
    message: &CommentMessage,
) -> Result<(), CacheError> {
    let (kind, author) = comments::sql::author_columns(&message.author);
    let model = match &message.author {
        Author::Agent { model, .. } => Some(model.clone()),
        _ => None,
    };
    conn.execute(
        "INSERT INTO comment_messages
         (id, thread_id, origin, github_id, author_kind, author, author_model, body, run_id,
          created_at, node_id, html_url, diff_hunk, reactions_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            message.id,
            message.thread_id,
            message.origin.name(),
            message.github_id.map(|id| id.to_string()),
            kind,
            author,
            model,
            message.body,
            message.run.as_ref().map(|run| run.id.as_str()),
            message.created_at.to_string(),
            message.node_id,
            message.html_url,
            message.diff_hunk,
            reactions_json(&message.reactions),
        ],
    )?;
    Ok(())
}

/// Attach a newly admitted discussion run to its durable request. When the
/// request already points at an earlier attempt, preserve that attempt as its
/// own inline row before moving the request to the new run. This runs inside
/// the admission transaction, so a crash cannot leave a queued run detached
/// from its discussion.
fn link_admitted_discussion_run(conn: &Connection, run: &RunRecord) -> Result<(), CacheError> {
    let RunOwner::Discussion {
        thread_id,
        request_id,
        ..
    } = &run.owner
    else {
        return Ok(());
    };

    let request = conn
        .query_row(
            "SELECT thread_id, author_kind, run_id FROM comment_messages WHERE id=?1",
            params![request_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| CacheError::Corrupt(format!("no comment message {request_id}")))?;
    if request.0 != *thread_id || request.1 != "user" {
        return Err(CacheError::Corrupt(format!(
            "request {request_id} is not the run's user message"
        )));
    }

    if let Some(previous_id) = request.2.as_deref().filter(|id| *id != run.id) {
        if run.retry_of.is_none() {
            return Err(CacheError::Corrupt(format!(
                "request {request_id} already points at run {previous_id}"
            )));
        }
        let previous = load_run_on(conn, previous_id)?
            .ok_or_else(|| CacheError::Corrupt(format!("no run {previous_id}")))?;
        if !matches!(
            &previous.owner,
            RunOwner::Discussion {
                thread_id: owner_thread,
                request_id: owner_request,
                ..
            } if owner_thread == thread_id && owner_request == request_id
        ) {
            return Err(CacheError::Corrupt(format!(
                "run {previous_id} does not belong to request {request_id}"
            )));
        }
        let represented: bool = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM comment_messages
                 WHERE id<>?1 AND thread_id=?2 AND run_id=?3
             )",
            params![request_id, thread_id, previous_id],
            |row| row.get(0),
        )?;
        if !represented {
            insert_comment_message_row(
                conn,
                &CommentMessage {
                    id: comments::new_id(),
                    thread_id: thread_id.clone(),
                    origin: comments::ThreadOrigin::Local,
                    author: Author::Agent {
                        backend: previous.backend.clone(),
                        model: previous.model.clone(),
                    },
                    body: String::new(),
                    // Preserve the attempt at the point where it happened,
                    // rather than appending it after newer conversation.
                    created_at: previous.ended_at.unwrap_or(previous.queued_at),
                    run: Some(previous),
                    github_id: None,
                    node_id: None,
                    html_url: None,
                    diff_hunk: None,
                    reactions: Reactions::default(),
                },
            )?;
        }
    }

    let changed = conn.execute(
        "UPDATE comment_messages SET run_id=?3
         WHERE id=?1 AND thread_id=?2 AND author_kind='user'",
        params![request_id, thread_id, run.id],
    )?;
    if changed != 1 {
        return Err(CacheError::Corrupt(format!(
            "request {request_id} could not be attached to run {}",
            run.id
        )));
    }
    Ok(())
}

impl Cache {
    pub fn open(db_path: &Path, avatar_dir: Option<PathBuf>) -> Result<Self, CacheError> {
        let (cache, _) = Self::open_with_outcome(db_path, avatar_dir)?;
        Ok(cache)
    }

    /// Compatibility entry point for callers written before preserving
    /// migrations. The boolean used to mean "reset"; resets no longer happen,
    /// so it is always false. New callers should use `open_with_outcome`.
    pub fn open_checked(
        db_path: &Path,
        avatar_dir: Option<PathBuf>,
    ) -> Result<(Self, bool), CacheError> {
        let (cache, _) = Self::open_with_outcome(db_path, avatar_dir)?;
        Ok((cache, false))
    }

    pub fn open_with_outcome(
        db_path: &Path,
        avatar_dir: Option<PathBuf>,
    ) -> Result<(Self, CacheOpen), CacheError> {
        let conn = Connection::open(db_path)?;
        Self::from_conn(conn, avatar_dir)
    }

    /// In-memory cache, used by tests.
    pub fn open_in_memory() -> Result<Self, CacheError> {
        Self::from_conn(Connection::open_in_memory()?, None).map(|(cache, _)| cache)
    }

    fn from_conn(
        mut conn: Connection,
        avatar_dir: Option<PathBuf>,
    ) -> Result<(Self, CacheOpen), CacheError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let outcome = initialize_schema(&mut conn)?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        Ok((Self { conn, avatar_dir }, outcome))
    }

    // -----------------------------------------------------------------
    // GitHub notification snapshot
    // -----------------------------------------------------------------

    /// Atomically replace the cached notification list and its polling
    /// validator, retaining local Done marks for the exact revisions they
    /// dismissed. Newer activity clears the mark and lets the row return.
    ///
    /// The API result is a snapshot, so rows missing from `threads` must be
    /// removed rather than accumulated. Keeping the validator in the same
    /// transaction prevents a crash from pairing an older list with a newer
    /// `Last-Modified` value (or the reverse).
    pub fn store_threads(
        &self,
        threads: &[Thread],
        last_modified: Option<&str>,
    ) -> Result<Vec<Thread>, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let dismissals = {
            let mut stmt = tx.prepare(
                "SELECT thread_id, updated_at FROM notification_dismissals",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut dismissals = HashMap::new();
            for row in rows {
                let (thread_id, updated_at) = row?;
                let updated_at = updated_at.parse::<Timestamp>().map_err(|error| {
                    CacheError::Corrupt(format!(
                        "notification dismissal {thread_id} has invalid updated_at: {error}"
                    ))
                })?;
                dismissals.insert(thread_id, updated_at);
            }
            dismissals
        };
        let mut visible = Vec::with_capacity(threads.len());
        for thread in threads {
            match dismissals.get(&thread.id) {
                Some(dismissed_at) if thread.updated_at <= *dismissed_at => {}
                Some(_) => {
                    tx.execute(
                        "DELETE FROM notification_dismissals WHERE thread_id=?1",
                        params![thread.id],
                    )?;
                    visible.push(thread.clone());
                }
                None => visible.push(thread.clone()),
            }
        }

        tx.execute("DELETE FROM threads", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO threads (position, id, updated_at, json)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (position, thread) in visible.iter().enumerate() {
                stmt.execute(params![
                    position as i64,
                    thread.id,
                    thread.updated_at.to_string(),
                    serde_json::to_string(thread)?
                ])?;
            }
        }

        if let Some(last_modified) = last_modified {
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                params![NOTIFICATIONS_LAST_MODIFIED_KEY, last_modified],
            )?;
        } else {
            tx.execute(
                "DELETE FROM meta WHERE key = ?1",
                params![NOTIFICATIONS_LAST_MODIFIED_KEY],
            )?;
        }
        tx.commit()?;
        Ok(visible)
    }

    /// Remember that this exact notification revision was marked Done in
    /// Perch. The mark deliberately outlives the replaceable thread snapshot.
    pub fn dismiss_notification(
        &self,
        thread_id: &str,
        updated_at: Timestamp,
    ) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let previous = tx
            .query_row(
                "SELECT updated_at FROM notification_dismissals WHERE thread_id=?1",
                params![thread_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| {
                value.parse::<Timestamp>().map_err(|error| {
                    CacheError::Corrupt(format!(
                        "notification dismissal {thread_id} has invalid updated_at: {error}"
                    ))
                })
            })
            .transpose()?;
        let dismissed_at = previous
            .filter(|previous| *previous > updated_at)
            .unwrap_or(updated_at);
        tx.execute(
            "INSERT INTO notification_dismissals (thread_id, updated_at)
             VALUES (?1, ?2)
             ON CONFLICT(thread_id) DO UPDATE SET updated_at=excluded.updated_at",
            params![thread_id, dismissed_at.to_string()],
        )?;
        let cached_updated_at = tx
            .query_row(
                "SELECT updated_at FROM threads WHERE id=?1",
                params![thread_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| {
                value.parse::<Timestamp>().map_err(|error| {
                    CacheError::Corrupt(format!(
                        "notification {thread_id} has invalid updated_at: {error}"
                    ))
                })
            })
            .transpose()?;
        if cached_updated_at.is_some_and(|cached| cached <= dismissed_at) {
            tx.execute("DELETE FROM threads WHERE id=?1", params![thread_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Load the cached list in the same order GitHub sent. Applying the Done
    /// marks here closes the small crash window between recording a dismissal
    /// and rewriting the replaceable snapshot.
    pub fn load_threads(&self) -> Result<Vec<Thread>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM threads ORDER BY position")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut threads: Vec<Thread> = Vec::new();
        for row in rows {
            threads.push(serde_json::from_str(&row?)?);
        }
        let dismissals = self.notification_dismissals()?;
        threads.retain(|thread| {
            dismissals
                .get(&thread.id)
                .is_none_or(|dismissed_at| thread.updated_at > *dismissed_at)
        });
        Ok(threads)
    }

    fn notification_dismissals(&self) -> Result<HashMap<String, Timestamp>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT thread_id, updated_at FROM notification_dismissals")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut dismissals = HashMap::new();
        for row in rows {
            let (thread_id, updated_at) = row?;
            let updated_at = updated_at.parse::<Timestamp>().map_err(|error| {
                CacheError::Corrupt(format!(
                    "notification dismissal {thread_id} has invalid updated_at: {error}"
                ))
            })?;
            dismissals.insert(thread_id, updated_at);
        }
        Ok(dismissals)
    }

    /// The validator paired atomically with [`Self::load_threads`].
    pub fn notifications_last_modified(&self) -> Result<Option<String>, CacheError> {
        self.meta(NOTIFICATIONS_LAST_MODIFIED_KEY)
    }

    // -----------------------------------------------------------------
    // Comment threads
    // -----------------------------------------------------------------

    /// Every thread in a worktree, with its messages and their runs.
    pub fn load_comment_threads(
        &self,
        worktree_id: &str,
    ) -> Result<Vec<CommentThread>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, origin, path, start_line, line, side, commit_sha, outdated, resolved,
                    created_at, github_id
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
                r.get::<_, Option<String>>(10)?,
            ))
        })?;

        let mut threads = Vec::new();
        for row in rows {
            let (id, origin, path, start, line, side, sha, outdated, resolved, created, github_id) =
                row?;
            let Ok(created_at) = created.parse::<Timestamp>() else {
                continue;
            };
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
                github_id: github_id_of(github_id),
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
            "SELECT id, origin, author_kind, author, author_model, body, run_id, created_at,
                    github_id, node_id, html_url, diff_hunk, reactions_json
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
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<String>>(12)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                origin,
                kind,
                author,
                model,
                body,
                run_id,
                created,
                github_id,
                node_id,
                html_url,
                diff_hunk,
                reactions,
            ) = row?;
            let Ok(created_at) = created.parse::<Timestamp>() else {
                continue;
            };
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
                github_id: github_id_of(github_id),
                node_id,
                html_url,
                diff_hunk,
                reactions: reactions_of(reactions),
            });
        }
        Ok(out)
    }

    pub fn load_run(&self, id: &str) -> Result<Option<RunRecord>, CacheError> {
        load_run_on(&self.conn, id)
    }

    /// Insert a thread with its messages, in one transaction: a failure
    /// part way leaves no thread with nothing under it.
    pub fn insert_comment_thread(&self, t: &CommentThread) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO comment_threads
             (id, worktree_id, origin, github_id, path, start_line, line, side, commit_sha,
              outdated, resolved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                t.id,
                t.worktree_id,
                t.origin.name(),
                t.github_id.map(|id| id.to_string()),
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
            insert_comment_message_on(&tx, m)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Insert a message. A run it carries is inserted too if it is new.
    pub fn insert_comment_message(&self, m: &CommentMessage) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        insert_comment_message_on(&tx, m)?;
        tx.commit()?;
        Ok(())
    }

    /// Keep a draft under its thread. A thread of Perch's own turns
    /// towards GitHub with it, since the draft will be a new review
    /// thread there at its anchor; one transaction, so a thread is never
    /// turned with no draft under it.
    pub fn insert_draft(&self, m: &CommentMessage) -> Result<(), CacheError> {
        if !m.is_draft() {
            return Err(CacheError::Corrupt(format!(
                "comment message {} is not a draft",
                m.id
            )));
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE comment_threads SET origin = 'github' WHERE id = ?1 AND origin = 'local'",
            params![m.thread_id],
        )?;
        insert_comment_message_on(&tx, m)?;
        tx.commit()?;
        Ok(())
    }

    /// Rewrite a draft's body, its time unchanged. The only body Perch
    /// rewrites is a draft's: an agent's answer and GitHub's comments
    /// are what they are, and asking for one is refused.
    pub fn update_comment_message_body(&self, id: &str, body: &str) -> Result<(), CacheError> {
        let changed = self.conn.execute(
            "UPDATE comment_messages SET body = ?2
             WHERE id = ?1 AND origin = 'github' AND github_id IS NULL AND author_kind = 'user'",
            params![id, body],
        )?;
        if changed == 0 {
            return Err(CacheError::Corrupt(format!("no draft {id}")));
        }
        Ok(())
    }

    /// Turn a thread towards GitHub, or back into one of Perch's own.
    pub fn set_thread_origin(&self, id: &str, origin: ThreadOrigin) -> Result<(), CacheError> {
        let changed = self.conn.execute(
            "UPDATE comment_threads SET origin = ?2 WHERE id = ?1",
            params![id, origin.name()],
        )?;
        if changed == 0 {
            return Err(CacheError::Corrupt(format!("no comment thread {id}")));
        }
        Ok(())
    }

    /// Throw a draft away, in one transaction with what follows from it.
    /// A thread that was only on its way to GitHub for the draft, with
    /// nothing else under it, goes too, unless a review finding owns it,
    /// when it turns back into Perch's own instead; one with only Perch's
    /// messages left turns back as well. A thread that is on GitHub is
    /// left as it is. A message that is not a draft is not touched.
    pub fn delete_draft(&self, message_id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let thread_id: Option<String> = tx
            .query_row(
                "SELECT thread_id FROM comment_messages
                 WHERE id = ?1 AND origin = 'github' AND github_id IS NULL AND author_kind = 'user'",
                params![message_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(thread_id) = thread_id else {
            return Err(CacheError::Corrupt(format!("no draft {message_id}")));
        };
        tx.execute(
            "DELETE FROM comment_messages WHERE id = ?1",
            params![message_id],
        )?;
        let on_its_way: bool = tx.query_row(
            "SELECT origin = 'github' AND github_id IS NULL FROM comment_threads WHERE id = ?1",
            params![thread_id],
            |row| row.get(0),
        )?;
        if !on_its_way {
            tx.commit()?;
            return Ok(());
        }
        let (left, github_left, finding): (i64, i64, bool) = tx.query_row(
            "SELECT COUNT(*), COUNT(CASE WHEN origin = 'github' THEN 1 END),
                    EXISTS(SELECT 1 FROM review_findings WHERE thread_id = ?1)
             FROM comment_messages WHERE thread_id = ?1",
            params![thread_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if left == 0 && !finding {
            tx.execute(
                "DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE owner_thread_id=?1)",
                params![thread_id],
            )?;
            tx.execute("DELETE FROM runs WHERE owner_thread_id = ?1", params![thread_id])?;
            tx.execute("DELETE FROM comment_threads WHERE id = ?1", params![thread_id])?;
        } else if github_left == 0 {
            tx.execute(
                "UPDATE comment_threads SET origin = 'local' WHERE id = ?1",
                params![thread_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Publish a completed discussion answer and detach its run from the
    /// originating user request in one transaction. The request must still
    /// point at this exact run; a stale completion cannot clear a newer
    /// attempt's link.
    pub fn insert_completed_reply(
        &self,
        request_message_id: &str,
        reply: &CommentMessage,
    ) -> Result<(), CacheError> {
        if !matches!(&reply.author, Author::Agent { .. }) {
            return Err(CacheError::Corrupt(
                "a completed reply must have an agent author".into(),
            ));
        }
        let claimed_run = reply.run.as_ref().ok_or_else(|| {
            CacheError::Corrupt("a completed reply must reference its run".into())
        })?;
        if claimed_run.status != RunStatus::Completed {
            return Err(CacheError::Corrupt(
                "a completed reply must reference a completed run".into(),
            ));
        }

        let tx = self.conn.unchecked_transaction()?;
        let stored_run = load_run_on(&tx, &claimed_run.id)?
            .ok_or_else(|| CacheError::Corrupt(format!("no run {}", claimed_run.id)))?;
        if stored_run.status != RunStatus::Completed {
            return Err(CacheError::Corrupt(format!(
                "run {} is not completed",
                stored_run.id
            )));
        }
        match &stored_run.owner {
            RunOwner::Discussion {
                thread_id,
                request_id,
                ..
            } if thread_id == &reply.thread_id && request_id == request_message_id => {}
            _ => {
                return Err(CacheError::Corrupt(
                    "the reply does not match the run's discussion request".into(),
                ));
            }
        }

        insert_comment_message_row(&tx, reply)?;
        let cleared = tx.execute(
            "UPDATE comment_messages SET run_id = NULL
             WHERE id = ?1 AND thread_id = ?2 AND author_kind = 'user' AND run_id = ?3",
            params![request_message_id, reply.thread_id, stored_run.id],
        )?;
        if cleared != 1 {
            return Err(CacheError::Corrupt(format!(
                "request {request_message_id} no longer points at run {}",
                stored_run.id
            )));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn insert_run(&self, r: &RunRecord) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        insert_run_row(&tx, r)?;
        sync_legacy_steps(&tx, r)?;
        tx.commit()?;
        Ok(())
    }

    /// Persist a newly admitted run and its first event atomically. A
    /// discussion run is also linked to its request in this transaction; a
    /// retry preserves the request's previous attempt as a separate inline
    /// row before replacing the link. If any part fails, no queued row or
    /// partial discussion edit is left behind.
    pub fn insert_run_with_event(
        &self,
        run: &RunRecord,
        event: &NewRunEvent,
    ) -> Result<RunEventAppend, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        insert_run_row(&tx, run)?;
        sync_legacy_steps(&tx, run)?;
        let appended = append_run_event_on(&tx, &run.id, event)?;
        link_admitted_discussion_run(&tx, run)?;
        tx.commit()?;
        Ok(appended)
    }

    /// Rewrite every column of a run that can move.
    pub fn update_run(&self, r: &RunRecord) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        update_run_row(&tx, r)?;
        sync_legacy_steps(&tx, r)?;
        tx.commit()?;
        Ok(())
    }

    /// Nonterminal runs, in scheduler order.
    pub fn active_runs(&self) -> Result<Vec<RunRecord>, CacheError> {
        self.load_runs_where(
            "status IN ('queued','starting','running','stopping') ORDER BY queued_at, id",
            [],
        )
    }

    /// Nonterminal scalar projections, in scheduler order. This query does
    /// not select final output and never touches `run_events`.
    pub fn active_run_summaries(&self) -> Result<Vec<RunSummary>, CacheError> {
        let sql = format!(
            "SELECT {RUN_SUMMARY_COLUMNS} FROM runs
             WHERE status IN ('queued','starting','running','stopping')
             ORDER BY queued_at, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map([], read_run_summary_db_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(summary_from_db).collect()
    }

    /// One bounded page of terminal scalar projections, newest first. The
    /// returned cursor is exclusive and can be supplied repeatedly to walk
    /// arbitrarily large histories without loading any transcript.
    pub fn run_history_page(
        &self,
        filter: &RunHistoryFilter,
        cursor: Option<&RunHistoryCursor>,
        limit: usize,
    ) -> Result<RunHistoryPage, CacheError> {
        let limit = limit.clamp(1, 500);
        let ended = cursor.map(|cursor| cursor.ended_at.to_string());
        let cursor_id = cursor.map(|cursor| cursor.id.as_str());
        let sql = format!(
            "SELECT {RUN_SUMMARY_COLUMNS} FROM runs
             WHERE status IN ('completed','failed','stopped','interrupted')
               AND (?1 IS NULL OR owner_item_id=?1)
               AND (?2 IS NULL OR backend=?2)
               AND (?3 IS NULL OR status=?3)
               AND (?4 IS NULL OR ended_at < ?4 OR (ended_at = ?4 AND id < ?5))
             ORDER BY ended_at DESC, id DESC LIMIT ?6"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(
                params![
                    filter.item_id.as_deref(),
                    filter.backend.as_deref(),
                    filter.status.map(RunStatus::name),
                    ended,
                    cursor_id,
                    (limit + 1) as i64,
                ],
                read_run_summary_db_row,
            )?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        let mut runs = rows
            .into_iter()
            .map(summary_from_db)
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = runs.len() > limit;
        runs.truncate(limit);
        let next_cursor = if has_more {
            let last = runs
                .last()
                .ok_or_else(|| CacheError::Corrupt("empty history page".into()))?;
            Some(RunHistoryCursor {
                ended_at: last
                    .ended_at
                    .ok_or_else(|| CacheError::Corrupt("terminal run ended_at".into()))?,
                id: last.id.clone(),
            })
        } else {
            None
        };
        Ok(RunHistoryPage { runs, next_cursor })
    }

    /// Terminal runs newest first. The cursor is the last row of the
    /// previous page and is exclusive.
    pub fn run_history(
        &self,
        filter: &RunHistoryFilter,
        cursor: Option<&RunHistoryCursor>,
        limit: usize,
    ) -> Result<Vec<RunRecord>, CacheError> {
        let limit = limit.clamp(1, 500) as i64;
        let ended = cursor.map(|cursor| cursor.ended_at.to_string());
        let cursor_id = cursor.map(|cursor| cursor.id.as_str());
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs
             WHERE status IN ('completed','failed','stopped','interrupted')
               AND (?1 IS NULL OR owner_item_id=?1)
               AND (?2 IS NULL OR backend=?2)
               AND (?3 IS NULL OR status=?3)
               AND (?4 IS NULL OR ended_at < ?4 OR (ended_at = ?4 AND id < ?5))
             ORDER BY ended_at DESC, id DESC LIMIT ?6"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(
                params![
                    filter.item_id.as_deref(),
                    filter.backend.as_deref(),
                    filter.status.map(RunStatus::name),
                    ended,
                    cursor_id,
                    limit,
                ],
                read_run_db_row,
            )?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        self.finish_run_rows(rows)
    }

    pub fn runs_for_item(&self, item_id: &str) -> Result<Vec<RunRecord>, CacheError> {
        let sql =
            format!("SELECT {RUN_COLUMNS} FROM runs WHERE owner_item_id=?1 ORDER BY queued_at, id");
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params![item_id], read_run_db_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        self.finish_run_rows(rows)
    }

    /// Every discussion-owned run for one thread, in attempt order. The
    /// `runs_request` index serves this query by its `owner_thread_id` prefix.
    pub fn runs_for_discussion(&self, thread_id: &str) -> Result<Vec<RunRecord>, CacheError> {
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs
             WHERE owner_kind='discussion' AND owner_thread_id=?1
             ORDER BY queued_at, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params![thread_id], read_run_db_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        self.finish_run_rows(rows)
    }

    pub fn runs_for_request(
        &self,
        thread_id: &str,
        request_id: &str,
    ) -> Result<Vec<RunRecord>, CacheError> {
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs
             WHERE owner_kind='discussion' AND owner_thread_id=?1 AND owner_request_id=?2
             ORDER BY queued_at, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params![thread_id, request_id], read_run_db_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        self.finish_run_rows(rows)
    }

    /// An event page in chronological order. `before_sequence` is exclusive;
    /// `None` starts from the newest page.
    pub fn load_run_events(
        &self,
        run_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<RunEventRecord>, CacheError> {
        load_run_events_on(&self.conn, run_id, before_sequence, limit)
    }

    /// Persist an event and update the run projection in the same
    /// transaction. Reusing `event_id` returns the original record.
    pub fn append_run_event(
        &self,
        run_id: &str,
        event: &NewRunEvent,
    ) -> Result<RunEventAppend, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let appended = append_run_event_on(&tx, run_id, event)?;
        tx.commit()?;
        Ok(appended)
    }

    /// Reconcile work which survived in the database but not as an owned
    /// process. Nothing is replayed.
    pub fn interrupt_active_runs(
        &self,
        reason: TerminalReason,
        message: &str,
    ) -> Result<usize, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM runs WHERE status IN ('queued','starting','running','stopping') ORDER BY queued_at, id",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let now = Timestamp::now();
        for id in &ids {
            let mut run = load_run_projection_on(&tx, id)?
                .ok_or_else(|| CacheError::Corrupt(format!("no run {id}")))?;
            let event = RunEventRecord {
                run_id: id.clone(),
                sequence: run.last_event_sequence + 1,
                event_id: format!("interrupt-{}", run.last_event_sequence + 1),
                occurred_at: now,
                event: RunEvent::Terminated {
                    status: RunStatus::Interrupted,
                    reason,
                    message: message.to_string(),
                },
                truncated: false,
            };
            apply_event_to_run(&mut run, &event.event, now, false)?;
            run.last_event_sequence = event.sequence;
            insert_event_row(&tx, &event)?;
            update_run_row(&tx, &run)?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    fn finish_run_rows(&self, rows: Vec<RunDbRow>) -> Result<Vec<RunRecord>, CacheError> {
        rows.into_iter()
            .map(|row| {
                let steps = load_steps_on(&self.conn, &row.id)?;
                run_from_db(row, steps)
            })
            .collect()
    }

    fn load_runs_where<P>(&self, clause: &str, params: P) -> Result<Vec<RunRecord>, CacheError>
    where
        P: rusqlite::Params,
    {
        let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE {clause}");
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params, read_run_db_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        self.finish_run_rows(rows)
    }

    /// Point a message at the run that answered it, or clear it.
    pub fn set_message_run(
        &self,
        message_id: &str,
        run_id: Option<&str>,
    ) -> Result<(), CacheError> {
        let changed = self.conn.execute(
            "UPDATE comment_messages SET run_id = ?2 WHERE id = ?1",
            params![message_id, run_id],
        )?;
        if changed == 0 {
            return Err(CacheError::Corrupt(format!(
                "no comment message {message_id}"
            )));
        }
        Ok(())
    }

    pub fn set_thread_outdated(&self, id: &str, outdated: bool) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE comment_threads SET outdated = ?2 WHERE id = ?1",
            params![id, outdated as i64],
        )?;
        Ok(())
    }

    /// The item that owns a discussion, without loading its messages and
    /// run history. Used to fence deletion against concurrent admission.
    pub fn comment_thread_item_id(&self, id: &str) -> Result<Option<String>, CacheError> {
        self.conn
            .query_row(
                "SELECT worktree_id FROM comment_threads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(CacheError::from)
    }

    /// Remove a request that could not be admitted. Runs are linked only
    /// after admission, so this never removes run history.
    pub fn delete_comment_message(&self, id: &str) -> Result<(), CacheError> {
        self.conn
            .execute("DELETE FROM comment_messages WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_comment_thread(&self, id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let finding_thread: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM review_findings WHERE thread_id=?1)",
            params![id],
            |row| row.get(0),
        )?;
        if finding_thread {
            return Err(CacheError::ReviewConflict(format!(
                "comment thread {id} belongs to a review finding"
            )));
        }
        tx.execute(
            "DELETE FROM comment_messages WHERE thread_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE owner_thread_id=?1)",
            params![id],
        )?;
        tx.execute("DELETE FROM runs WHERE owner_thread_id = ?1", params![id])?;
        tx.execute("DELETE FROM comment_threads WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Delete what Perch put under a thread of GitHub's: the messages no
    /// one on GitHub can see, and their runs. Every message that went to
    /// GitHub, or is on its way, stays, and the thread with it. A thread
    /// whose root had already left GitHub, with only Perch's messages under
    /// it, has nothing left, and goes too rather than draw as an empty card.
    pub fn delete_private_messages(&self, thread_id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE owner_thread_id=?1)",
            params![thread_id],
        )?;
        tx.execute("DELETE FROM runs WHERE owner_thread_id = ?1", params![thread_id])?;
        tx.execute(
            "DELETE FROM comment_messages WHERE thread_id = ?1 AND origin = 'local'",
            params![thread_id],
        )?;
        tx.execute(
            "DELETE FROM comment_threads WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM comment_messages WHERE thread_id = ?1)",
            params![thread_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Compatibility wrapper for the old startup call.
    pub fn fail_running_runs(&self, reason: &str) -> Result<usize, CacheError> {
        self.interrupt_active_runs(TerminalReason::AppRestart, reason)
    }

    // -----------------------------------------------------------------
    // Items
    // -----------------------------------------------------------------

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

    pub fn find_item_by_branch(
        &self,
        repo_full_name: &str,
        branch: &str,
    ) -> Result<Option<Item>, CacheError> {
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
            return Err(CacheError::Corrupt(
                "an item needs a subject or a checkout".into(),
            ));
        }
        let c = ItemColumns::of(item)?;
        self.conn.execute(
            "INSERT INTO items
             (id, repo_full_name, subject_kind, subject_number, subject_json,
              branch, base_ref, head_sha, checkout_path, owned, on_disk, from_item_id, name,
              saved, pinned_at, archived_at, created_at, last_opened_at, additions, deletions,
              subject_repo, last_seen_head)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                item.id, item.repo_full_name,
                c.subject_kind, c.subject_number, c.subject_json,
                c.branch, c.base_ref, c.head_sha, c.checkout_path, c.owned, c.on_disk,
                item.from_item_id, item.name,
                item.saved as i64,
                item.pinned_at.map(|t| t.to_string()),
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
            return Err(CacheError::Corrupt(
                "an item needs a subject or a checkout".into(),
            ));
        }
        let c = ItemColumns::of(item)?;
        self.conn.execute(
            "UPDATE items SET
                repo_full_name = ?2, subject_kind = ?3, subject_number = ?4,
                subject_json = ?5, branch = ?6, base_ref = ?7, head_sha = ?8, checkout_path = ?9,
                owned = ?10, on_disk = ?11, from_item_id = ?12, name = ?13, saved = ?14,
                pinned_at = ?15, archived_at = ?16, last_opened_at = ?17,
                additions = ?18, deletions = ?19, subject_repo = ?20, last_seen_head = ?21
             WHERE id = ?1",
            params![
                item.id,
                item.repo_full_name,
                c.subject_kind,
                c.subject_number,
                c.subject_json,
                c.branch,
                c.base_ref,
                c.head_sha,
                c.checkout_path,
                c.owned,
                c.on_disk,
                item.from_item_id,
                item.name,
                item.saved as i64,
                item.pinned_at.map(|t| t.to_string()),
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
        review_store::delete_review_history_on(&tx, id)?;
        tx.execute(
            "DELETE FROM comment_messages WHERE thread_id IN (SELECT id FROM comment_threads WHERE worktree_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE owner_item_id = ?1)",
            params![id],
        )?;
        tx.execute("DELETE FROM runs WHERE owner_item_id = ?1", params![id])?;
        // An unavailable legacy run has no item id, but its retained thread
        // can still belong to this item.
        tx.execute(
            "DELETE FROM run_events WHERE run_id IN (
                 SELECT r.id FROM runs r JOIN comment_threads t ON t.id=r.owner_thread_id
                 WHERE t.worktree_id=?1
             )",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM runs WHERE owner_thread_id IN (SELECT id FROM comment_threads WHERE worktree_id=?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM comment_threads WHERE worktree_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM discussions WHERE item_id = ?1", params![id])?;
        tx.execute("DELETE FROM checks WHERE item_id = ?1", params![id])?;
        tx.execute("DELETE FROM viewed WHERE item_id = ?1", params![id])?;
        tx.execute("DELETE FROM reveals WHERE item_id = ?1", params![id])?;
        tx.execute(
            "UPDATE items SET from_item_id = NULL WHERE from_item_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // What was fetched for an item, and what the reader marked on it
    // -----------------------------------------------------------------

    /// Keep the discussion, replacing whatever was kept before, and its
    /// review threads as comment threads beside Perch's own. One row per
    /// item, so Conversation can paint before the network answers.
    pub fn store_discussion(
        &self,
        item_id: &str,
        discussion: &Discussion,
    ) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO discussions (item_id, fetched_at, json) VALUES (?1, ?2, ?3)",
            params![
                item_id,
                Timestamp::now().to_string(),
                serde_json::to_string(discussion)?
            ],
        )?;
        let head_sha = item_head_sha_on(&tx, item_id)?;
        let threads = comments::review_threads_of(discussion, item_id, &head_sha);
        ingest_review_threads(&tx, item_id, &threads)?;
        tx.commit()?;
        Ok(())
    }

    /// The kept discussion and when it was fetched. A row an older build
    /// wrote that no longer reads is `None`, the same as no row.
    pub fn load_discussion(
        &self,
        item_id: &str,
    ) -> Result<Option<(Timestamp, Discussion)>, CacheError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT fetched_at, json FROM discussions WHERE item_id = ?1",
                params![item_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((fetched_at, json)) = row else {
            return Ok(None);
        };
        let (Ok(fetched_at), Ok(discussion)) = (
            fetched_at.parse::<Timestamp>(),
            serde_json::from_str::<Discussion>(&json),
        ) else {
            tracing::warn!(item_id, "skipping unreadable cached discussion");
            return Ok(None);
        };
        Ok(Some((fetched_at, discussion)))
    }

    /// Keep the checks for the head they were read at, replacing whatever
    /// was kept before.
    pub fn store_checks(
        &self,
        item_id: &str,
        head_sha: &str,
        checks: &Checks,
    ) -> Result<(), CacheError> {
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
    pub fn set_viewed(
        &self,
        item_id: &str,
        path: &str,
        blob: &str,
        viewed: bool,
    ) -> Result<(), CacheError> {
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
        let mut stmt = self
            .conn
            .prepare("SELECT path, blob FROM viewed WHERE item_id = ?1")?;
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
    pub fn set_reveal(
        &self,
        item_id: &str,
        path: &str,
        blob: &str,
        first: u32,
        last: u32,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO reveals (item_id, path, blob, first, last) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![item_id, path, blob, first, last],
        )?;
        Ok(())
    }

    /// Forget every reveal on a file's content that overlaps `first` to
    /// `last`, inclusive: a divider's Reset, which folds its run back to
    /// the default by taking every reveal the run has a piece of.
    pub fn remove_reveals(
        &self,
        item_id: &str,
        path: &str,
        blob: &str,
        first: u32,
        last: u32,
    ) -> Result<(), CacheError> {
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
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, u32>(3)?,
            ))
        })?;
        let mut out = StoredReveals::new();
        for row in rows {
            let (path, blob, first, last) = row?;
            out.entry((path, blob)).or_default().push((first, last));
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

const ITEM_COLUMNS: &str = "id, repo_full_name, subject_kind, subject_number, subject_json,
    branch, base_ref, head_sha, checkout_path, owned, on_disk, from_item_id, name, saved,
    pinned_at, archived_at, created_at, last_opened_at, additions, deletions, last_seen_head";

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
            subject_repo: item
                .subject
                .as_ref()
                .map(|s| s.repo_full_name().to_lowercase()),
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
        base_ref: r
            .get::<_, Option<String>>(6)
            .ok()
            .flatten()
            .unwrap_or_default(),
        head_sha: r
            .get::<_, Option<String>>(7)
            .ok()
            .flatten()
            .unwrap_or_default(),
        path: PathBuf::from(
            r.get::<_, Option<String>>(8)
                .ok()
                .flatten()
                .unwrap_or_default(),
        ),
        owned: r.get::<_, i64>(9).unwrap_or(1) != 0,
        on_disk: r.get::<_, i64>(10).unwrap_or(1) != 0,
        churn: match (
            r.get::<_, Option<i64>>(18).ok().flatten(),
            r.get::<_, Option<i64>>(19).ok().flatten(),
        ) {
            (Some(a), Some(d)) => Some((a as u32, d as u32)),
            _ => None,
        },
        last_seen_head: r.get::<_, Option<String>>(20).ok().flatten(),
    });
    let created: String = r.get(16)?;
    let opened: String = r.get(17)?;
    Ok(Item {
        id: r.get(0)?,
        repo_full_name: r.get(1)?,
        subject,
        checkout,
        from_item_id: r.get(11)?,
        name: r.get(12)?,
        saved: r.get::<_, i64>(13)? != 0,
        pinned_at: stamp(r.get(14)?),
        archived_at: stamp(r.get(15)?),
        created_at: created.parse().unwrap_or(Timestamp::UNIX_EPOCH),
        last_opened_at: opened.parse().unwrap_or(Timestamp::UNIX_EPOCH),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{RunStatus, Side, ThreadOrigin};
    use crate::items::{Subject as ItemSubject, SubjectKind as ItemKind};
    use crate::model::{
        Reason, Repository, Subject as NotificationSubject, SubjectKind as NotificationSubjectKind,
    };
    use crate::pulls::PullRequest;

    fn pull_request(number: u64) -> PullRequest {
        PullRequest {
            number,
            title: "Add a thing".into(),
            html_url: format!("https://github.com/acme/widgets/pull/{number}"),
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

    /// Tests which start from today's schema and then label the database as
    /// an older run-schema version must also remove schema 18's review
    /// objects. Otherwise the 17 -> 18 migration correctly tries to create
    /// tables which the synthetic fixture accidentally left behind.
    fn remove_review_schema(cache: &Cache) {
        cache
            .conn
            .execute_batch(
                "DROP TABLE finding_audit_entries;
                 DROP TABLE review_anchor_mappings;
                 DROP TABLE review_observations;
                 DROP TABLE review_findings;
                 DROP TABLE reviewer_attempts;
                 DROP TABLE review_groups;
                 DROP TABLE review_preferences;
                 DROP INDEX comment_threads_anchor_snapshot;
                 ALTER TABLE comment_threads DROP COLUMN anchor_snapshot_id;
                 ALTER TABLE comment_threads DROP COLUMN anchor_blob_sha;
                 DROP TABLE review_snapshots;",
            )
            .unwrap();
    }

    fn notification_thread(id: &str, updated_at: &str) -> Thread {
        Thread {
            id: id.into(),
            unread: true,
            local_unread: false,
            local_read: false,
            reason: Reason::Subscribed,
            updated_at: stamp(updated_at),
            last_read_at: None,
            subject: NotificationSubject {
                title: format!("thread {id}"),
                kind: NotificationSubjectKind::PullRequest,
                url: Some(format!(
                    "https://api.github.com/repos/acme/widgets/pulls/{id}"
                )),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "acme/widgets".into(),
                owner_avatar_url: "https://avatars/1".into(),
            },
            subject_snapshot: None,
            run_snapshot: None,
        }
    }

    fn run(id: &str, thread: &str, status: RunStatus) -> RunRecord {
        let at = stamp("2026-09-18T10:00:00Z");
        RunRecord {
            id: id.into(),
            owner: RunOwner::Discussion {
                item_id: "w".into(),
                thread_id: thread.into(),
                request_id: format!("{thread}-m1"),
            },
            purpose: RunPurpose::Question,
            backend: "claude-code".into(),
            executable: None,
            executable_version: None,
            requested_model: "sonnet".into(),
            model: "sonnet".into(),
            requested_effort: "medium".into(),
            effort: "medium".into(),
            session_id: Some("sess".into()),
            original_request: "why?".into(),
            prompt_snapshot: Some("full prompt: why?".into()),
            working_directory: None,
            revision: RevisionContext::Unknown,
            labels: RunLabels::default(),
            status,
            terminal_reason: status
                .is_terminal()
                .then_some(if status == RunStatus::Completed {
                    TerminalReason::Completed
                } else {
                    TerminalReason::Other
                }),
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            usage: None,
            retry_of: None,
            steps: vec![
                RunStep {
                    summary: "Read src/x.rs".into(),
                    ok: true,
                },
                RunStep {
                    summary: "Asked to edit src/x.rs".into(),
                    ok: false,
                },
            ],
            error: None,
            queued_at: at,
            started_at: Some(at),
            ended_at: status
                .is_terminal()
                .then_some(stamp("2026-09-18T10:00:14Z")),
            final_result: None,
            final_result_truncated: false,
            latest_activity: None,
            last_event_sequence: 0,
        }
    }

    fn comment_thread(id: &str, worktree: &str) -> CommentThread {
        CommentThread {
            id: id.into(),
            worktree_id: worktree.into(),
            origin: ThreadOrigin::Local,
            github_id: None,
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
                    github_id: None,
                    node_id: None,
                    html_url: None,
                    diff_hunk: None,
                    reactions: Reactions::default(),
                },
                CommentMessage {
                    id: format!("{id}-m2"),
                    thread_id: id.into(),
                    origin: ThreadOrigin::Local,
                    author: Author::Agent {
                        backend: "claude-code".into(),
                        model: "sonnet".into(),
                    },
                    body: "because".into(),
                    created_at: stamp("2026-09-18T10:00:14Z"),
                    run: Some(run(&format!("{id}-r1"), id, RunStatus::Completed)),
                    github_id: None,
                    node_id: None,
                    html_url: None,
                    diff_hunk: None,
                    reactions: Reactions::default(),
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
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, t.id);
        assert_eq!(loaded[0].messages.len(), 2);
        let loaded_run = loaded[0].messages[1].run.as_ref().unwrap();
        assert_eq!(loaded_run.steps, t.messages[1].run.as_ref().unwrap().steps);
        assert_eq!(loaded_run.last_event_sequence, 2);
        assert!(c.load_comment_threads("other").unwrap().is_empty());
    }

    #[test]
    fn cache_connections_enforce_foreign_key_actions() {
        let c = Cache::open_in_memory().unwrap();
        let enabled: i64 = c
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(enabled, 1);

        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        assert_eq!(c.load_run_events("t1-r1", None, 10).unwrap().len(), 2);

        c.conn
            .execute("DELETE FROM runs WHERE id = ?1", params!["t1-r1"])
            .unwrap();

        let remaining_events: i64 = c
            .conn
            .query_row(
                "SELECT count(*) FROM run_events WHERE run_id = ?1",
                params!["t1-r1"],
                |row| row.get(0),
            )
            .unwrap();
        let message_run_id: Option<String> = c
            .conn
            .query_row(
                "SELECT run_id FROM comment_messages WHERE id = ?1",
                params!["t1-m2"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_events, 0, "run events cascade with their run");
        assert_eq!(
            message_run_id, None,
            "answer messages survive with their run reference cleared"
        );
    }

    #[test]
    fn steps_round_trip_through_update() {
        let c = Cache::open_in_memory().unwrap();
        let t = comment_thread("t1", "w");
        c.insert_comment_thread(&t).unwrap();
        let mut r = run("t1-r1", "t1", RunStatus::Completed);
        r.steps.push(RunStep {
            summary: "Ran git log".into(),
            ok: true,
        });
        r.cost_usd = Some(0.05);
        c.update_run(&r).unwrap();
        let loaded = c.load_run("t1-r1").unwrap().unwrap();
        assert_eq!(loaded.steps.len(), 3);
        assert_eq!(loaded.cost_usd, r.cost_usd);
        assert_eq!(loaded.steps, r.steps);
        assert_eq!(loaded.last_event_sequence, 3);
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
        c.insert_run(&run("r-done", "t1", RunStatus::Completed))
            .unwrap();

        assert_eq!(c.fail_running_runs("Perch quit").unwrap(), 1);
        let failed = c.load_run("r-running").unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Interrupted);
        assert_eq!(failed.error.as_deref(), Some("Perch quit"));
        assert!(failed.ended_at.is_some());
        assert_eq!(
            c.load_run("r-done").unwrap().unwrap().status,
            RunStatus::Completed
        );
    }

    fn event(id: &str, at: &str, event: RunEvent) -> NewRunEvent {
        NewRunEvent {
            event_id: id.into(),
            occurred_at: stamp(at),
            event,
            truncated: false,
        }
    }

    #[test]
    fn started_after_stop_requested_preserves_stopping_and_metadata() {
        let c = Cache::open_in_memory().unwrap();
        let mut queued = run("stop-during-start", "thread", RunStatus::Queued);
        queued.steps.clear();
        queued.started_at = None;
        queued.ended_at = None;
        queued.terminal_reason = None;
        c.insert_run(&queued).unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "starting",
                "2026-09-18T10:00:01Z",
                RunEvent::State {
                    status: RunStatus::Starting,
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "stopping",
                "2026-09-18T10:00:02Z",
                RunEvent::State {
                    status: RunStatus::Stopping,
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "late-started",
                "2026-09-18T10:00:03Z",
                RunEvent::Started {
                    session_id: "session-late".into(),
                    model: Some("model-actual".into()),
                },
            ),
        )
        .unwrap();

        let stopping = c.load_run(&queued.id).unwrap().unwrap();
        assert_eq!(stopping.status, RunStatus::Stopping);
        assert_eq!(stopping.session_id.as_deref(), Some("session-late"));
        assert_eq!(stopping.model, "model-actual");
        assert_eq!(stopping.latest_activity.as_deref(), Some("Started"));

        c.append_run_event(
            &queued.id,
            &event(
                "stopped",
                "2026-09-18T10:00:04Z",
                RunEvent::Terminated {
                    status: RunStatus::Stopped,
                    reason: TerminalReason::UserStopped,
                    message: "Stopped by the reader".into(),
                },
            ),
        )
        .unwrap();
        let stopped = c.load_run(&queued.id).unwrap().unwrap();
        assert_eq!(stopped.status, RunStatus::Stopped);
        assert_eq!(stopped.terminal_reason, Some(TerminalReason::UserStopped));
        assert_eq!(stopped.session_id.as_deref(), Some("session-late"));
    }

    #[test]
    fn completed_reply_insert_and_request_detach_are_atomic() {
        let c = Cache::open_in_memory().unwrap();
        let mut thread = comment_thread("reply", "w");
        thread.messages.truncate(1);
        c.insert_comment_thread(&thread).unwrap();
        let completed = run("reply-run", "reply", RunStatus::Completed);
        c.insert_run(&completed).unwrap();
        c.set_message_run("reply-m1", Some(&completed.id)).unwrap();
        let reply = CommentMessage {
            id: "reply-m2".into(),
            thread_id: "reply".into(),
            origin: ThreadOrigin::Local,
            author: Author::Agent {
                backend: completed.backend.clone(),
                model: completed.model.clone(),
            },
            body: "because".into(),
            created_at: stamp("2026-09-18T10:00:14Z"),
            run: Some(completed.clone()),
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        };

        c.insert_completed_reply("reply-m1", &reply).unwrap();

        let loaded = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert!(loaded.messages[0].run.is_none());
        assert_eq!(
            loaded.messages[1].run.as_ref().map(|run| run.id.as_str()),
            Some("reply-run")
        );
    }

    #[test]
    fn completed_reply_rolls_back_when_the_request_link_is_missing() {
        let c = Cache::open_in_memory().unwrap();
        let mut thread = comment_thread("rollback", "w");
        thread.messages.clear();
        c.insert_comment_thread(&thread).unwrap();
        let completed = run("rollback-run", "rollback", RunStatus::Completed);
        c.insert_run(&completed).unwrap();
        let reply = CommentMessage {
            id: "rollback-m2".into(),
            thread_id: "rollback".into(),
            origin: ThreadOrigin::Local,
            author: Author::Agent {
                backend: completed.backend.clone(),
                model: completed.model.clone(),
            },
            body: "because".into(),
            created_at: stamp("2026-09-18T10:00:14Z"),
            run: Some(completed),
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        };

        assert!(matches!(
            c.insert_completed_reply("rollback-m1", &reply),
            Err(CacheError::Corrupt(_))
        ));
        assert!(c.load_comment_threads("w").unwrap()[0].messages.is_empty());
    }

    #[test]
    fn admitting_a_run_and_its_first_event_is_atomic() {
        let c = Cache::open_in_memory().unwrap();
        let mut queued = run("atomic-admission", "thread", RunStatus::Queued);
        queued.steps.clear();
        queued.started_at = None;
        queued.ended_at = None;
        queued.terminal_reason = None;
        let invalid = event(
            "completed-too-soon",
            "2026-09-18T10:00:01Z",
            RunEvent::Completed {
                session_id: None,
                result: String::new(),
                cost_usd: None,
                duration_ms: None,
                usage: None,
            },
        );

        assert!(matches!(
            c.insert_run_with_event(&queued, &invalid),
            Err(CacheError::InvalidRunTransition {
                from: RunStatus::Queued,
                to: RunStatus::Completed
            })
        ));
        assert!(c.load_run(&queued.id).unwrap().is_none());
    }

    #[test]
    fn discussion_admission_links_the_request_in_the_run_transaction() {
        let c = Cache::open_in_memory().unwrap();
        let mut thread = comment_thread("admission", "w");
        thread.messages.truncate(1);
        c.insert_comment_thread(&thread).unwrap();
        let mut queued = run("admission-run", "admission", RunStatus::Queued);
        queued.steps.clear();
        queued.started_at = None;
        queued.ended_at = None;
        queued.terminal_reason = None;
        let queued_event = event(
            "state-queued",
            "2026-09-18T10:00:00Z",
            RunEvent::State {
                status: RunStatus::Queued,
            },
        );

        c.insert_run_with_event(&queued, &queued_event).unwrap();

        let loaded = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(
            loaded.messages[0].run.as_ref().map(|run| run.id.as_str()),
            Some("admission-run")
        );
    }

    #[test]
    fn discussion_admission_rolls_back_when_its_request_is_missing() {
        let c = Cache::open_in_memory().unwrap();
        let mut queued = run("detached-run", "missing", RunStatus::Queued);
        queued.steps.clear();
        queued.started_at = None;
        queued.ended_at = None;
        queued.terminal_reason = None;
        let queued_event = event(
            "state-queued",
            "2026-09-18T10:00:00Z",
            RunEvent::State {
                status: RunStatus::Queued,
            },
        );

        assert!(matches!(
            c.insert_run_with_event(&queued, &queued_event),
            Err(CacheError::Corrupt(_))
        ));
        assert!(c.load_run("detached-run").unwrap().is_none());
        assert!(c
            .load_run_events("detached-run", None, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn retry_admission_preserves_the_prior_attempt_beside_its_request() {
        let c = Cache::open_in_memory().unwrap();
        let mut thread = comment_thread("retry", "w");
        thread.messages.truncate(1);
        c.insert_comment_thread(&thread).unwrap();

        let previous = run("retry-old", "retry", RunStatus::Failed);
        c.insert_run(&previous).unwrap();
        c.set_message_run("retry-m1", Some(&previous.id)).unwrap();
        c.insert_comment_message(&CommentMessage {
            id: "retry-later".into(),
            thread_id: "retry".into(),
            origin: ThreadOrigin::Local,
            author: Author::User,
            body: "a later question".into(),
            created_at: stamp("2026-09-18T11:00:00Z"),
            run: None,
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        })
        .unwrap();

        let mut retry = run("retry-new", "retry", RunStatus::Queued);
        retry.steps.clear();
        retry.started_at = None;
        retry.ended_at = None;
        retry.terminal_reason = None;
        retry.retry_of = Some(previous.id.clone());
        retry.queued_at = stamp("2026-09-18T12:00:00Z");
        c.insert_run_with_event(
            &retry,
            &event(
                "state-queued",
                "2026-09-18T12:00:00Z",
                RunEvent::State {
                    status: RunStatus::Queued,
                },
            ),
        )
        .unwrap();

        let loaded = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.messages[0].id, "retry-m1");
        assert_eq!(
            loaded.messages[0].run.as_ref().map(|run| run.id.as_str()),
            Some("retry-new")
        );
        assert_eq!(
            loaded.messages[1].run.as_ref().map(|run| run.id.as_str()),
            Some("retry-old")
        );
        assert_eq!(loaded.messages[2].id, "retry-later");
    }

    #[test]
    fn streamed_text_survives_reload_and_completion_is_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let c = Cache::open(&path, None).unwrap();
        let mut queued = run("streamed", "thread", RunStatus::Queued);
        queued.steps.clear();
        queued.started_at = None;
        queued.ended_at = None;
        queued.terminal_reason = None;
        queued.final_result = None;
        c.insert_run(&queued).unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "starting",
                "2026-09-18T10:00:01Z",
                RunEvent::State {
                    status: RunStatus::Starting,
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "started",
                "2026-09-18T10:00:02Z",
                RunEvent::Started {
                    session_id: "session".into(),
                    model: None,
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "text-1",
                "2026-09-18T10:00:03Z",
                RunEvent::Text {
                    text: "first paragraph".into(),
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "text-2",
                "2026-09-18T10:00:04Z",
                RunEvent::Text {
                    text: "second paragraph".into(),
                },
            ),
        )
        .unwrap();
        drop(c);

        let c = Cache::open(&path, None).unwrap();
        assert_eq!(
            c.load_run(&queued.id)
                .unwrap()
                .unwrap()
                .final_result
                .as_deref(),
            Some("first paragraph\n\nsecond paragraph")
        );
        c.append_run_event(
            &queued.id,
            &event(
                "completed",
                "2026-09-18T10:00:05Z",
                RunEvent::Completed {
                    session_id: Some("session".into()),
                    result: "authoritative final answer".into(),
                    cost_usd: None,
                    duration_ms: Some(3_000),
                    usage: None,
                },
            ),
        )
        .unwrap();
        assert_eq!(
            c.load_run(&queued.id)
                .unwrap()
                .unwrap()
                .final_result
                .as_deref(),
            Some("authoritative final answer")
        );
    }

    #[test]
    fn events_append_idempotently_and_update_the_run_projection() {
        let c = Cache::open_in_memory().unwrap();
        let mut queued = run("run-events", "thread-events", RunStatus::Queued);
        queued.steps.clear();
        queued.started_at = None;
        queued.ended_at = None;
        queued.terminal_reason = None;
        queued.duration_ms = None;
        c.insert_run(&queued).unwrap();

        c.append_run_event(
            &queued.id,
            &event(
                "state-starting",
                "2026-09-18T10:00:01Z",
                RunEvent::State {
                    status: RunStatus::Starting,
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "started",
                "2026-09-18T10:00:02Z",
                RunEvent::Started {
                    session_id: "session-new".into(),
                    model: Some("sonnet-actual".into()),
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "tool",
                "2026-09-18T10:00:03Z",
                RunEvent::ToolCall {
                    id: "tool-1".into(),
                    name: "Read".into(),
                    summary: "Read src/lib.rs".into(),
                    parent_id: None,
                },
            ),
        )
        .unwrap();
        c.append_run_event(
            &queued.id,
            &event(
                "tool-result",
                "2026-09-18T10:00:04Z",
                RunEvent::ToolResult {
                    id: "tool-1".into(),
                    ok: false,
                    summary: "permission denied".into(),
                },
            ),
        )
        .unwrap();
        let completed = event(
            "done",
            "2026-09-18T10:00:05Z",
            RunEvent::Completed {
                session_id: Some("session-new".into()),
                result: "Answer".into(),
                cost_usd: Some(0.25),
                duration_ms: Some(3_000),
                usage: None,
            },
        );
        assert!(matches!(
            c.append_run_event(&queued.id, &completed).unwrap(),
            RunEventAppend::Appended(_)
        ));
        assert!(matches!(
            c.append_run_event(&queued.id, &completed).unwrap(),
            RunEventAppend::Duplicate(_)
        ));
        assert!(matches!(
            c.append_run_event(
                &queued.id,
                &event(
                    "late",
                    "2026-09-18T10:00:06Z",
                    RunEvent::Text {
                        text: "late".into()
                    }
                )
            ),
            Err(CacheError::TerminalRun(_))
        ));

        let loaded = c.load_run(&queued.id).unwrap().unwrap();
        assert_eq!(loaded.status, RunStatus::Completed);
        assert_eq!(loaded.model, "sonnet-actual");
        assert_eq!(loaded.session_id.as_deref(), Some("session-new"));
        assert_eq!(loaded.final_result.as_deref(), Some("Answer"));
        assert_eq!(loaded.cost_usd, Some(0.25));
        assert_eq!(loaded.steps.len(), 1);
        assert!(!loaded.steps[0].ok);
        assert!(loaded.steps[0].summary.contains("permission denied"));
        let events = c.load_run_events(&queued.id, None, 3).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        let older = c.load_run_events(&queued.id, Some(3), 10).unwrap();
        assert_eq!(
            older.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn active_history_request_queries_and_interruption_are_stable() {
        let c = Cache::open_in_memory().unwrap();
        for (index, status) in [
            RunStatus::Queued,
            RunStatus::Starting,
            RunStatus::Running,
            RunStatus::Stopping,
        ]
        .into_iter()
        .enumerate()
        {
            let mut active = run(
                &format!("active-{index}"),
                &format!("thread-{index}"),
                status,
            );
            active.steps.clear();
            active.terminal_reason = None;
            active.ended_at = None;
            active.owner = RunOwner::Discussion {
                item_id: "item-active".into(),
                thread_id: format!("thread-{index}"),
                request_id: format!("request-{index}"),
            };
            c.insert_run(&active).unwrap();
        }
        let done = run("done-history", "thread-done", RunStatus::Completed);
        c.insert_run(&done).unwrap();
        assert_eq!(c.active_runs().unwrap().len(), 4);
        assert_eq!(
            c.interrupt_active_runs(TerminalReason::AppRestart, "Perch restarted")
                .unwrap(),
            4
        );
        assert!(c.active_runs().unwrap().is_empty());
        for index in 0..4 {
            let run = c.load_run(&format!("active-{index}")).unwrap().unwrap();
            assert_eq!(run.status, RunStatus::Interrupted);
            assert_eq!(run.terminal_reason, Some(TerminalReason::AppRestart));
            assert_eq!(
                c.load_run_events(&run.id, None, 20)
                    .unwrap()
                    .last()
                    .unwrap()
                    .event_id,
                "interrupt-1"
            );
        }
        let history = c
            .run_history(&RunHistoryFilter::default(), None, 3)
            .unwrap();
        assert_eq!(history.len(), 3);
        let cursor = RunHistoryCursor {
            ended_at: history.last().unwrap().ended_at.unwrap(),
            id: history.last().unwrap().id.clone(),
        };
        assert!(!c
            .run_history(&RunHistoryFilter::default(), Some(&cursor), 10)
            .unwrap()
            .is_empty());
        assert_eq!(c.runs_for_item("item-active").unwrap().len(), 4);
        assert_eq!(
            c.runs_for_request("thread-2", "request-2").unwrap().len(),
            1
        );
    }

    #[test]
    fn summary_queries_and_event_appends_do_not_read_tool_transcripts() {
        let c = Cache::open_in_memory().unwrap();
        let mut active = run("summary-active", "summary-thread", RunStatus::Running);
        active.steps.clear();
        active.terminal_reason = None;
        active.ended_at = None;
        c.insert_run(&active).unwrap();
        c.conn
            .execute(
                "INSERT INTO run_events
                 (run_id, sequence, event_id, occurred_at, kind, payload_json, truncated)
                 VALUES (?1, 1, 'broken-tool', ?2, 'tool_call', '{broken', 0)",
                params![active.id, active.queued_at.to_string()],
            )
            .unwrap();
        c.conn
            .execute(
                "UPDATE runs SET last_event_sequence=1 WHERE id=?1",
                params![active.id],
            )
            .unwrap();

        let summaries = c.active_run_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, active.id);
        assert!(c.load_run(&active.id).is_err());
        c.append_run_event(
            &active.id,
            &event(
                "text-after-broken-tool",
                "2026-09-18T10:00:01Z",
                RunEvent::Text {
                    text: "still persisted".into(),
                },
            ),
        )
        .unwrap();

        let terminal = RunRecord {
            steps: Vec::new(),
            ..run("summary-terminal", "summary-done", RunStatus::Completed)
        };
        c.insert_run(&terminal).unwrap();
        c.conn
            .execute(
                "INSERT INTO run_events
                 (run_id, sequence, event_id, occurred_at, kind, payload_json, truncated)
                 VALUES (?1, 1, 'broken-history-tool', ?2, 'tool_call', '{broken', 0)",
                params![terminal.id, terminal.queued_at.to_string()],
            )
            .unwrap();
        let page = c
            .run_history_page(&RunHistoryFilter::default(), None, 10)
            .unwrap();
        assert!(page.runs.iter().any(|run| run.id == terminal.id));
    }

    #[test]
    fn history_cursor_walks_past_the_per_page_cap() {
        let c = Cache::open_in_memory().unwrap();
        let tx = c.conn.unchecked_transaction().unwrap();
        for index in 0..620 {
            let mut record = run(
                &format!("history-{index:04}"),
                &format!("history-thread-{index:04}"),
                RunStatus::Completed,
            );
            record.steps.clear();
            insert_run_row(&tx, &record).unwrap();
        }
        tx.commit().unwrap();

        let first = c
            .run_history_page(&RunHistoryFilter::default(), None, 500)
            .unwrap();
        assert_eq!(first.runs.len(), 500);
        let second = c
            .run_history_page(
                &RunHistoryFilter::default(),
                first.next_cursor.as_ref(),
                500,
            )
            .unwrap();
        assert_eq!(second.runs.len(), 120);
        assert!(second.next_cursor.is_none());
        let ids = first
            .runs
            .iter()
            .chain(&second.runs)
            .map(|run| run.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 620);
    }

    #[test]
    fn partial_result_is_bounded_while_all_text_events_remain_pageable() {
        let c = Cache::open_in_memory().unwrap();
        let mut running = run("bounded-stream", "bounded-thread", RunStatus::Running);
        running.steps.clear();
        running.terminal_reason = None;
        running.ended_at = None;
        running.final_result = None;
        c.insert_run(&running).unwrap();

        for index in 0..520 {
            c.append_run_event(
                &running.id,
                &event(
                    &format!("text-{index}"),
                    "2026-09-18T10:00:01Z",
                    RunEvent::Text {
                        text: format!("{index:04} {}", "x".repeat(192)),
                    },
                ),
            )
            .unwrap();
        }
        let projected = c.load_run(&running.id).unwrap().unwrap();
        assert!(projected.final_result_truncated);
        assert!(projected.final_result.unwrap().len() <= MAX_RUN_RESULT_BYTES);

        let mut before = None;
        let mut sequences = Vec::new();
        loop {
            let page = c.load_run_events(&running.id, before, 73).unwrap();
            if page.is_empty() {
                break;
            }
            before = page.first().map(|event| event.sequence);
            sequences.extend(page.into_iter().map(|event| event.sequence));
        }
        sequences.sort_unstable();
        sequences.dedup();
        assert_eq!(sequences.len(), 520);
        assert_eq!(sequences.first(), Some(&1));
        assert_eq!(sequences.last(), Some(&520));

        c.append_run_event(
            &running.id,
            &event(
                "completed-large",
                "2026-09-18T10:00:02Z",
                RunEvent::Completed {
                    session_id: None,
                    result: "z".repeat(MAX_RUN_RESULT_BYTES * 2),
                    cost_usd: None,
                    duration_ms: None,
                    usage: None,
                },
            ),
        )
        .unwrap();
        let completed = c.load_run(&running.id).unwrap().unwrap();
        assert!(completed.final_result_truncated);
        assert!(completed.final_result.unwrap().len() <= MAX_RUN_RESULT_BYTES);
    }

    #[test]
    fn only_one_nonterminal_attempt_per_request_can_be_inserted() {
        let c = Cache::open_in_memory().unwrap();
        let mut first = run("attempt-1", "thread", RunStatus::Queued);
        first.steps.clear();
        first.started_at = None;
        first.ended_at = None;
        first.terminal_reason = None;
        c.insert_run(&first).unwrap();
        let mut second = first.clone();
        second.id = "attempt-2".into();
        second.retry_of = Some(first.id);
        assert!(matches!(c.insert_run(&second), Err(CacheError::Sqlite(_))));
    }

    #[test]
    fn runs_for_discussion_returns_all_requests_in_attempt_order() {
        let c = Cache::open_in_memory().unwrap();
        let first = run("discussion-1", "shared-thread", RunStatus::Completed);
        let mut second = run("discussion-2", "shared-thread", RunStatus::Failed);
        second.owner = RunOwner::Discussion {
            item_id: "w".into(),
            thread_id: "shared-thread".into(),
            request_id: "shared-thread-m2".into(),
        };
        let other = run("other", "other-thread", RunStatus::Completed);
        c.insert_run(&first).unwrap();
        c.insert_run(&second).unwrap();
        c.insert_run(&other).unwrap();

        let ids: Vec<_> = c
            .runs_for_discussion("shared-thread")
            .unwrap()
            .into_iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(ids, ["discussion-1", "discussion-2"]);
    }

    #[test]
    fn event_payloads_are_bounded_after_json_escaping() {
        let c = Cache::open_in_memory().unwrap();
        let mut running = run("long-event", "long-thread", RunStatus::Running);
        running.steps.clear();
        running.ended_at = None;
        running.terminal_reason = None;
        c.insert_run(&running).unwrap();
        let long = "\\\n".repeat(100_000);
        let appended = c
            .append_run_event(
                &running.id,
                &event(
                    "long",
                    "2026-09-18T10:00:01Z",
                    RunEvent::Text { text: long },
                ),
            )
            .unwrap();
        let RunEventAppend::Appended(stored) = appended else {
            panic!("first append was not new")
        };
        assert!(stored.truncated);
        let bytes: i64 = c
            .conn
            .query_row(
                "SELECT length(CAST(payload_json AS BLOB)) FROM run_events WHERE run_id=?1 AND event_id='long'",
                params![running.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            bytes as usize <= MAX_RUN_EVENT_BYTES,
            "stored {bytes} bytes"
        );
    }

    #[test]
    fn deleting_an_item_takes_its_threads_and_runs() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let id = pr_item(7).id;
        c.insert_comment_thread(&comment_thread("t1", &id)).unwrap();
        c.insert_comment_thread(&comment_thread("t2", "elsewhere"))
            .unwrap();

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

    fn stamp_of(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn a_fresh_database_carries_the_schema_version() {
        let c = Cache::open_in_memory().unwrap();
        assert_eq!(
            c.meta("schema_version").unwrap().as_deref(),
            Some(&*SCHEMA_VERSION.to_string())
        );
    }

    fn write_schema_13(path: &Path, status: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(BASE_SCHEMA).unwrap();
        conn.execute("DROP TABLE threads", []).unwrap();
        conn.execute_batch(
            "CREATE TABLE runs (
                id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, backend TEXT NOT NULL,
                model TEXT NOT NULL, effort TEXT NOT NULL, session_id TEXT, status TEXT NOT NULL,
                cost_usd REAL, duration_ms INTEGER, steps_json TEXT NOT NULL DEFAULT '[]',
                error TEXT, started_at TEXT NOT NULL, ended_at TEXT
             );
             CREATE TABLE comment_messages (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL REFERENCES comment_threads(id) ON DELETE CASCADE,
                origin TEXT NOT NULL, github_id TEXT, author_kind TEXT NOT NULL, author TEXT,
                author_model TEXT, body TEXT NOT NULL, run_id TEXT REFERENCES runs(id),
                created_at TEXT NOT NULL
             );
             CREATE INDEX comment_messages_thread ON comment_messages(thread_id);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES ('schema_version','13')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO items
             (id,repo_full_name,branch,base_ref,head_sha,checkout_path,created_at,last_opened_at)
             VALUES ('item-1','acme/widgets','feature','main','abc','/tmp/feature',?1,?1)",
            params!["2026-09-18T09:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO comment_threads
             (id,worktree_id,origin,outdated,resolved,created_at)
             VALUES ('thread-1','item-1','local',0,0,?1)",
            params!["2026-09-18T09:30:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO runs
             (id,thread_id,backend,model,effort,session_id,status,cost_usd,duration_ms,
              steps_json,error,started_at,ended_at)
             VALUES ('run-1','thread-1','claude-code','sonnet','high','session-1',?1,0.12,4200,
                     '[{\"summary\":\"Read src/lib.rs\",\"ok\":true}]',NULL,?2,?3)",
            params![status, "2026-09-18T10:00:00Z", "2026-09-18T10:00:04Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO runs
             (id,thread_id,backend,model,effort,status,steps_json,error,started_at,ended_at)
             VALUES ('run-orphan','missing','codex','gpt','medium','failed','[]','gone',?1,?1)",
            params!["2026-09-18T11:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO comment_messages
             (id,thread_id,origin,author_kind,body,run_id,created_at)
             VALUES ('request-1','thread-1','local','user','Why?',NULL,?1),
                    ('answer-1','thread-1','local','agent','Because.','run-1',?2)",
            params!["2026-09-18T09:59:00Z", "2026-09-18T10:00:04Z"],
        )
        .unwrap();
    }

    #[test]
    fn schema_13_migrates_runs_messages_and_steps_without_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        write_schema_13(&path, "finished");

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 13,
                to: SCHEMA_VERSION,
            }
        );
        let run = cache.load_run("run-1").unwrap().unwrap();
        assert_eq!(
            run.owner,
            RunOwner::Discussion {
                item_id: "item-1".into(),
                thread_id: "thread-1".into(),
                request_id: "request-1".into(),
            }
        );
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.session_id.as_deref(), Some("session-1"));
        assert_eq!(run.original_request, "Why?");
        assert_eq!(run.final_result.as_deref(), Some("Because."));
        assert_eq!(run.labels.repository, "acme/widgets");
        assert_eq!(run.labels.branch.as_deref(), Some("feature"));
        assert_eq!(
            run.steps,
            vec![RunStep {
                summary: "Read src/lib.rs".into(),
                ok: true
            }]
        );
        assert_eq!(cache.load_run_events("run-1", None, 20).unwrap().len(), 1);
        assert!(matches!(
            cache.load_run("run-orphan").unwrap().unwrap().owner,
            RunOwner::Unavailable { legacy_thread_id: Some(ref id) } if id == "missing"
        ));
        let threads = cache.load_comment_threads("item-1").unwrap();
        assert_eq!(threads[0].messages[1].run.as_ref().unwrap().id, "run-1");
        assert!(
            cache.load_threads().unwrap().is_empty(),
            "the notification cache is added after the run migration"
        );
    }

    #[test]
    fn a_failed_migration_rolls_back_every_schema_and_data_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        write_schema_13(&path, "not-a-status");

        assert!(matches!(
            Cache::open_with_outcome(&path, None),
            Err(CacheError::Corrupt(_))
        ));
        let conn = Connection::open(&path).unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "13");
        let kept: i64 = conn
            .query_row("SELECT count(*) FROM runs WHERE id='run-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kept, 1);
        let legacy_column: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('runs') WHERE name='thread_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_column, 1);
        let new_column: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('runs') WHERE name='owner_kind'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_column, 0);
    }

    #[test]
    fn a_database_with_no_version_is_retained_and_refused() {
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
        assert!(matches!(
            Cache::open_with_outcome(&path, None),
            Err(CacheError::MissingSchemaVersion)
        ));
        let conn = Connection::open(&path).unwrap();
        let old: i64 = conn
            .query_row("SELECT count(*) FROM threads WHERE id='old'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(old, 1, "unsupported data remains intact");
    }

    #[test]
    fn an_unsupported_old_version_is_retained_and_a_current_one_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let (c, reset) = Cache::open_checked(&path, None).unwrap();
            assert!(!reset, "a new file is not a reset");
            c.insert_item(&pr_item(7)).unwrap();
            c.set_meta("schema_version", "6").unwrap();
        }
        assert!(matches!(
            Cache::open_with_outcome(&path, None),
            Err(CacheError::UnsupportedSchema { found: 6 })
        ));
        let conn = Connection::open(&path).unwrap();
        let kept: i64 = conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1);
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='schema_version'",
            params![SCHEMA_VERSION.to_string()],
        )
        .unwrap();
        drop(conn);
        let (c, reset) = Cache::open_checked(&path, None).unwrap();
        assert!(!reset, "a current database is kept");
        assert_eq!(c.load_items().unwrap().len(), 1);
    }

    #[test]
    fn feature_schema_14_adds_notifications_without_losing_rich_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = pr_item(14);
        let thread = comment_thread("feature-14", &item.id);
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache.insert_comment_thread(&thread).unwrap();
            remove_review_schema(&cache);
            cache.conn.execute("DROP TABLE threads", []).unwrap();
            cache.set_meta("schema_version", "14").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 14,
                to: SCHEMA_VERSION,
            }
        );
        assert_eq!(cache.load_item(&item.id).unwrap(), Some(item));
        let loaded = cache.load_comment_threads(&thread.worktree_id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, thread.id);
        assert_eq!(
            loaded[0].messages[1].run.as_ref().unwrap().id,
            "feature-14-r1"
        );
        assert!(cache.load_threads().unwrap().is_empty());
    }

    #[test]
    fn main_schema_15_migrates_legacy_runs_and_keeps_notifications() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        write_schema_13(&path, "finished");
        let notification = notification_thread("15", "2026-09-19T12:00:00Z");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(NOTIFICATION_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO threads (position, id, updated_at, json) VALUES (0, ?1, ?2, ?3)",
                params![
                    notification.id,
                    notification.updated_at.to_string(),
                    serde_json::to_string(&notification).unwrap(),
                ],
            )
            .unwrap();
            conn.execute("UPDATE meta SET value='15' WHERE key='schema_version'", [])
                .unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 15,
                to: SCHEMA_VERSION,
            }
        );
        assert_eq!(cache.load_threads().unwrap(), vec![notification]);
        let run = cache.load_run("run-1").unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.original_request, "Why?");
        assert_eq!(run.final_result.as_deref(), Some("Because."));
    }

    #[test]
    fn schema_16_adds_retry_prompt_and_bounds_existing_result_projection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            let mut record = run("schema-16-run", "schema-16-thread", RunStatus::Completed);
            record.steps.clear();
            record.prompt_snapshot = None;
            cache.insert_run(&record).unwrap();
            remove_review_schema(&cache);
            cache
                .conn
                .execute_batch(
                    "ALTER TABLE runs DROP COLUMN prompt_snapshot;
                     ALTER TABLE runs DROP COLUMN final_result_truncated;",
                )
                .unwrap();
            cache
                .conn
                .execute(
                    "UPDATE runs SET final_result=?2 WHERE id=?1",
                    params![record.id, "z".repeat(MAX_RUN_RESULT_BYTES * 2)],
                )
                .unwrap();
            cache.set_meta("schema_version", "16").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 16,
                to: SCHEMA_VERSION,
            }
        );
        let record = cache.load_run("schema-16-run").unwrap().unwrap();
        assert_eq!(record.prompt_snapshot.as_deref(), Some("why?"));
        assert!(record.final_result_truncated);
        assert!(record.final_result.unwrap().len() <= MAX_RUN_RESULT_BYTES);
    }

    #[test]
    fn schema_19_adds_notification_dismissals_without_losing_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = pr_item(19);
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache
                .conn
                .execute("DROP TABLE notification_dismissals", [])
                .unwrap();
            cache.set_meta("schema_version", "19").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 19,
                to: SCHEMA_VERSION,
            }
        );
        assert_eq!(cache.load_item(&item.id).unwrap(), Some(item));
        cache
            .dismiss_notification("19", stamp("2026-09-19T12:00:00Z"))
            .unwrap();
    }

    #[test]
    fn a_newer_schema_is_retained_and_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&pr_item(9)).unwrap();
            cache.set_meta("schema_version", "99").unwrap();
        }
        assert!(matches!(
            Cache::open_with_outcome(&path, None),
            Err(CacheError::NewerSchema {
                found: 99,
                current: SCHEMA_VERSION
            })
        ));
        let conn = Connection::open(&path).unwrap();
        let kept: i64 = conn
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 1);
    }

    #[test]
    fn the_schema_has_no_project_table_and_indexes_items_by_repository() {
        let c = Cache::open_in_memory().unwrap();
        let count = |sql: &str| -> i64 { c.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT count(*) FROM sqlite_master WHERE name = 'projects'"),
            0
        );
        assert_eq!(count("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'items_project'"), 0);
        assert_eq!(
            count(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'items_repo'"
            ),
            1
        );
        assert_eq!(
            count("SELECT count(*) FROM pragma_table_info('items') WHERE name = 'project_id'"),
            0
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
        c.insert_item(&i).unwrap();
        assert_eq!(c.load_items().unwrap(), vec![i.clone()]);
        assert_eq!(c.load_item(&i.id).unwrap(), Some(i.clone()));
        assert_eq!(
            c.find_item_by_subject("ACME/widgets", ItemKind::PullRequest, 7)
                .unwrap(),
            Some(i.clone())
        );
        assert_eq!(
            c.find_item_by_branch("acme/widgets", "perch/pr-7").unwrap(),
            Some(i.clone())
        );
        assert_eq!(
            c.find_item_by_subject("acme/widgets", ItemKind::Issue, 7)
                .unwrap(),
            None
        );

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
        assert_eq!(
            c.find_item_by_subject("acme/widgets", ItemKind::PullRequest, 7)
                .unwrap()
                .map(|i| i.id),
            Some(i.id.clone())
        );
        assert_eq!(
            c.find_item_by_subject("chris/widgets", ItemKind::PullRequest, 7)
                .unwrap(),
            None
        );
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
    fn saved_and_deletion() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut cut = branch_item("perch/pr-7-2");
        cut.from_item_id = Some(pr_item(7).id);
        c.insert_item(&cut).unwrap();
        // Saved is a column of the row, written the way every other edit is.
        let mut marked = pr_item(7);
        marked.saved = true;
        c.update_item(&marked).unwrap();
        let i = c.load_item(&pr_item(7).id).unwrap().unwrap();
        assert!(i.saved);

        c.delete_item(&pr_item(7).id).unwrap();
        assert!(c.load_item(&pr_item(7).id).unwrap().is_none());
        let cut = c.load_item(&cut.id).unwrap().unwrap();
        assert_eq!(
            cut.from_item_id, None,
            "the pointer is cleared, the row stays"
        );
    }

    #[test]
    fn the_last_seen_head_is_a_column_of_its_own() {
        let c = Cache::open_in_memory().unwrap();
        let mut i = branch_item("feature");
        c.insert_item(&i).unwrap();
        assert_eq!(
            c.load_item(&i.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .last_seen_head,
            None
        );

        i.checkout.as_mut().unwrap().last_seen_head = Some("abc".into());
        i.checkout.as_mut().unwrap().head_sha = "def".into();
        c.update_item(&i).unwrap();
        let checkout = c.load_item(&i.id).unwrap().unwrap().checkout.unwrap();
        assert_eq!(checkout.last_seen_head.as_deref(), Some("abc"));
        assert_eq!(checkout.head_sha, "def");
    }

    fn discussion() -> Discussion {
        use crate::discussion::{IssueComment, PullEvent, PullEventKind, Review};
        Discussion {
            node_id: "PR_7".into(),
            opened_at: Some(stamp_of("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            reactions: Default::default(),
            comments: vec![IssueComment {
                id: 1,
                node_id: "IC_1".into(),
                user: "chris".into(),
                user_avatar_url: "https://avatars/1".into(),
                body: "q".into(),
                created_at: stamp_of("2026-09-15T09:14:00Z"),
                updated_at: None,
                html_url: "https://github.com/acme/widgets/pull/7#issuecomment-1".into(),
                reactions: Default::default(),
            }],
            reviews: vec![Review {
                id: 9,
                node_id: "PRR_9".into(),
                user: "rohan".into(),
                body: "nice".into(),
                state: "APPROVED".into(),
                submitted_at: Some(stamp_of("2026-09-16T11:40:00Z")),
                html_url: String::new(),
                reactions: Default::default(),
            }],
            review_comments: vec![review_comment(3, None, "2026-09-16T11:38:00Z")],
            events: vec![
                PullEvent {
                    kind: PullEventKind::Opened,
                    at: stamp_of("2026-09-14T14:02:00Z"),
                },
                PullEvent {
                    kind: PullEventKind::Merged {
                        by: Some("rohan".into()),
                    },
                    at: stamp_of("2026-09-17T15:00:00Z"),
                },
            ],
            review_threads: Vec::new(),
        }
    }

    fn review_comment(id: u64, reply_to: Option<u64>, at: &str) -> crate::discussion::ReviewComment {
        let mut thumbs = Reactions::default();
        thumbs.toggle(crate::reactions::ReactionKind::ThumbsUp, true);
        crate::discussion::ReviewComment {
            id,
            node_id: format!("PRRC_{id}"),
            pull_request_review_id: Some(9),
            in_reply_to_id: reply_to,
            user: "rohan".into(),
            user_avatar_url: "https://avatars/2".into(),
            body: format!("epoch {id}"),
            path: "src/manifest.rs".into(),
            line: Some(41),
            original_line: None,
            start_line: None,
            original_start_line: None,
            diff_hunk: "@@ -40,3 +40,3 @@".into(),
            side: Side::Right,
            start_side: None,
            commit_id: String::new(),
            original_commit_id: String::new(),
            created_at: stamp_of(at),
            updated_at: None,
            html_url: format!("https://github.com/acme/widgets/pull/7#discussion_r{id}"),
            reactions: thumbs,
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
        assert_eq!(
            c.load_discussion("item-pr-8").unwrap(),
            None,
            "keyed by item"
        );

        // A second fetch replaces the row rather than adding to it.
        let mut newer = discussion();
        newer.comments.clear();
        c.store_discussion("item-pr-7", &newer).unwrap();
        assert_eq!(c.load_discussion("item-pr-7").unwrap().unwrap().1, newer);
    }

    #[test]
    fn storing_a_discussion_keeps_its_review_threads_as_comment_threads() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut d = discussion();
        d.review_comments.push(review_comment(4, Some(3), "2026-09-16T11:39:00Z"));
        c.store_discussion("item-pr-7", &d).unwrap();

        let threads = c.load_comment_threads("item-pr-7").unwrap();
        assert_eq!(threads.len(), 1);
        let t = &threads[0];
        assert_eq!((t.id.as_str(), t.origin, t.github_id), ("gh-3", ThreadOrigin::GitHub, Some(3)));
        assert_eq!(
            t.anchor.as_ref().map(|a| a.commit_sha.as_str()),
            Some("abc123"),
            "a comment without a commit anchors at the item's head"
        );
        assert_eq!(t.created_at, stamp_of("2026-09-16T11:38:00Z"));
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3", "gh-4"]);
        let root = &t.messages[0];
        assert_eq!(root.author, Author::GitHubUser("rohan".into()));
        assert_eq!(root.github_id, Some(3));
        assert_eq!(root.node_id.as_deref(), Some("PRRC_3"));
        assert_eq!(root.html_url.as_deref(), Some("https://github.com/acme/widgets/pull/7#discussion_r3"));
        assert_eq!(root.diff_hunk.as_deref(), Some("@@ -40,3 +40,3 @@"));
        assert_eq!(root.reactions, d.review_comments[0].reactions);

        // Storing the same discussion again changes nothing.
        c.store_discussion("item-pr-7", &d).unwrap();
        assert_eq!(c.load_comment_threads("item-pr-7").unwrap(), threads);

        // An edit on GitHub reaches the row; a reply gone from GitHub goes.
        d.review_comments[0].body = "epoch, revised".into();
        d.review_comments.pop();
        c.store_discussion("item-pr-7", &d).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.messages.len(), 1);
        assert_eq!(t.messages[0].body, "epoch, revised");
    }

    #[test]
    fn re_ingesting_keeps_what_perch_put_under_a_github_thread() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        let local = CommentMessage {
            id: "q1".into(),
            thread_id: "gh-3".into(),
            origin: ThreadOrigin::Local,
            author: Author::User,
            body: "what does this mean?".into(),
            created_at: stamp_of("2026-09-16T12:00:00Z"),
            run: None,
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        };
        let draft = CommentMessage {
            id: "d1".into(),
            origin: ThreadOrigin::GitHub,
            body: "I will fix it".into(),
            created_at: stamp_of("2026-09-16T12:01:00Z"),
            ..local.clone()
        };
        c.insert_comment_message(&local).unwrap();
        c.insert_comment_message(&draft).unwrap();

        c.store_discussion("item-pr-7", &discussion()).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3", "q1", "d1"]);
        assert!(t.messages[1].is_private());
        assert!(t.messages[2].is_draft());

        // The thread left GitHub. With Perch's messages under it, it stays.
        let mut gone = discussion();
        gone.review_comments.clear();
        c.store_discussion("item-pr-7", &gone).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.id, "gh-3");
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["q1", "d1"]);
        assert_eq!(
            (t.origin, t.github_id),
            (ThreadOrigin::GitHub, None),
            "nothing on GitHub to reply to any more"
        );

        // It comes back as a thread of its own, and is not deleted for
        // being empty of GitHub's comments.
        c.store_discussion("item-pr-7", &gone).unwrap();
        assert_eq!(c.load_comment_threads("item-pr-7").unwrap().len(), 1);
    }

    #[test]
    fn deleting_perchs_messages_under_a_github_thread_leaves_the_thread_and_githubs() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        let question = CommentMessage {
            id: "q1".into(),
            thread_id: "gh-3".into(),
            origin: ThreadOrigin::Local,
            author: Author::User,
            body: "what does this mean?".into(),
            created_at: stamp_of("2026-09-16T12:00:00Z"),
            run: None,
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        };
        let mut answer_run = run("gh-3-r1", "gh-3", RunStatus::Completed);
        answer_run.owner = RunOwner::Discussion {
            item_id: "item-pr-7".into(),
            thread_id: "gh-3".into(),
            request_id: "q1".into(),
        };
        let answer = CommentMessage {
            id: "a1".into(),
            author: Author::Agent { backend: "claude-code".into(), model: "sonnet".into() },
            body: "it means this".into(),
            created_at: stamp_of("2026-09-16T12:01:00Z"),
            run: Some(answer_run.clone()),
            ..question.clone()
        };
        let draft = CommentMessage {
            id: "d1".into(),
            origin: ThreadOrigin::GitHub,
            body: "I will fix it".into(),
            created_at: stamp_of("2026-09-16T12:02:00Z"),
            ..question.clone()
        };
        c.insert_comment_message(&question).unwrap();
        c.insert_run(&answer_run).unwrap();
        c.insert_comment_message(&answer).unwrap();
        c.insert_comment_message(&draft).unwrap();

        c.delete_private_messages("gh-3").unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3", "d1"], "GitHub's comment and the draft stay");
        assert_eq!(c.load_run("gh-3-r1").unwrap(), None, "the answer's run goes with it");

        // Nothing of Perch's under it is nothing to do.
        c.delete_private_messages("gh-3").unwrap();
        assert_eq!(c.load_comment_threads("item-pr-7").unwrap().len(), 1);
    }

    #[test]
    fn deleting_perchs_messages_under_a_thread_whose_root_left_github_deletes_the_thread() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        c.insert_comment_message(&CommentMessage {
            id: "q1".into(),
            thread_id: "gh-3".into(),
            origin: ThreadOrigin::Local,
            author: Author::User,
            body: "what does this mean?".into(),
            created_at: stamp_of("2026-09-16T12:00:00Z"),
            run: None,
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        })
        .unwrap();
        let mut gone = discussion();
        gone.review_comments.clear();
        c.store_discussion("item-pr-7", &gone).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!((t.id.as_str(), t.github_id), ("gh-3", None));

        c.delete_private_messages("gh-3").unwrap();
        assert!(
            c.load_comment_threads("item-pr-7").unwrap().is_empty(),
            "nothing was left under it"
        );
    }

    /// A draft of the reader's under `thread_id`, dated `at`.
    fn draft(id: &str, thread_id: &str, at: &str) -> CommentMessage {
        CommentMessage {
            id: id.into(),
            thread_id: thread_id.into(),
            origin: ThreadOrigin::GitHub,
            author: Author::User,
            body: "I will fix it".into(),
            created_at: stamp_of(at),
            run: None,
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        }
    }

    #[test]
    fn a_drafts_body_is_rewritten_and_an_answers_is_not() {
        let c = Cache::open_in_memory().unwrap();
        let t = comment_thread("t1", "w");
        c.insert_comment_thread(&t).unwrap();
        c.insert_draft(&draft("d1", "t1", "2026-09-18T10:01:00Z")).unwrap();

        c.update_comment_message_body("d1", "I fixed it").unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        let d = t.messages.iter().find(|m| m.id == "d1").unwrap();
        assert_eq!(d.body, "I fixed it");
        assert_eq!(d.created_at, stamp_of("2026-09-18T10:01:00Z"), "its time stays");

        assert!(matches!(
            c.update_comment_message_body("t1-m2", "no"),
            Err(CacheError::Corrupt(_))
        ));
        assert!(matches!(
            c.update_comment_message_body("nowhere", "no"),
            Err(CacheError::Corrupt(_))
        ));
    }

    #[test]
    fn a_thread_turns_towards_github_and_back() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        c.set_thread_origin("t1", ThreadOrigin::GitHub).unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(t.origin, ThreadOrigin::GitHub);
        assert_eq!(t.github_id, None, "not on GitHub yet");
        c.set_thread_origin("t1", ThreadOrigin::Local).unwrap();
        assert_eq!(c.load_comment_threads("w").unwrap()[0].origin, ThreadOrigin::Local);
        assert!(matches!(
            c.set_thread_origin("nowhere", ThreadOrigin::Local),
            Err(CacheError::Corrupt(_))
        ));
    }

    #[test]
    fn a_draft_under_perchs_thread_turns_it_towards_github() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        c.insert_draft(&draft("d1", "t1", "2026-09-18T10:01:00Z")).unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(t.origin, ThreadOrigin::GitHub);
        assert_eq!(t.github_id, None);
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["t1-m1", "t1-m2", "d1"]);

        // A message that is not a draft is not what this is for.
        let mut answer = draft("d2", "t1", "2026-09-18T10:02:00Z");
        answer.origin = ThreadOrigin::Local;
        assert!(matches!(c.insert_draft(&answer), Err(CacheError::Corrupt(_))));
        assert_eq!(c.load_comment_threads("w").unwrap()[0].messages.len(), 3);
    }

    #[test]
    fn deleting_a_draft_turns_its_thread_back_or_takes_it() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        c.insert_draft(&draft("d1", "t1", "2026-09-18T10:01:00Z")).unwrap();
        c.delete_draft("d1").unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(t.origin, ThreadOrigin::Local, "only Perch's messages are left");
        assert_eq!(t.messages.len(), 2);

        // A thread that was only made for its draft goes with it.
        let mut fresh = comment_thread("t2", "w");
        fresh.origin = ThreadOrigin::GitHub;
        fresh.messages = vec![draft("d2", "t2", "2026-09-18T10:03:00Z")];
        c.insert_comment_thread(&fresh).unwrap();
        assert_eq!(c.load_comment_threads("w").unwrap().len(), 2);
        c.delete_draft("d2").unwrap();
        let threads = c.load_comment_threads("w").unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "t1");

        // Not a draft: nothing happens.
        assert!(matches!(c.delete_draft("t1-m1"), Err(CacheError::Corrupt(_))));
        assert_eq!(c.load_comment_threads("w").unwrap()[0].messages.len(), 2);
    }

    #[test]
    fn deleting_a_draft_under_githubs_thread_leaves_the_thread_as_it_is() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        c.insert_draft(&draft("d1", "gh-3", "2026-09-16T12:02:00Z")).unwrap();
        c.delete_draft("d1").unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.origin, ThreadOrigin::GitHub);
        assert_eq!(t.github_id, Some(3));
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3"]);
    }

    #[test]
    fn a_thread_whose_root_left_github_stops_pointing_at_it() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut d = discussion();
        d.review_comments.push(review_comment(4, Some(3), "2026-09-16T11:39:00Z"));
        c.store_discussion("item-pr-7", &d).unwrap();
        c.insert_comment_message(&CommentMessage {
            id: "q1".into(),
            thread_id: "gh-3".into(),
            origin: ThreadOrigin::Local,
            author: Author::User,
            body: "what does this mean?".into(),
            created_at: stamp_of("2026-09-16T12:00:00Z"),
            run: None,
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Reactions::default(),
        })
        .unwrap();

        // The root is deleted on GitHub; the reply is now a root of its own.
        d.review_comments.remove(0);
        c.store_discussion("item-pr-7", &d).unwrap();
        let threads = c.load_comment_threads("item-pr-7").unwrap();
        let by_id = |id: &str| threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(by_id("gh-4").github_id, Some(4));
        assert_eq!(by_id("gh-3").github_id, None, "comment 3 is gone");
        let ids: Vec<_> = by_id("gh-3").messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["q1"]);
    }

    #[test]
    fn a_thread_gone_from_github_with_nothing_else_under_it_is_deleted() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        assert_eq!(c.load_comment_threads("item-pr-7").unwrap().len(), 1);
        let mut gone = discussion();
        gone.review_comments.clear();
        c.store_discussion("item-pr-7", &gone).unwrap();
        assert!(c.load_comment_threads("item-pr-7").unwrap().is_empty());
    }

    #[test]
    fn ingestion_is_scoped_to_its_item() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.insert_item(&pr_item(8)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        let mut other = discussion();
        other.review_comments = vec![review_comment(5, None, "2026-09-16T11:38:00Z")];
        c.store_discussion("item-pr-8", &other).unwrap();
        let mut gone = discussion();
        gone.review_comments.clear();
        c.store_discussion("item-pr-8", &gone).unwrap();
        assert_eq!(c.load_comment_threads("item-pr-7").unwrap().len(), 1);
        assert!(c.load_comment_threads("item-pr-8").unwrap().is_empty());
    }

    #[test]
    fn a_message_from_github_round_trips_its_columns() {
        let c = Cache::open_in_memory().unwrap();
        let mut t = comment_thread("t1", "w");
        t.origin = ThreadOrigin::GitHub;
        t.github_id = Some(77);
        let mut hooray = Reactions::default();
        hooray.toggle(crate::reactions::ReactionKind::Hooray, true);
        t.messages[0] = CommentMessage {
            origin: ThreadOrigin::GitHub,
            author: Author::GitHubUser("rohan".into()),
            github_id: Some(77),
            node_id: Some("PRRC_77".into()),
            html_url: Some("https://github.com/acme/widgets/pull/7#discussion_r77".into()),
            diff_hunk: Some("@@ -1 +1 @@".into()),
            reactions: hooray,
            ..t.messages[0].clone()
        };
        c.insert_comment_thread(&t).unwrap();
        let loaded = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(loaded.github_id, Some(77));
        assert_eq!(loaded.messages[0], t.messages[0]);
        assert_eq!(loaded.messages[1].github_id, None);
        assert!(loaded.messages[1].reactions.is_empty());
        let stored: Option<String> = c
            .conn
            .query_row(
                "SELECT reactions_json FROM comment_messages WHERE id = ?1",
                params![t.messages[1].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, None, "no reactions is no column value");
    }

    #[test]
    fn schema_20_gains_the_github_message_columns_and_the_threads_of_kept_discussions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = pr_item(20);
        let local = comment_thread("local-20", &item.id);
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache.insert_comment_thread(&local).unwrap();
            for column in GITHUB_MESSAGE_COLUMNS {
                cache
                    .conn
                    .execute(&format!("ALTER TABLE comment_messages DROP COLUMN {column}"), [])
                    .unwrap();
            }
            cache
                .conn
                .execute(
                    "INSERT INTO discussions (item_id, fetched_at, json) VALUES (?1, ?2, ?3)",
                    params![
                        item.id,
                        "2026-09-18T09:00:00Z",
                        serde_json::to_string(&discussion()).unwrap()
                    ],
                )
                .unwrap();
            cache
                .conn
                .execute(
                    "INSERT INTO discussions (item_id, fetched_at, json) VALUES ('broken', ?1, '{')",
                    params!["2026-09-18T09:00:00Z"],
                )
                .unwrap();
            cache.set_meta("schema_version", "20").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 20,
                to: SCHEMA_VERSION,
            }
        );
        let threads = cache.load_comment_threads(&item.id).unwrap();
        let ids: Vec<_> = threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3", "local-20"]);
        assert_eq!(threads[0].messages[0].node_id.as_deref(), Some("PRRC_3"));
        assert_eq!(
            threads[0].anchor.as_ref().map(|a| a.commit_sha.as_str()),
            Some("abc123")
        );
        assert_eq!(threads[1].messages.len(), 2);
        assert!(cache.load_comment_threads("broken").unwrap().is_empty());
    }

    #[test]
    fn checks_are_only_good_for_the_head_they_were_read_at() {
        use crate::workflow_runs::CheckState;
        let c = Cache::open_in_memory().unwrap();
        let checks = Checks {
            state: CheckState::Failing {
                workflow: "CI".into(),
            },
            url: Some("https://github.com/acme/widgets/actions/runs/1".into()),
            runs: Vec::new(),
        };
        c.store_checks("item-pr-7", "abc123", &checks).unwrap();
        assert_eq!(
            c.load_checks("item-pr-7", "abc123").unwrap(),
            Some(checks.clone())
        );
        assert_eq!(
            c.load_checks("item-pr-7", "fff999").unwrap(),
            None,
            "a push makes them a miss"
        );
        assert_eq!(c.load_checks("item-pr-8", "abc123").unwrap(), None);

        // Reading them at the new head replaces the row.
        let green = Checks {
            state: CheckState::Passing,
            url: None,
            runs: Vec::new(),
        };
        c.store_checks("item-pr-7", "fff999", &green).unwrap();
        assert_eq!(c.load_checks("item-pr-7", "fff999").unwrap(), Some(green));
        assert_eq!(c.load_checks("item-pr-7", "abc123").unwrap(), None);
    }

    #[test]
    fn viewed_is_keyed_by_blob_so_the_same_path_can_be_viewed_at_one_and_not_another() {
        let c = Cache::open_in_memory().unwrap();
        assert!(c.load_viewed("item-pr-7").unwrap().is_empty());

        c.set_viewed("item-pr-7", "src/x.rs", "blob-a", true)
            .unwrap();
        c.set_viewed("item-pr-7", "src/y.rs", "blob-y", true)
            .unwrap();
        let viewed = c.load_viewed("item-pr-7").unwrap();
        assert!(viewed.contains(&("src/x.rs".into(), "blob-a".into())));
        assert!(viewed.contains(&("src/y.rs".into(), "blob-y".into())));
        // The file was pushed to: a new blob, and no row for it.
        assert!(!viewed.contains(&("src/x.rs".into(), "blob-b".into())));

        // Viewing the new blob leaves the old one's row alone, so a revert
        // to it is still viewed.
        c.set_viewed("item-pr-7", "src/x.rs", "blob-b", true)
            .unwrap();
        let viewed = c.load_viewed("item-pr-7").unwrap();
        assert!(viewed.contains(&("src/x.rs".into(), "blob-a".into())));
        assert!(viewed.contains(&("src/x.rs".into(), "blob-b".into())));

        // Unviewing is by blob too, and marking twice is one row.
        c.set_viewed("item-pr-7", "src/x.rs", "blob-a", true)
            .unwrap();
        c.set_viewed("item-pr-7", "src/x.rs", "blob-a", false)
            .unwrap();
        let viewed = c.load_viewed("item-pr-7").unwrap();
        assert!(!viewed.contains(&("src/x.rs".into(), "blob-a".into())));
        assert_eq!(viewed.len(), 2);
        assert!(
            c.load_viewed("item-pr-8").unwrap().is_empty(),
            "keyed by item"
        );
    }

    #[test]
    fn reveals_round_trip_by_path_and_blob_and_a_new_blob_has_none() {
        let c = Cache::open_in_memory().unwrap();
        assert!(c.load_reveals("item-pr-7").unwrap().is_empty());

        c.set_reveal("item-pr-7", "src/x.rs", "blob-a", 40, 59)
            .unwrap();
        c.set_reveal("item-pr-7", "src/x.rs", "blob-a", 4, 23)
            .unwrap();
        c.set_reveal("item-pr-7", "src/y.rs", "blob-y", 0, 19)
            .unwrap();
        // Asking for the same range twice is one row.
        c.set_reveal("item-pr-7", "src/x.rs", "blob-a", 4, 23)
            .unwrap();
        let reveals = c.load_reveals("item-pr-7").unwrap();
        assert_eq!(reveals.len(), 2);
        assert_eq!(
            reveals[&("src/x.rs".into(), "blob-a".into())],
            vec![(4, 23), (40, 59)],
            "each file's in order"
        );
        assert_eq!(
            reveals[&("src/y.rs".into(), "blob-y".into())],
            vec![(0, 19)]
        );
        // The file was pushed to: a new blob, and no rows for it.
        assert!(!reveals.contains_key(&("src/x.rs".to_string(), "blob-b".to_string())));
        assert!(
            c.load_reveals("item-pr-8").unwrap().is_empty(),
            "keyed by item"
        );
    }

    #[test]
    fn removing_reveals_takes_those_overlapping_the_range_and_no_other() {
        let c = Cache::open_in_memory().unwrap();
        let key = ("src/x.rs".to_string(), "blob-a".to_string());
        for (first, last) in [(0, 9), (10, 29), (30, 30), (31, 50), (60, 79)] {
            c.set_reveal("item-pr-7", "src/x.rs", "blob-a", first, last)
                .unwrap();
        }
        c.set_reveal("item-pr-7", "src/x.rs", "blob-b", 10, 29)
            .unwrap();
        c.set_reveal("other", "src/x.rs", "blob-a", 10, 29).unwrap();

        // A run of 10 to 30: the reveal that ends on its first line and the
        // one that begins on its last both overlap it; the ones either side
        // of those do not.
        c.remove_reveals("item-pr-7", "src/x.rs", "blob-a", 10, 30)
            .unwrap();
        let reveals = c.load_reveals("item-pr-7").unwrap();
        assert_eq!(reveals[&key], vec![(0, 9), (31, 50), (60, 79)]);
        assert_eq!(
            reveals[&("src/x.rs".into(), "blob-b".into())],
            vec![(10, 29)],
            "another blob's rows stay"
        );
        assert_eq!(
            c.load_reveals("other").unwrap().len(),
            1,
            "another item's rows stay"
        );

        // A range that touches a reveal's edge takes it.
        c.remove_reveals("item-pr-7", "src/x.rs", "blob-a", 50, 60)
            .unwrap();
        let reveals = c.load_reveals("item-pr-7").unwrap();
        assert_eq!(reveals[&key], vec![(0, 9)]);
        // A file with nothing left has no entry at all.
        c.remove_reveals("item-pr-7", "src/x.rs", "blob-a", 0, 0)
            .unwrap();
        assert!(!c.load_reveals("item-pr-7").unwrap().contains_key(&key));
    }

    #[test]
    fn deleting_an_item_takes_what_was_fetched_and_marked_for_it() {
        use crate::workflow_runs::CheckState;
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let id = pr_item(7).id;
        c.store_discussion(&id, &discussion()).unwrap();
        c.store_checks(
            &id,
            "abc123",
            &Checks {
                state: CheckState::Passing,
                url: None,
                runs: Vec::new(),
            },
        )
        .unwrap();
        c.set_viewed(&id, "src/x.rs", "blob-a", true).unwrap();
        c.set_viewed("other", "src/x.rs", "blob-a", true).unwrap();
        c.set_reveal(&id, "src/x.rs", "blob-a", 4, 23).unwrap();
        c.set_reveal("other", "src/x.rs", "blob-a", 4, 23).unwrap();

        c.delete_item(&id).unwrap();
        assert_eq!(c.load_discussion(&id).unwrap(), None);
        assert_eq!(c.load_checks(&id, "abc123").unwrap(), None);
        assert!(c.load_viewed(&id).unwrap().is_empty());
        assert_eq!(
            c.load_viewed("other").unwrap().len(),
            1,
            "another item's rows stay"
        );
        assert!(c.load_reveals(&id).unwrap().is_empty());
        assert_eq!(c.load_reveals("other").unwrap().len(), 1, "and its reveals");
    }

    #[test]
    fn meta_round_trips() {
        let c = Cache::open_in_memory().unwrap();
        assert_eq!(c.meta("last_modified").unwrap(), None);
        c.set_meta("last_modified", "Wed, 17 Sep 2026 18:00:00 GMT")
            .unwrap();
        assert_eq!(
            c.meta("last_modified").unwrap().as_deref(),
            Some("Wed, 17 Sep 2026 18:00:00 GMT")
        );
    }

    #[test]
    fn notification_snapshot_and_validator_round_trip_in_api_order() {
        let c = Cache::open_in_memory().unwrap();
        let mut locally_unread = notification_thread("20", "2026-09-19T12:00:00Z");
        locally_unread.local_unread = true;
        let threads = vec![
            locally_unread,
            notification_thread("10", "2026-09-18T12:00:00Z"),
        ];

        c.store_threads(&threads, Some("Sat, 19 Sep 2026 12:00:00 GMT"))
            .unwrap();

        assert_eq!(c.load_threads().unwrap(), threads);
        assert_eq!(
            c.notifications_last_modified().unwrap().as_deref(),
            Some("Sat, 19 Sep 2026 12:00:00 GMT")
        );
    }

    #[test]
    fn notification_snapshot_replaces_rows_and_can_clear_its_validator() {
        let c = Cache::open_in_memory().unwrap();
        c.store_threads(
            &[
                notification_thread("1", "2026-09-17T18:00:00Z"),
                notification_thread("2", "2026-09-17T17:00:00Z"),
            ],
            Some("Thu, 17 Sep 2026 18:00:00 GMT"),
        )
        .unwrap();

        let replacement = notification_thread("3", "2026-09-19T09:00:00Z");
        c.store_threads(std::slice::from_ref(&replacement), None)
            .unwrap();

        assert_eq!(c.load_threads().unwrap(), vec![replacement]);
        assert_eq!(c.notifications_last_modified().unwrap(), None);
    }

    #[test]
    fn a_done_notification_stays_hidden_across_refresh_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let dismissed = notification_thread("1", "2026-09-19T12:00:00Z");
        let other = notification_thread("2", "2026-09-19T11:00:00Z");
        {
            let c = Cache::open(&path, None).unwrap();
            c.store_threads(
                &[dismissed.clone(), other.clone()],
                Some("Sat, 19 Sep 2026 12:00:00 GMT"),
            )
            .unwrap();
            c.dismiss_notification(&dismissed.id, dismissed.updated_at)
                .unwrap();
            assert_eq!(c.load_threads().unwrap(), vec![other.clone()]);
        }

        let c = Cache::open(&path, None).unwrap();
        assert_eq!(c.load_threads().unwrap(), vec![other.clone()]);
        let visible = c
            .store_threads(
                &[dismissed, other.clone()],
                Some("Sat, 19 Sep 2026 12:00:00 GMT"),
            )
            .unwrap();
        assert_eq!(visible, vec![other.clone()]);
        assert_eq!(c.load_threads().unwrap(), vec![other]);
    }

    #[test]
    fn newer_activity_clears_done_and_returns_in_github_order() {
        let c = Cache::open_in_memory().unwrap();
        let dismissed = notification_thread("1", "2026-09-19T12:00:00Z");
        c.dismiss_notification(&dismissed.id, dismissed.updated_at)
            .unwrap();

        let older = notification_thread("1", "2026-09-19T11:00:00Z");
        let other = notification_thread("2", "2026-09-19T12:30:00Z");
        let visible = c.store_threads(&[other.clone(), older], None).unwrap();
        assert_eq!(visible, vec![other.clone()]);

        let newer = notification_thread("1", "2026-09-20T09:00:00Z");
        let visible = c
            .store_threads(&[newer.clone(), other.clone()], None)
            .unwrap();
        assert_eq!(visible, vec![newer.clone(), other.clone()]);
        assert_eq!(c.load_threads().unwrap(), vec![newer, other]);
        assert!(c.notification_dismissals().unwrap().is_empty());
    }

    #[test]
    fn a_stale_done_cannot_hide_newer_activity_or_regress_the_watermark() {
        let c = Cache::open_in_memory().unwrap();
        let dismissed = notification_thread("1", "2026-09-19T12:00:00Z");
        c.dismiss_notification(&dismissed.id, dismissed.updated_at)
            .unwrap();

        let stale = stamp("2026-09-19T11:00:00Z");
        c.dismiss_notification(&dismissed.id, stale).unwrap();
        assert_eq!(
            c.notification_dismissals().unwrap().get(&dismissed.id),
            Some(&dismissed.updated_at)
        );

        let newer = notification_thread("1", "2026-09-20T09:00:00Z");
        c.store_threads(std::slice::from_ref(&newer), None)
            .unwrap();
        c.dismiss_notification(&newer.id, stale).unwrap();
        assert_eq!(c.load_threads().unwrap(), vec![newer]);
    }

    #[test]
    fn failed_notification_replacement_rolls_back_rows_and_validator_together() {
        let c = Cache::open_in_memory().unwrap();
        let original = notification_thread("1", "2026-09-17T18:00:00Z");
        c.store_threads(
            std::slice::from_ref(&original),
            Some("Thu, 17 Sep 2026 18:00:00 GMT"),
        )
        .unwrap();

        let duplicate = notification_thread("2", "2026-09-19T09:00:00Z");
        assert!(c
            .store_threads(
                &[duplicate.clone(), duplicate],
                Some("Sat, 19 Sep 2026 09:00:00 GMT"),
            )
            .is_err());

        assert_eq!(c.load_threads().unwrap(), vec![original]);
        assert_eq!(
            c.notifications_last_modified().unwrap().as_deref(),
            Some("Thu, 17 Sep 2026 18:00:00 GMT")
        );
    }

    #[test]
    fn avatar_hash_is_stable_and_distinct() {
        assert_eq!(hash("https://a"), hash("https://a"));
        assert_ne!(hash("https://a"), hash("https://b"));
    }
}
