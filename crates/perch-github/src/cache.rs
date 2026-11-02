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

use crate::activity::{ActivityEvent, ItemActivity};
use crate::comments::{
    self, Anchor, Author, CommentMessage, CommentThread, NewRunEvent, RevisionContext, RunEvent,
    RunEventRecord, RunHistoryCursor, RunHistoryFilter, RunHistoryPage, RunLabels, RunMode,
    RunOwner, RunPurpose, RunRecord, RunStatus, RunStep, RunSummary, TerminalReason, ThreadOrigin,
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
/// the discussions already kept. Version 22 moves who did a thing off the
/// merge event and onto every event, for the rest of what GitHub's
/// timeline lists. Version 23 keeps each GitHub thread's own id and
/// whether the reader may resolve it. Version 24 keeps the instructions a
/// worktree review was run with. Version 30 keeps the `HEAD` a review of
/// uncommitted work was captured over. Version 32 keeps the thread a Done
/// mark hides, for the Inbox's Done list. Version 33 lets a mark be a
/// snooze, which lapses at a time. Version 34 keeps the reader's answer on
/// running an item's code in a language server, and starts the branches
/// already cut from a pull request untrusted. Version 35 keeps the servers
/// the reader trusts on everything in a repository. Version 36 drops the
/// copies of tool calls an update wrote into a run's history as legacy
/// steps. Version 37 clears the activity the inbox stopped showing, so
/// what comes back to it starts empty. Schemas 13 through 23
/// migrate in place so local conversations and agent history are never
/// discarded.
pub const SCHEMA_VERSION: u32 = 37;

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

/// What a run recorded of the discussion it was given: the directory the
/// files were written to, and the manifest of what its prompt showed, as
/// JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunContext {
    pub dir: Option<String>,
    pub manifest: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEventAppend {
    Appended(RunEventRecord),
    Duplicate(RunEventRecord),
}

/// A draft as the review submission reads it: the message, its thread's
/// root comment on GitHub when the thread has one, and the anchor it
/// posts at when it does not.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftRow {
    pub message_id: String,
    pub thread_id: String,
    /// The root comment on GitHub the draft replies to. `None` for a
    /// thread the draft opens.
    pub thread_github_id: Option<u64>,
    pub anchor: Option<Anchor>,
    pub body: String,
    pub thread_created_at: Timestamp,
    pub created_at: Timestamp,
}

/// What became of the drafts that rode in a review once GitHub's copy of
/// the discussion was read back: the ones found there, now posted, and the
/// ones not found, still drafts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub matched: Vec<String>,
    pub unmatched: Vec<String>,
}

/// Something the reader always allows Work runs in a repository to do,
/// kept from an approval card's "Always". `key` is what a request is
/// matched on (`network:example.com`, `command:<text>`, `tool:<name>:<what>`,
/// `commit`); `label` is what the card said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkGrantRecord {
    pub id: String,
    pub repo: String,
    pub key: String,
    pub label: String,
    pub created_at: Timestamp,
}

const GRANT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS work_grants (
    id          TEXT PRIMARY KEY,
    repo        TEXT NOT NULL,
    key         TEXT NOT NULL,
    label       TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS work_grants_key ON work_grants(repo COLLATE NOCASE, key);
"#;

const ACTIVITY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS item_activity (
    item_id       TEXT PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    at            TEXT NOT NULL,
    event_json    TEXT NOT NULL,
    read_at       TEXT,
    done_at       TEXT,
    local_unread  INTEGER NOT NULL DEFAULT 0
);
"#;

/// The reader's answer on running an item's code in a language server
/// that runs it (rust-analyzer, jdtls): 1 after Trust and start, 0 for a
/// branch cut from code the reader hadn't trusted. An item with no row is
/// judged by `[repo.lsp]` and whose work it is.
const LSP_TRUST_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS item_lsp_trust (
    item_id  TEXT PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    trusted  INTEGER NOT NULL
);
"#;

/// The language servers the reader trusts on everything in a repository,
/// anyone's pull request included, from Always trust: rust-analyzer or
/// jdtls by name. A repository matches without regard to case, as the
/// config's do.
const REPO_LSP_TRUST_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS repo_lsp_trust (
    repo    TEXT NOT NULL COLLATE NOCASE,
    server  TEXT NOT NULL,
    PRIMARY KEY (repo, server)
);
"#;

pub struct Cache {
    conn: Connection,
    avatar_dir: Option<PathBuf>,
}

/// A Done mark or a snooze: the revision of a thread it hides, and for a
/// snooze when it lapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotificationMark {
    updated_at: Timestamp,
    snoozed_until: Option<Timestamp>,
}

impl NotificationMark {
    /// Whether the mark keeps `thread` out of the inbox at `now`. Done
    /// holds until newer activity. A snooze holds until its time, or until
    /// newer activity GitHub reports unread: a read revision is likely the
    /// reader's own doing elsewhere.
    fn hides(&self, thread: &Thread, now: Timestamp) -> bool {
        match self.snoozed_until {
            None => thread.updated_at <= self.updated_at,
            Some(until) => now < until && (thread.updated_at <= self.updated_at || !thread.unread),
        }
    }
}

fn notification_marks(conn: &Connection) -> Result<HashMap<String, NotificationMark>, CacheError> {
    let mut stmt =
        conn.prepare("SELECT thread_id, updated_at, snoozed_until FROM notification_dismissals")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut marks = HashMap::new();
    for row in rows {
        let (thread_id, updated_at, until) = row?;
        let mark = NotificationMark {
            updated_at: parse_mark_time(&thread_id, "updated_at", &updated_at)?,
            snoozed_until: until
                .map(|until| parse_mark_time(&thread_id, "snoozed_until", &until))
                .transpose()?,
        };
        marks.insert(thread_id, mark);
    }
    Ok(marks)
}

fn parse_mark_time(thread_id: &str, column: &str, value: &str) -> Result<Timestamp, CacheError> {
    value.parse::<Timestamp>().map_err(|error| {
        CacheError::Corrupt(format!(
            "notification dismissal {thread_id} has invalid {column}: {error}"
        ))
    })
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
    thread_id      TEXT PRIMARY KEY,
    updated_at     TEXT NOT NULL,
    json           TEXT,
    snoozed_until  TEXT
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
    source_path      TEXT,
    divergence       TEXT,
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
    created_at     TEXT NOT NULL,
    node_id        TEXT,
    can_resolve    INTEGER NOT NULL DEFAULT 0
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
    context_dir            TEXT,
    context_manifest       TEXT,
    mode                   TEXT NOT NULL DEFAULT 'ask',
    work_before            TEXT,
    work_after             TEXT,
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

/// The columns schema 23 adds to a comment thread, with their types: what
/// resolving a thread from GitHub needs.
const GITHUB_THREAD_COLUMNS: [(&str, &str); 2] = [
    ("node_id", "TEXT"),
    ("can_resolve", "INTEGER NOT NULL DEFAULT 0"),
];

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
        tx.execute_batch(GRANT_SCHEMA)?;
        tx.execute_batch(ACTIVITY_SCHEMA)?;
        tx.execute_batch(LSP_TRUST_SCHEMA)?;
        tx.execute_batch(REPO_LSP_TRUST_SCHEMA)?;
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
            21 => migrate_21_to_22(&tx)?,
            22 => add_github_thread_columns(&tx)?,
            23 => review_store::add_group_instructions(&tx)?,
            24 => add_run_context_columns(&tx)?,
            25 => add_work_columns(&tx)?,
            26 => tx.execute_batch(GRANT_SCHEMA)?,
            27 => add_divergence_column(&tx)?,
            // Empty: nothing that happened before is backfilled.
            28 => tx.execute_batch(ACTIVITY_SCHEMA)?,
            29 => review_store::add_snapshot_worktree_parent(&tx)?,
            30 => fold_saved_into_pinned(&tx)?,
            31 => add_dismissed_thread_column(&tx)?,
            32 => add_snoozed_until_column(&tx)?,
            33 => add_code_trust_table(&tx)?,
            34 => tx.execute_batch(REPO_LSP_TRUST_SCHEMA)?,
            35 => drop_repeated_legacy_steps(&tx)?,
            // Old enough that none of it is news.
            36 => tx.execute_batch("DELETE FROM item_activity")?,
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

/// Legacy steps a run with step events of its own was given by
/// `sync_legacy_steps`, which copied those events' steps again on each
/// update. Only a run from before events has legacy steps of its own.
fn drop_repeated_legacy_steps(tx: &Transaction<'_>) -> Result<(), CacheError> {
    tx.execute(
        "DELETE FROM run_events WHERE kind='legacy_step' AND run_id IN (
             SELECT run_id FROM run_events WHERE kind IN ('tool_call','steer','steer_landed'))",
        [],
    )?;
    Ok(())
}

/// A Done mark keeps the thread it hides. The marks already made have
/// none, so the stored validator goes too: the next poll fetches the whole
/// list rather than a 304, and fills in the ones GitHub still lists.
fn add_dismissed_thread_column(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('notification_dismissals') WHERE name='json')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        tx.execute(
            "ALTER TABLE notification_dismissals ADD COLUMN json TEXT",
            [],
        )?;
    }
    tx.execute(
        "DELETE FROM meta WHERE key = ?1",
        params![NOTIFICATIONS_LAST_MODIFIED_KEY],
    )?;
    Ok(())
}

/// A mark may be a snooze, which lapses at a time. The marks already
/// made are Done marks, with none.
fn add_snoozed_until_column(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('notification_dismissals') WHERE name='snoozed_until')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        tx.execute(
            "ALTER TABLE notification_dismissals ADD COLUMN snoozed_until TEXT",
            [],
        )?;
    }
    Ok(())
}

/// The trust table, with the branches already cut from a pull request,
/// and those cut from them, marked untrusted: they hold the pull
/// request's author's code, and nothing before said whose code it was.
/// A branch whose pull request was deleted lost its pointer
/// (`delete_item`), so its name counts too: one cut or forked from a pull
/// request's checkout, `perch/pr-N-work` or `perch/pr-N-2`, a pull request
/// of its own since or not, and any other `perch/pr-` branch with none.
/// A pull request's own checkout, `perch/pr-N`, is judged as the pull
/// request is. Wrong here costs the reader one Trust and start.
fn add_code_trust_table(tx: &Transaction<'_>) -> Result<(), CacheError> {
    tx.execute_batch(LSP_TRUST_SCHEMA)?;
    tx.execute(
        "WITH RECURSIVE cut(id) AS (
             SELECT id FROM items
             WHERE from_item_id IN (SELECT id FROM items WHERE subject_kind = 'pr')
                OR (from_item_id IS NULL AND branch LIKE 'perch/pr-%-%')
                OR (subject_kind IS NULL AND branch LIKE 'perch/pr-%')
             UNION
             SELECT items.id FROM items JOIN cut ON items.from_item_id = cut.id
         )
         INSERT OR IGNORE INTO item_lsp_trust (item_id, trusted) SELECT id, 0 FROM cut",
        [],
    )?;
    Ok(())
}

/// Save folded into Pin. A saved row with no pin is pinned as of when it
/// was last opened, so it sorts among the pins by that; one already
/// pinned keeps its pin. The column stays, always 0: dropping it would
/// mean rebuilding `items`, which other tables reference.
fn fold_saved_into_pinned(tx: &Transaction<'_>) -> Result<(), CacheError> {
    tx.execute_batch(
        "UPDATE items SET pinned_at = last_opened_at WHERE saved != 0 AND pinned_at IS NULL;
         UPDATE items SET saved = 0 WHERE saved != 0;",
    )?;
    Ok(())
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
    // Ingesting writes a thread's columns as they are now, schema 23's
    // among them.
    add_github_thread_columns(tx)?;
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

/// A comment thread gains its id on GitHub and whether the reader may
/// resolve it, each left empty until the next fetch says. Also run ahead
/// of schema 21's ingestion, which writes them.
fn add_github_thread_columns(tx: &Transaction<'_>) -> Result<(), CacheError> {
    for (column, kind) in GITHUB_THREAD_COLUMNS {
        let present: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM pragma_table_info('comment_threads') WHERE name=?1
             )",
            params![column],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute(
                &format!("ALTER TABLE comment_threads ADD COLUMN {column} {kind}"),
                [],
            )?;
        }
    }
    Ok(())
}

/// The columns schema 25 adds to a run: the directory of the discussion
/// it was pointed at, and the manifest of what it was shown.
const RUN_CONTEXT_COLUMNS: [&str; 2] = ["context_dir", "context_manifest"];

fn add_run_context_columns(tx: &Transaction<'_>) -> Result<(), CacheError> {
    for column in RUN_CONTEXT_COLUMNS {
        let present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runs') WHERE name=?1)",
            params![column],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute(&format!("ALTER TABLE runs ADD COLUMN {column} TEXT"), [])?;
        }
    }
    Ok(())
}

/// Schema 26: an owned checkout says which repository it was added from,
/// and a run says whether it was a question or work, and the commits
/// that record the worktree before and after work.
fn add_work_columns(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let columns = [
        ("items", "source_path", "TEXT"),
        ("runs", "mode", "TEXT NOT NULL DEFAULT 'ask'"),
        ("runs", "work_before", "TEXT"),
        ("runs", "work_after", "TEXT"),
    ];
    for (table, column, kind) in columns {
        let present: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name=?1)"),
            params![column],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
                [],
            )?;
        }
    }
    Ok(())
}

/// An item's checkout remembers how its head stood against its pull
/// request's. Nothing is filled in: the next scan or open counts it.
fn add_divergence_column(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('items') WHERE name='divergence')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        tx.execute("ALTER TABLE items ADD COLUMN divergence TEXT", [])?;
    }
    Ok(())
}

/// Every event gains an actor, and the merge gives up its own `by`.
/// Rewritten in place rather than dropped, so a pull request read offline
/// still says how it ended before the next fetch. A row that no longer
/// reads is passed over, as the fetch replaces it whole.
fn migrate_21_to_22(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let discussions = {
        let mut stmt = tx.prepare("SELECT item_id, json FROM discussions")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (item_id, json) in discussions {
        let Ok(mut row) = serde_json::from_str::<serde_json::Value>(&json) else {
            tracing::warn!(item_id, "skipping an unreadable cached discussion");
            continue;
        };
        let Some(events) = row.get_mut("events").and_then(|e| e.as_array_mut()) else {
            continue;
        };
        for event in events {
            // `{"kind":{"merged":{"by":"rohan"}}}` becomes
            // `{"kind":"merged","actor":"rohan"}`; every other kind was a
            // bare name already and only wants the empty actor.
            let by = event
                .get("kind")
                .and_then(|k| k.get("merged"))
                .and_then(|m| m.get("by"))
                .and_then(|by| by.as_str())
                .unwrap_or_default()
                .to_string();
            if event.get("kind").is_some_and(|k| k.get("merged").is_some()) {
                event["kind"] = serde_json::Value::String("merged".into());
            }
            event["actor"] = serde_json::Value::String(by);
        }
        tx.execute(
            "UPDATE discussions SET json=?2 WHERE item_id=?1",
            params![item_id, serde_json::to_string(&row)?],
        )?;
    }
    Ok(())
}

/// The head of an item's pull request, or nothing when the item has none.
/// Reads the one column it needs, since a migration calls it on a schema
/// older than `ITEM_COLUMNS`.
fn item_head_sha_on(conn: &Connection, item_id: &str) -> Result<String, CacheError> {
    let subject_json: Option<String> = conn
        .query_row(
            "SELECT subject_json FROM items WHERE id = ?1",
            params![item_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let subject: Option<Subject> = subject_json.and_then(|j| serde_json::from_str(&j).ok());
    Ok(match subject {
        Some(Subject::PullRequest(pr)) => pr.head_sha,
        _ => String::new(),
    })
}

/// Put GitHub's review threads into the cache beside Perch's own,
/// replacing what was kept of them before. A thread keeps its id, and so
/// everything local under it: the reader's questions to an agent and the
/// answers, and a draft not yet posted. Only comments that are on GitHub
/// are replaced. A thread no longer on GitHub keeps its row while anything
/// else is under it or a review finding owns it, and goes otherwise. The
/// one that stays has no comment on GitHub to point at any more, so it
/// loses its `github_id` and is a thread on its way there, as a draft's is.
///
/// A row already pointing at a comment by `github_id` is the row that
/// comment updates, whatever its id and whatever thread it is under: a
/// thread the reader opened here and posted keeps its Perch id, and so
/// its runs and private messages, and a posted comment of the reader's
/// keeps its id and stays the reader's rather than becoming a GitHub
/// user's. A comment that is a root now but was a reply, its own root
/// deleted on github.com, takes over the thread it was under, so what
/// Perch put there stays with it. A thread that is left with nothing
/// bound for GitHub under it is Perch's own again, as it is when its
/// last draft is deleted.
///
/// `incoming` is what GitHub lists now. Runs inside the caller's
/// transaction; nothing here is written on its own.
pub(crate) fn ingest_review_threads(
    conn: &Connection,
    item_id: &str,
    incoming: &[CommentThread],
) -> Result<(), CacheError> {
    let roots: HashSet<u64> = incoming.iter().filter_map(|t| t.github_id).collect();
    for thread in incoming {
        let anchor = thread.anchor.as_ref();
        let thread_id = match thread.github_id {
            Some(github_id) => thread_row_for_root(conn, item_id, github_id, &roots)?
                .unwrap_or_else(|| thread.id.clone()),
            None => thread.id.clone(),
        };
        conn.execute(
            "INSERT INTO comment_threads
             (id, worktree_id, origin, github_id, path, start_line, line, side, commit_sha,
              outdated, resolved, created_at, node_id, can_resolve)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                worktree_id=excluded.worktree_id, origin=excluded.origin,
                github_id=excluded.github_id, path=excluded.path,
                start_line=excluded.start_line, line=excluded.line, side=excluded.side,
                commit_sha=excluded.commit_sha, outdated=excluded.outdated,
                resolved=excluded.resolved,
                created_at=min(created_at, excluded.created_at),
                node_id=excluded.node_id, can_resolve=excluded.can_resolve",
            params![
                thread_id,
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
                thread.node_id,
                thread.can_resolve as i64,
            ],
        )?;
        for message in &thread.messages {
            let posted: Option<String> = match message.github_id {
                Some(github_id) => conn
                    .query_row(
                        "SELECT m.id FROM comment_messages m
                         JOIN comment_threads t ON t.id = m.thread_id
                         WHERE t.worktree_id = ?1 AND m.github_id = ?2",
                        params![item_id, github_id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => None,
            };
            if let Some(id) = posted {
                conn.execute(
                    "UPDATE comment_messages
                     SET thread_id = ?2, body = ?3, node_id = ?4, html_url = ?5,
                         diff_hunk = ?6, reactions_json = ?7
                     WHERE id = ?1",
                    params![
                        id,
                        thread_id,
                        message.body,
                        message.node_id,
                        message.html_url,
                        message.diff_hunk,
                        reactions_json(&message.reactions),
                    ],
                )?;
                continue;
            }
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
                    thread_id,
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

    // What is on GitHub is known by its id there, not by its row's.
    let keep: HashSet<u64> = incoming
        .iter()
        .flat_map(|t| t.messages.iter().filter_map(|m| m.github_id))
        .collect();
    for id in stale_message_rows(conn, item_id, &keep)? {
        conn.execute("DELETE FROM comment_messages WHERE id = ?1", params![id])?;
    }
    // A review finding's thread stays whatever went from under it: the
    // finding, and its history, would go with the row.
    conn.execute(
        "DELETE FROM comment_threads
         WHERE worktree_id = ?1 AND origin = 'github'
           AND NOT EXISTS (SELECT 1 FROM comment_messages WHERE thread_id = comment_threads.id)
           AND NOT EXISTS (SELECT 1 FROM review_findings WHERE thread_id = comment_threads.id)",
        params![item_id],
    )?;

    for id in stale_thread_rows(conn, item_id, &roots)? {
        conn.execute(
            "UPDATE comment_threads SET github_id = NULL, node_id = NULL, can_resolve = 0
             WHERE id = ?1",
            params![id],
        )?;
    }
    // A thread with nothing bound for GitHub under it any more carries no
    // destination of its own: the messages do.
    conn.execute(
        "UPDATE comment_threads SET origin = 'local'
         WHERE worktree_id = ?1 AND origin = 'github' AND github_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM comment_messages
                           WHERE thread_id = comment_threads.id AND origin = 'github')",
        params![item_id],
    )?;
    Ok(())
}

/// The row of the thread whose root is comment `github_id`, when the
/// cache has one: the thread pointing at it, else the thread holding it
/// as a message, a reply whose own root is gone from GitHub, unless that
/// thread is still another incoming root's. `None` when the comment is
/// new here.
fn thread_row_for_root(
    conn: &Connection,
    item_id: &str,
    github_id: u64,
    roots: &HashSet<u64>,
) -> Result<Option<String>, CacheError> {
    let pointing: Option<String> = conn
        .query_row(
            "SELECT id FROM comment_threads WHERE worktree_id = ?1 AND github_id = ?2",
            params![item_id, github_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    if pointing.is_some() {
        return Ok(pointing);
    }
    let holding: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT t.id, t.github_id FROM comment_threads t
             JOIN comment_messages m ON m.thread_id = t.id
             WHERE t.worktree_id = ?1 AND m.github_id = ?2",
            params![item_id, github_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(holding
        .filter(|(_, root)| !is_kept(root, roots))
        .map(|(id, _)| id))
}

/// Whether a `github_id` column names a comment among `keep`.
fn is_kept(column: &Option<String>, keep: &HashSet<u64>) -> bool {
    github_id_of(column.clone()).is_some_and(|id| keep.contains(&id))
}

/// The rows of comments on GitHub under `item_id` whose comment is not
/// among `keep`, the ids GitHub still lists: what a refresh deletes.
fn stale_message_rows(
    conn: &Connection,
    item_id: &str,
    keep: &HashSet<u64>,
) -> Result<Vec<String>, CacheError> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.github_id FROM comment_messages m
         JOIN comment_threads t ON t.id = m.thread_id
         WHERE t.worktree_id = ?1 AND m.origin = 'github' AND m.github_id IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![item_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(_, github_id)| !is_kept(github_id, keep))
        .map(|(id, _)| id)
        .collect())
}

/// The rows of threads under `item_id` pointing at a root comment not
/// among `keep`, the roots GitHub still lists: what a refresh unpoints.
fn stale_thread_rows(
    conn: &Connection,
    item_id: &str,
    keep: &HashSet<u64>,
) -> Result<Vec<String>, CacheError> {
    let mut stmt = conn.prepare(
        "SELECT id, github_id FROM comment_threads
         WHERE worktree_id = ?1 AND origin = 'github' AND github_id IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![item_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(_, github_id)| !is_kept(github_id, keep))
        .map(|(id, _)| id)
        .collect())
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
            mode: crate::comments::RunMode::Ask,
            work_before: None,
            work_after: None,
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
    last_event_sequence, mode, work_before, work_after";

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
    mode: String,
    work_before: Option<String>,
    work_after: Option<String>,
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
        mode: row.get(36)?,
        work_before: row.get(37)?,
        work_after: row.get(38)?,
    })
}

/// A stored timestamp, or `Corrupt(field)` when it does not parse.
fn timestamp(value: String, field: &str) -> Result<Timestamp, CacheError> {
    value.parse().map_err(|_| CacheError::Corrupt(field.into()))
}

fn optional_timestamp(value: Option<String>, field: &str) -> Result<Option<Timestamp>, CacheError> {
    value.map(|value| timestamp(value, field)).transpose()
}

/// The owner a run row's owner columns name: the inverse of
/// [`owner_columns`].
fn run_owner(
    kind: &str,
    item_id: Option<String>,
    thread_id: Option<String>,
    request_id: Option<String>,
    reviewer_run_id: Option<String>,
) -> Result<RunOwner, CacheError> {
    Ok(match kind {
        "discussion" => RunOwner::Discussion {
            item_id: item_id.ok_or_else(|| CacheError::Corrupt("discussion item id".into()))?,
            thread_id: thread_id
                .ok_or_else(|| CacheError::Corrupt("discussion thread id".into()))?,
            request_id: request_id
                .ok_or_else(|| CacheError::Corrupt("discussion request id".into()))?,
        },
        "review" => RunOwner::Review {
            item_id: item_id.ok_or_else(|| CacheError::Corrupt("review item id".into()))?,
            reviewer_run_id: reviewer_run_id
                .ok_or_else(|| CacheError::Corrupt("reviewer run id".into()))?,
        },
        "unavailable" => RunOwner::Unavailable {
            legacy_thread_id: thread_id,
        },
        _ => return Err(CacheError::Corrupt("run owner kind".into())),
    })
}

/// A run row's purpose, status and terminal reason.
fn run_state(
    purpose: &str,
    status: &str,
    terminal_reason: Option<String>,
) -> Result<(RunPurpose, RunStatus, Option<TerminalReason>), CacheError> {
    let purpose =
        RunPurpose::parse(purpose).ok_or_else(|| CacheError::Corrupt("run purpose".into()))?;
    let status =
        RunStatus::parse(status).ok_or_else(|| CacheError::Corrupt("run status".into()))?;
    let terminal_reason = terminal_reason
        .map(|reason| {
            TerminalReason::parse(&reason)
                .ok_or_else(|| CacheError::Corrupt("terminal reason".into()))
        })
        .transpose()?;
    Ok((purpose, status, terminal_reason))
}

fn run_from_db(row: RunDbRow, steps: Vec<RunStep>) -> Result<RunRecord, CacheError> {
    let owner = run_owner(
        &row.owner_kind,
        row.owner_item_id,
        row.owner_thread_id,
        row.owner_request_id,
        row.owner_reviewer_run_id,
    )?;
    let (purpose, status, terminal_reason) =
        run_state(&row.purpose, &row.status, row.terminal_reason)?;
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
        queued_at: timestamp(row.queued_at, "run queued_at")?,
        started_at: optional_timestamp(row.started_at, "run started_at")?,
        ended_at: optional_timestamp(row.ended_at, "run ended_at")?,
        final_result: row.final_result,
        final_result_truncated: row.final_result_truncated,
        latest_activity: row.latest_activity,
        last_event_sequence: row.last_event_sequence.max(0) as u64,
        mode: RunMode::parse(&row.mode).ok_or_else(|| CacheError::Corrupt("run mode".into()))?,
        work_before: row.work_before,
        work_after: row.work_after,
    })
}

const RUN_SUMMARY_COLUMNS: &str = "id, owner_kind, owner_item_id, owner_thread_id,
    owner_request_id, owner_reviewer_run_id, purpose, backend, effective_model,
    effective_effort, original_request, repository_label, item_label, branch_label, status,
    terminal_reason, cost_usd, duration_ms, usage_json, retry_of_id, error, queued_at,
    started_at, ended_at, latest_activity, last_event_sequence, mode, work_before, work_after";

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
    mode: String,
    work_before: Option<String>,
    work_after: Option<String>,
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
        mode: row.get(26)?,
        work_before: row.get(27)?,
        work_after: row.get(28)?,
    })
}

fn summary_from_db(row: RunSummaryDbRow) -> Result<RunSummary, CacheError> {
    let owner = run_owner(
        &row.owner_kind,
        row.owner_item_id,
        row.owner_thread_id,
        row.owner_request_id,
        row.owner_reviewer_run_id,
    )?;
    let (purpose, status, terminal_reason) =
        run_state(&row.purpose, &row.status, row.terminal_reason)?;
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
        queued_at: timestamp(row.queued_at, "run queued_at")?,
        started_at: optional_timestamp(row.started_at, "run started_at")?,
        ended_at: optional_timestamp(row.ended_at, "run ended_at")?,
        latest_activity: row.latest_activity,
        last_event_sequence: row.last_event_sequence.max(0) as u64,
        mode: RunMode::parse(&row.mode).ok_or_else(|| CacheError::Corrupt("run mode".into()))?,
        work_before: row.work_before,
        work_after: row.work_after,
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
          last_event_sequence, mode, work_before, work_after)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,
                 ?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,
                 ?35,?36,?37,?38,?39)",
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
            run.mode.name(),
            run.work_before,
            run.work_after,
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
         final_result_truncated=?15, latest_activity=?16, last_event_sequence=?17,
         work_before=?18, work_after=?19 WHERE id=?1",
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
            run.work_before,
            run.work_after,
        ],
    )?;
    Ok(())
}

fn load_run_on(conn: &Connection, id: &str) -> Result<Option<RunRecord>, CacheError> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1");
    let row = conn
        .prepare_cached(&sql)?
        .query_row(params![id], read_run_db_row)
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
        occurred_at: timestamp(occurred_at, "event occurred_at")?,
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

/// The tool lines in the order they ran, with what the agent said between
/// them as notes. Its last words are notes too: the view decides whether
/// they are still narration or the answer.
fn load_steps_on(conn: &Connection, run_id: &str) -> Result<Vec<RunStep>, CacheError> {
    let mut stmt = conn.prepare_cached(
        "SELECT payload_json FROM run_events
         WHERE run_id=?1 AND kind IN ('text','tool_call','tool_result','legacy_step','steer','steer_landed')
         ORDER BY sequence",
    )?;
    let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
    let mut steps = Vec::new();
    let mut tools: HashMap<String, usize> = HashMap::new();
    // Steers the agent has not read, oldest first. They go at the end.
    let mut unread: Vec<String> = Vec::new();
    for payload in rows {
        match serde_json::from_str::<RunEvent>(&payload?)? {
            RunEvent::LegacyStep { summary, ok } => steps.push(RunStep {
                summary,
                ok,
                note: false,
                pending: false,
                user: false,
            }),
            RunEvent::Steer { text } => unread.push(text),
            RunEvent::SteerLanded { text } => {
                if let Some(at) = unread.iter().position(|t| *t == text) {
                    unread.remove(at);
                }
                steps.push(RunStep {
                    summary: text,
                    ok: true,
                    note: false,
                    pending: false,
                    user: true,
                });
            }
            // A call is pending until its result comes. A run that ended
            // without one leaves it pending, and the view shows it as done.
            RunEvent::ToolCall {
                id,
                summary,
                parent_id,
                ..
            } => {
                tools.insert(id, steps.len());
                steps.push(RunStep {
                    summary: match parent_id {
                        Some(_) => format!("↳ {summary}"),
                        None => summary,
                    },
                    ok: true,
                    note: false,
                    pending: true,
                    user: false,
                });
            }
            RunEvent::Text { text } if !text.trim().is_empty() => steps.push(RunStep {
                summary: text.trim().to_string(),
                ok: true,
                note: true,
                pending: false,
                user: false,
            }),
            RunEvent::ToolResult { id, ok, summary } => {
                if let Some(step) = tools.get(&id).and_then(|index| steps.get_mut(*index)) {
                    step.ok = ok;
                    step.pending = false;
                    if !ok && !summary.is_empty() {
                        step.summary = format!("{}: {summary}", step.summary);
                    }
                }
            }
            _ => {}
        }
    }
    steps.extend(unread.into_iter().map(|text| RunStep {
        summary: text,
        ok: true,
        note: false,
        pending: true,
        user: true,
    }));
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
        // What the reader said is theirs, not what the agent is doing.
        RunEvent::Steer { .. } | RunEvent::SteerLanded { .. } => {}
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
        RunEvent::Text { text } | RunEvent::Steer { text } | RunEvent::SteerLanded { text } => {
            truncate_string(text, max)
        }
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
///
/// A run whose steps come from its own events is left alone: a record
/// read back from it carries those steps, and writing them again as
/// legacy steps would show each one twice, and once more per update.
fn sync_legacy_steps(conn: &Connection, run: &RunRecord) -> Result<(), CacheError> {
    let evented: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM run_events
         WHERE run_id=?1 AND kind IN ('tool_call','steer','steer_landed'))",
        params![run.id],
        |row| row.get(0),
    )?;
    if evented {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM run_events WHERE run_id=?1 AND kind='legacy_step'",
        params![run.id],
    )?;
    let mut sequence: u64 = conn.query_row(
        "SELECT COALESCE(max(sequence), 0) FROM run_events WHERE run_id=?1",
        params![run.id],
        |row| row.get::<_, i64>(0),
    )? as u64;
    // Notes come from the run's text events, which stay put.
    for (index, step) in run.steps.iter().enumerate().filter(|(_, step)| !step.note) {
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
        params![
            run.id,
            sequence as i64,
            run.steps
                .iter()
                .rev()
                .find(|step| !step.note)
                .map(|step| step.summary.as_str())
        ],
    )?;
    Ok(())
}

/// `delete_draft` on `conn`; see there.
fn delete_draft_on(conn: &Connection, message_id: &str) -> Result<Option<String>, CacheError> {
    let finding = review_store::open_finding_of_draft_on(conn, message_id)?;
    remove_draft_on(conn, message_id)?;
    if let Some(finding_id) = &finding {
        review_store::dismiss_for_deleted_draft_on(conn, finding_id, Timestamp::now())?;
    }
    Ok(finding)
}

/// Throw a draft away with what follows from it for its thread, as
/// `delete_draft` does, and leave any finding it speaks for as it is.
fn remove_draft_on(conn: &Connection, message_id: &str) -> Result<(), CacheError> {
    let thread_id: Option<String> = conn
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
    conn.execute(
        "DELETE FROM comment_messages WHERE id = ?1",
        params![message_id],
    )?;
    let on_its_way: bool = conn.query_row(
        "SELECT origin = 'github' AND github_id IS NULL FROM comment_threads WHERE id = ?1",
        params![thread_id],
        |row| row.get(0),
    )?;
    if !on_its_way {
        return Ok(());
    }
    let (left, github_left, finding): (i64, i64, bool) = conn.query_row(
        "SELECT COUNT(*), COUNT(CASE WHEN origin = 'github' THEN 1 END),
                EXISTS(SELECT 1 FROM review_findings WHERE thread_id = ?1)
         FROM comment_messages WHERE thread_id = ?1",
        params![thread_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if left == 0 && !finding {
        conn.execute(
            "DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE owner_thread_id=?1)",
            params![thread_id],
        )?;
        conn.execute(
            "DELETE FROM runs WHERE owner_thread_id = ?1",
            params![thread_id],
        )?;
        conn.execute(
            "DELETE FROM comment_threads WHERE id = ?1",
            params![thread_id],
        )?;
    } else if github_left == 0 {
        conn.execute(
            "UPDATE comment_threads SET origin = 'local' WHERE id = ?1",
            params![thread_id],
        )?;
    }
    Ok(())
}

/// `mark_draft_posted` on `conn`; see there.
fn mark_draft_posted_on(
    conn: &Connection,
    message_id: &str,
    comment: &crate::discussion::ReviewComment,
    set_thread_github_id: bool,
) -> Result<(), CacheError> {
    let thread_id: Option<String> = conn
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
    conn.execute(
        "UPDATE comment_messages
         SET github_id = ?2, node_id = ?3, html_url = ?4, diff_hunk = ?5
         WHERE id = ?1",
        params![
            message_id,
            comment.id.to_string(),
            Some(comment.node_id.as_str()).filter(|id| !id.is_empty()),
            Some(comment.html_url.as_str()).filter(|url| !url.is_empty()),
            Some(comment.diff_hunk.as_str()).filter(|hunk| !hunk.is_empty()),
        ],
    )?;
    if set_thread_github_id {
        conn.execute(
            "UPDATE comment_threads SET origin = 'github', github_id = ?2
             WHERE id = ?1 AND github_id IS NULL",
            params![thread_id, comment.id.to_string()],
        )?;
    }
    Ok(())
}

/// Whether a comment GitHub lists is at `anchor`: the same file, side and
/// lines. GitHub names no start line for a comment on one line.
fn comment_at(comment: &crate::discussion::ReviewComment, anchor: &Anchor) -> bool {
    let Some(line) = comment.line else {
        return false;
    };
    comment.path == anchor.path
        && comment.side == anchor.side
        && line == u64::from(anchor.line)
        && comment.start_line.unwrap_or(line) == u64::from(anchor.start_line)
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
        self.replace_threads(threads, last_modified, false)
    }

    /// The same for a poll's whole list from GitHub, which also refreshes
    /// the Done list: each Done mark keeps its thread as GitHub last sent
    /// it, and a thread GitHub no longer lists leaves the Done list while
    /// its mark stays.
    pub fn store_notification_snapshot(
        &self,
        threads: &[Thread],
        last_modified: Option<&str>,
    ) -> Result<Vec<Thread>, CacheError> {
        self.replace_threads(threads, last_modified, true)
    }

    fn replace_threads(
        &self,
        threads: &[Thread],
        last_modified: Option<&str>,
        snapshot: bool,
    ) -> Result<Vec<Thread>, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let marks = notification_marks(&tx)?;
        let now = Timestamp::now();
        if snapshot {
            tx.execute("UPDATE notification_dismissals SET json = NULL", [])?;
        }
        let mut visible = Vec::with_capacity(threads.len());
        for thread in threads {
            match marks.get(&thread.id) {
                Some(mark) if mark.hides(thread, now) => {
                    if snapshot {
                        tx.execute(
                            "UPDATE notification_dismissals SET json=?2 WHERE thread_id=?1",
                            params![thread.id, serde_json::to_string(thread)?],
                        )?;
                    }
                }
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
    /// Perch, and the thread, for the Done list. The mark deliberately
    /// outlives the replaceable thread snapshot.
    pub fn dismiss_notification(&self, thread: &Thread) -> Result<(), CacheError> {
        self.mark_notification(thread, None)
    }

    /// Hide this revision of the thread until `until`, as a Done mark
    /// would, and keep the thread for Show › Snoozed. Newer activity that
    /// GitHub reports unread lifts the snooze early.
    pub fn snooze_notification(&self, thread: &Thread, until: Timestamp) -> Result<(), CacheError> {
        self.mark_notification(thread, Some(until))
    }

    fn mark_notification(
        &self,
        thread: &Thread,
        snoozed_until: Option<Timestamp>,
    ) -> Result<(), CacheError> {
        let (thread_id, updated_at) = (thread.id.as_str(), thread.updated_at);
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
        let mut kept = thread.clone();
        kept.snoozed_until = None;
        tx.execute(
            "INSERT INTO notification_dismissals (thread_id, updated_at, json, snoozed_until)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(thread_id) DO UPDATE
                 SET updated_at=excluded.updated_at, json=excluded.json,
                     snoozed_until=excluded.snoozed_until",
            params![
                thread_id,
                dismissed_at.to_string(),
                serde_json::to_string(&kept)?,
                snoozed_until.map(|until| until.to_string()),
            ],
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
        let marks = self.notification_dismissals()?;
        let now = Timestamp::now();
        threads.retain(|thread| {
            marks
                .get(&thread.id)
                .is_none_or(|mark| !mark.hides(thread, now))
        });
        Ok(threads)
    }

    /// The threads marked Done that GitHub still lists, newest first, as
    /// its list is.
    pub fn load_done_threads(&self) -> Result<Vec<Thread>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM notification_dismissals
             WHERE json IS NOT NULL AND snoozed_until IS NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut threads: Vec<Thread> = Vec::new();
        for row in rows {
            threads.push(serde_json::from_str(&row?)?);
        }
        threads.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(threads)
    }

    /// Take a Done mark back, returning the thread it hid. `None` when
    /// there was no mark, or it kept no thread.
    pub fn restore_notification(&self, thread_id: &str) -> Result<Option<Thread>, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let json = tx
            .query_row(
                "SELECT json FROM notification_dismissals WHERE thread_id=?1",
                params![thread_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        tx.execute(
            "DELETE FROM notification_dismissals WHERE thread_id=?1",
            params![thread_id],
        )?;
        let thread = json.map(|json| serde_json::from_str(&json)).transpose()?;
        tx.commit()?;
        Ok(thread)
    }

    /// The snoozed threads GitHub still lists, each with when it comes
    /// back, soonest first.
    pub fn load_snoozed_threads(&self) -> Result<Vec<Thread>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT thread_id, json, snoozed_until FROM notification_dismissals
             WHERE json IS NOT NULL AND snoozed_until IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut threads: Vec<Thread> = Vec::new();
        for row in rows {
            let (thread_id, json, until) = row?;
            let mut thread: Thread = serde_json::from_str(&json)?;
            thread.snoozed_until = Some(parse_mark_time(&thread_id, "snoozed_until", &until)?);
            threads.push(thread);
        }
        threads.sort_by(|a, b| {
            a.snoozed_until
                .cmp(&b.snoozed_until)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        Ok(threads)
    }

    /// Lift the snoozes whose time has come by `now`, returning the
    /// threads they hid, newest first, as the inbox wants them back. Each
    /// is as read or unread as it was snoozed: nothing new happened.
    pub fn wake_snoozed_notifications(&self, now: Timestamp) -> Result<Vec<Thread>, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let due = {
            let mut stmt = tx.prepare(
                "SELECT thread_id, json, snoozed_until FROM notification_dismissals
                 WHERE snoozed_until IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut due = Vec::new();
            for row in rows {
                let (thread_id, json, until) = row?;
                if parse_mark_time(&thread_id, "snoozed_until", &until)? <= now {
                    due.push((thread_id, json));
                }
            }
            due
        };
        let mut threads: Vec<Thread> = Vec::new();
        for (thread_id, json) in due {
            tx.execute(
                "DELETE FROM notification_dismissals WHERE thread_id=?1",
                params![thread_id],
            )?;
            if let Some(json) = json {
                threads.push(serde_json::from_str(&json)?);
            }
        }
        tx.commit()?;
        threads.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(threads)
    }

    fn notification_dismissals(&self) -> Result<HashMap<String, NotificationMark>, CacheError> {
        notification_marks(&self.conn)
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
                    created_at, github_id, node_id, can_resolve
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
                r.get::<_, Option<String>>(11)?,
                r.get::<_, i64>(12)?,
            ))
        })?;

        let mut threads = Vec::new();
        for row in rows {
            let (
                id,
                origin,
                path,
                start,
                line,
                side,
                sha,
                outdated,
                resolved,
                created,
                github_id,
                node_id,
                can_resolve,
            ) = row?;
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
                node_id,
                can_resolve: can_resolve != 0,
                created_at,
                messages,
            });
        }
        Ok(threads)
    }

    fn load_messages(&self, thread_id: &str) -> Result<Vec<CommentMessage>, CacheError> {
        let mut stmt = self.conn.prepare_cached(
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

    /// A run without its steps: what it ran with and how it went.
    pub fn load_run_projection(&self, id: &str) -> Result<Option<RunRecord>, CacheError> {
        load_run_projection_on(&self.conn, id)
    }

    /// Record where a run's discussion was written and the manifest of what
    /// its prompt was shown. Either may be absent: a run with no discussion
    /// has neither, and one whose directory could not be written keeps the
    /// manifest alone.
    pub fn set_run_context(
        &self,
        run_id: &str,
        dir: Option<&str>,
        manifest: Option<&str>,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE runs SET context_dir=?2, context_manifest=?3 WHERE id=?1",
            params![run_id, dir, manifest],
        )?;
        Ok(())
    }

    /// Where a run's discussion is and what it was shown, if it recorded
    /// either. `None` for a run that is gone.
    pub fn run_context(&self, run_id: &str) -> Result<Option<RunContext>, CacheError> {
        Ok(self
            .conn
            .query_row(
                "SELECT context_dir, context_manifest FROM runs WHERE id=?1",
                params![run_id],
                |row| {
                    Ok(RunContext {
                        dir: row.get(0)?,
                        manifest: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Every discussion directory a run still points at, so a sweep leaves
    /// them alone.
    pub fn run_context_dirs(&self) -> Result<Vec<String>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT context_dir FROM runs WHERE context_dir IS NOT NULL")?;
        let dirs = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(dirs)
    }

    /// Insert a thread with its messages, in one transaction: a failure
    /// part way leaves no thread with nothing under it.
    pub fn insert_comment_thread(&self, t: &CommentThread) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO comment_threads
             (id, worktree_id, origin, github_id, path, start_line, line, side, commit_sha,
              outdated, resolved, created_at, node_id, can_resolve)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                t.node_id,
                t.can_resolve as i64,
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

    /// Rewrite what a reviewer said in a finding's thread, its time
    /// unchanged: the reader trims a finding before posting it. Only a
    /// message Perch alone has, from an agent that answered no question;
    /// an answer stays as its run left it.
    pub fn update_reviewer_message_body(&self, id: &str, body: &str) -> Result<(), CacheError> {
        let changed = self.conn.execute(
            "UPDATE comment_messages SET body = ?2
             WHERE id = ?1 AND origin = 'local' AND author_kind = 'agent' AND run_id IS NULL",
            params![id, body],
        )?;
        if changed == 0 {
            return Err(CacheError::Corrupt(format!("no reviewer message {id}")));
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
    ///
    /// A review finding that opened its thread with the draft is
    /// dismissed with it, and its id returned: the draft was the finding,
    /// as the reader would have said it.
    pub fn delete_draft(&self, message_id: &str) -> Result<Option<String>, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let dismissed = delete_draft_on(&tx, message_id)?;
        tx.commit()?;
        Ok(dismissed)
    }

    /// Throw every draft in the item away, each as `delete_draft` would,
    /// in one transaction: Discard all leaves none or all of them. How
    /// many went, and the findings dismissed with them.
    pub fn delete_all_drafts(&self, item_id: &str) -> Result<(usize, Vec<String>), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let drafts: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT m.id FROM comment_messages m
                 JOIN comment_threads t ON t.id = m.thread_id
                 WHERE t.worktree_id = ?1 AND m.origin = 'github' AND m.github_id IS NULL
                   AND m.author_kind = 'user'",
            )?;
            let rows = stmt.query_map(params![item_id], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut dismissed = Vec::new();
        for id in &drafts {
            dismissed.extend(delete_draft_on(&tx, id)?);
        }
        tx.commit()?;
        Ok((drafts.len(), dismissed))
    }

    /// The drafts in an item, threads in the order they were made and a
    /// thread's drafts in the order they were written: the order they
    /// post in.
    pub fn load_drafts(&self, item_id: &str) -> Result<Vec<DraftRow>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, t.id, t.github_id, t.path, t.start_line, t.line, t.side, t.commit_sha,
                    m.body, t.created_at, m.created_at
             FROM comment_messages m
             JOIN comment_threads t ON t.id = m.thread_id
             WHERE t.worktree_id = ?1 AND m.origin = 'github' AND m.github_id IS NULL
               AND m.author_kind = 'user'
             ORDER BY t.created_at, t.id, m.created_at, m.rowid",
        )?;
        let rows = stmt.query_map(params![item_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                message_id,
                thread_id,
                github_id,
                path,
                start,
                line,
                side,
                sha,
                body,
                thread_created,
                created,
            ) = row?;
            let (Ok(thread_created_at), Ok(created_at)) = (
                thread_created.parse::<Timestamp>(),
                created.parse::<Timestamp>(),
            ) else {
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
            out.push(DraftRow {
                message_id,
                thread_id,
                thread_github_id: github_id_of(github_id),
                anchor,
                body,
                thread_created_at,
                created_at,
            });
        }
        Ok(out)
    }

    /// A draft is on GitHub as `comment`: it keeps its id and its author,
    /// the reader, and gains the comment's ids and place. A draft that
    /// opened its thread makes the thread point at the comment as its
    /// root, `set_thread_github_id`, so a refresh finds the same row; a
    /// thread already pointing at a root keeps it. One transaction, so a
    /// thread never points at a comment its draft does not.
    pub fn mark_draft_posted(
        &self,
        message_id: &str,
        comment: &crate::discussion::ReviewComment,
        set_thread_github_id: bool,
    ) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        mark_draft_posted_on(&tx, message_id, comment, set_thread_github_id)?;
        tx.commit()?;
        Ok(())
    }

    /// A reply posted under its thread on its own, as `comment`: `m` goes
    /// in as a draft and is marked posted in one transaction, so it is
    /// never a draft that would post again.
    pub fn insert_posted_reply(
        &self,
        m: &CommentMessage,
        comment: &crate::discussion::ReviewComment,
    ) -> Result<(), CacheError> {
        if !m.is_draft() {
            return Err(CacheError::Corrupt(format!(
                "comment message {} is not a draft",
                m.id
            )));
        }
        let tx = self.conn.unchecked_transaction()?;
        insert_comment_message_on(&tx, m)?;
        mark_draft_posted_on(&tx, &m.id, comment, false)?;
        tx.commit()?;
        Ok(())
    }

    /// Find the drafts that rode in review `review_id` among the comments
    /// GitHub lists for it, and mark each found one posted. A draft matches
    /// a comment of the review that opens a thread at its anchor: same
    /// path, side, line and start line; with more than one there, the one
    /// with the same text, and none when no text is the same, since a
    /// guess would put another comment's ids on the draft; and a comment
    /// is claimed once. A draft with no comment to claim stays a draft and
    /// is reported, never dropped. Only `message_ids` are looked for: the
    /// drafts the review carried.
    pub fn reconcile_review_drafts(
        &self,
        item_id: &str,
        review_id: u64,
        discussion: &Discussion,
        message_ids: &[String],
    ) -> Result<Reconciled, CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        let drafts: Vec<DraftRow> = self
            .load_drafts(item_id)?
            .into_iter()
            .filter(|d| message_ids.contains(&d.message_id))
            .collect();
        let candidates: Vec<&crate::discussion::ReviewComment> = discussion
            .review_comments
            .iter()
            .filter(|c| c.pull_request_review_id == Some(review_id) && c.in_reply_to_id.is_none())
            .collect();
        let mut claimed: HashSet<u64> = HashSet::new();
        let mut reconciled = Reconciled::default();
        for draft in drafts {
            let Some(anchor) = &draft.anchor else {
                reconciled.unmatched.push(draft.message_id);
                continue;
            };
            let at_anchor: Vec<&&crate::discussion::ReviewComment> = candidates
                .iter()
                .filter(|c| !claimed.contains(&c.id) && comment_at(c, anchor))
                .collect();
            let found = if at_anchor.len() > 1 {
                at_anchor
                    .iter()
                    .find(|c| c.body.trim() == draft.body.trim())
                    .copied()
            } else {
                at_anchor.first().copied()
            };
            match found {
                Some(comment) => {
                    claimed.insert(comment.id);
                    mark_draft_posted_on(&tx, &draft.message_id, comment, true)?;
                    reconciled.matched.push(draft.message_id);
                }
                None => reconciled.unmatched.push(draft.message_id),
            }
        }
        tx.commit()?;
        Ok(reconciled)
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

    /// Keep what GitHub answered when the reader resolved a thread or
    /// unresolved it, until the next fetch says the same.
    pub fn set_thread_resolved(&self, id: &str, resolved: bool) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE comment_threads SET resolved = ?2 WHERE id = ?1",
            params![id, resolved as i64],
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
    /// it, has nothing left, and goes too rather than draw as an empty card,
    /// unless a review finding owns it.
    pub fn delete_private_messages(&self, thread_id: &str) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE owner_thread_id=?1)",
            params![thread_id],
        )?;
        tx.execute(
            "DELETE FROM runs WHERE owner_thread_id = ?1",
            params![thread_id],
        )?;
        tx.execute(
            "DELETE FROM comment_messages WHERE thread_id = ?1 AND origin = 'local'",
            params![thread_id],
        )?;
        tx.execute(
            "DELETE FROM comment_threads WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM comment_messages WHERE thread_id = ?1)
               AND NOT EXISTS (SELECT 1 FROM review_findings WHERE thread_id = ?1)",
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
              subject_repo, last_seen_head, source_path, divergence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            params![
                item.id, item.repo_full_name,
                c.subject_kind, c.subject_number, c.subject_json,
                c.branch, c.base_ref, c.head_sha, c.checkout_path, c.owned, c.on_disk,
                item.from_item_id, item.name,
                // Saved folded into Pin; the column stays, always 0.
                0_i64,
                item.pinned_at.map(|t| t.to_string()),
                item.archived_at.map(|t| t.to_string()),
                item.created_at.to_string(),
                item.last_opened_at.to_string(),
                c.additions,
                c.deletions,
                c.subject_repo,
                c.last_seen_head,
                c.source_path,
                c.divergence,
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
                additions = ?18, deletions = ?19, subject_repo = ?20, last_seen_head = ?21,
                source_path = ?22, divergence = ?23
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
                0_i64,
                item.pinned_at.map(|t| t.to_string()),
                item.archived_at.map(|t| t.to_string()),
                item.last_opened_at.to_string(),
                c.additions,
                c.deletions,
                c.subject_repo,
                c.last_seen_head,
                c.source_path,
                c.divergence,
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

    /// Keep a discussion Perch changed by its own hand, a comment it just
    /// posted put at the end or a reaction it just left, without treating
    /// it as what GitHub lists: the comment threads are left alone. The
    /// kept copy can be behind GitHub, a reply the reader posted after the
    /// last fetch marked posted on its row but not in the copy, and a
    /// refresh from that copy would delete the row as gone from GitHub.
    pub fn rewrite_discussion(
        &self,
        item_id: &str,
        discussion: &Discussion,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO discussions (item_id, fetched_at, json) VALUES (?1, ?2, ?3)",
            params![
                item_id,
                Timestamp::now().to_string(),
                serde_json::to_string(discussion)?
            ],
        )?;
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

    /// A repository's grants, oldest first.
    pub fn grants_for(&self, repo: &str) -> Result<Vec<WorkGrantRecord>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo, key, label, created_at FROM work_grants
             WHERE repo = ?1 COLLATE NOCASE ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(params![repo], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut grants = Vec::new();
        for row in rows {
            let (id, repo, key, label, created_at) = row?;
            grants.push(WorkGrantRecord {
                id,
                repo,
                key,
                label,
                created_at: timestamp(created_at, "grant created_at")?,
            });
        }
        Ok(grants)
    }

    /// Record the latest thing that happened on an item. It replaces the
    /// one before, and is unread and visible again: Mark Unread is for the
    /// event it was given on, and a Done or a read stays with its time.
    pub fn record_item_activity(
        &self,
        item_id: &str,
        at: Timestamp,
        event: &ActivityEvent,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT INTO item_activity (item_id, at, event_json, local_unread)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(item_id) DO UPDATE SET
                at = excluded.at, event_json = excluded.event_json, local_unread = 0",
            params![item_id, at.to_string(), serde_json::to_string(event)?],
        )?;
        Ok(())
    }

    /// What the reader said about running an item's code in a language
    /// server: `Some(true)` once trusted, `Some(false)` for a branch cut
    /// from code the reader hadn't trusted, `None` when nothing was said.
    pub fn item_code_trust(&self, item_id: &str) -> Result<Option<bool>, CacheError> {
        Ok(self
            .conn
            .query_row(
                "SELECT trusted FROM item_lsp_trust WHERE item_id = ?1",
                params![item_id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?)
    }

    /// Trust an item's code from now on, whatever was said before.
    pub fn trust_item_code(&self, item_id: &str) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT INTO item_lsp_trust (item_id, trusted) VALUES (?1, 1)
             ON CONFLICT(item_id) DO UPDATE SET trusted = 1",
            params![item_id],
        )?;
        Ok(())
    }

    /// Whether the reader trusts the language server named `server` on
    /// everything in `repo`.
    pub fn repo_server_trusted(&self, repo: &str, server: &str) -> Result<bool, CacheError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM repo_lsp_trust WHERE repo = ?1 AND server = ?2)",
            params![repo, server],
            |r| r.get(0),
        )?)
    }

    /// Every repository and server the reader trusts on everything there,
    /// as `(repository, server)` in the spelling first written.
    pub fn repo_server_trust(&self) -> Result<Vec<(String, String)>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT repo, server FROM repo_lsp_trust ORDER BY repo, server")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Trust the language server named `server` on everything in `repo`
    /// from now on, or stop.
    pub fn set_repo_server_trust(
        &self,
        repo: &str,
        server: &str,
        trusted: bool,
    ) -> Result<(), CacheError> {
        if trusted {
            self.conn.execute(
                "INSERT OR IGNORE INTO repo_lsp_trust (repo, server) VALUES (?1, ?2)",
                params![repo, server],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM repo_lsp_trust WHERE repo = ?1 AND server = ?2",
                params![repo, server],
            )?;
        }
        Ok(())
    }

    /// Insert a branch cut from another item, `untrusted` when it holds
    /// code the reader hasn't trusted: in one transaction, so the row is
    /// never there without what it starts out with.
    pub fn insert_cut_item(&self, item: &Item, untrusted: bool) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        self.insert_item(item)?;
        if untrusted {
            tx.execute(
                "INSERT INTO item_lsp_trust (item_id, trusted) VALUES (?1, 0)
                 ON CONFLICT(item_id) DO NOTHING",
                params![item.id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Every item's latest activity. A record whose event no longer reads
    /// is passed over.
    pub fn load_item_activity(&self) -> Result<Vec<ItemActivity>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT item_id, at, event_json, read_at, done_at, local_unread FROM item_activity",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (item_id, at, event, read_at, done_at, local_unread) = row?;
            let (Ok(at), Ok(event)) = (at.parse(), serde_json::from_str(&event)) else {
                continue;
            };
            out.push(ItemActivity {
                item_id,
                at,
                event,
                read_at: stamp(read_at),
                done_at: stamp(done_at),
                local_unread: local_unread != 0,
            });
        }
        Ok(out)
    }

    pub fn item_activity(&self, item_id: &str) -> Result<Option<ItemActivity>, CacheError> {
        Ok(self
            .load_item_activity()?
            .into_iter()
            .find(|a| a.item_id == item_id))
    }

    /// The reader opened the item: its current event is read. Whether
    /// that changed anything.
    pub fn read_item_activity(&self, item_id: &str) -> Result<bool, CacheError> {
        let Some(activity) = self.item_activity(item_id)? else {
            return Ok(false);
        };
        if !activity.is_unread() {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE item_activity SET read_at = at, local_unread = 0 WHERE item_id = ?1",
            params![item_id],
        )?;
        Ok(true)
    }

    /// Done: the current event is put away until a newer one.
    pub fn done_item_activity(&self, item_id: &str) -> Result<bool, CacheError> {
        Ok(self.conn.execute(
            "UPDATE item_activity SET done_at = at WHERE item_id = ?1",
            params![item_id],
        )? > 0)
    }

    /// Mark Unread: the current event is unread again, until the item is
    /// opened.
    pub fn mark_item_activity_unread(&self, item_id: &str) -> Result<bool, CacheError> {
        Ok(self.conn.execute(
            "UPDATE item_activity SET local_unread = 1 WHERE item_id = ?1",
            params![item_id],
        )? > 0)
    }

    /// Keep a grant. One the repository already has is left as it is.
    pub fn insert_grant(&self, grant: &WorkGrantRecord) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO work_grants (id, repo, key, label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                grant.id,
                grant.repo,
                grant.key,
                grant.label,
                grant.created_at.to_string()
            ],
        )?;
        Ok(())
    }

    /// Take a grant back. Returns the repository it was on, when it was.
    pub fn delete_grant(&self, id: &str) -> Result<Option<String>, CacheError> {
        let repo: Option<String> = self
            .conn
            .query_row(
                "SELECT repo FROM work_grants WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        self.conn
            .execute("DELETE FROM work_grants WHERE id = ?1", params![id])?;
        Ok(repo)
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

    /// One meta value, read without opening the cache: no migration runs
    /// and no file is made. For what the UI needs before the backend has
    /// the database open.
    pub fn peek_meta(db_path: &Path, key: &str) -> Option<String> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()
        .ok()?
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
    pinned_at, archived_at, created_at, last_opened_at, additions, deletions, last_seen_head,
    source_path, divergence";

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
    source_path: Option<String>,
    divergence: Option<String>,
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
            source_path: c
                .and_then(|c| c.source.as_ref())
                .map(|p| p.to_string_lossy().into_owned()),
            divergence: match c.and_then(|c| c.divergence.as_ref()) {
                Some(d) => Some(serde_json::to_string(d)?),
                None => None,
            },
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
        source: r
            .get::<_, Option<String>>(21)
            .ok()
            .flatten()
            .map(PathBuf::from),
        // A count an older build wrote in another shape is counted again.
        divergence: r
            .get::<_, Option<String>>(22)
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok()),
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
            snoozed_until: None,
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
                    note: false,
                    pending: false,
                    user: false,
                },
                RunStep {
                    summary: "Asked to edit src/x.rs".into(),
                    ok: false,
                    note: false,
                    pending: false,
                    user: false,
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
            mode: crate::comments::RunMode::Ask,
            work_before: None,
            work_after: None,
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
            node_id: None,
            can_resolve: false,
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
            note: false,
            pending: false,
            user: false,
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
    fn updating_a_run_read_back_does_not_repeat_its_tool_calls() {
        let c = Cache::open_in_memory().unwrap();
        let mut running = run("work-r1", "thread", RunStatus::Running);
        running.steps.clear();
        c.insert_run(&running).unwrap();
        for (n, at) in ["2026-09-18T10:00:01Z", "2026-09-18T10:00:02Z"]
            .into_iter()
            .enumerate()
        {
            c.append_run_event(
                &running.id,
                &event(
                    &format!("call-{n}"),
                    at,
                    RunEvent::ToolCall {
                        id: format!("t{n}"),
                        name: "Read".into(),
                        summary: format!("Read f{n}"),
                        parent_id: None,
                    },
                ),
            )
            .unwrap();
        }
        let loaded = c.load_run(&running.id).unwrap().unwrap();
        assert_eq!(loaded.steps.len(), 2);

        // As record_work does at a Work run's start and end.
        c.update_run(&loaded).unwrap();
        c.update_run(&c.load_run(&running.id).unwrap().unwrap())
            .unwrap();
        let again = c.load_run(&running.id).unwrap().unwrap();
        assert_eq!(again.steps, loaded.steps);
        assert_eq!(again.last_event_sequence, loaded.last_event_sequence);
    }

    #[test]
    fn schema_36_drops_only_the_legacy_steps_events_repeat() {
        let c = Cache::open_in_memory().unwrap();
        // One run from before events, with legacy steps of its own, and
        // one with a tool call that an update copied as a legacy step.
        let mut old = run("old-r1", "thread", RunStatus::Completed);
        old.steps = vec![RunStep {
            summary: "Ran git log".into(),
            ok: true,
            note: false,
            pending: false,
            user: false,
        }];
        c.insert_run(&old).unwrap();
        let mut new = run("new-r1", "thread", RunStatus::Running);
        new.steps.clear();
        c.insert_run(&new).unwrap();
        c.append_run_event(
            &new.id,
            &event(
                "call",
                "2026-09-18T10:00:01Z",
                RunEvent::ToolCall {
                    id: "t1".into(),
                    name: "Read".into(),
                    summary: "Read f".into(),
                    parent_id: None,
                },
            ),
        )
        .unwrap();
        insert_event_row(
            &c.conn,
            &RunEventRecord {
                run_id: new.id.clone(),
                sequence: 2,
                event_id: "compat-step-2-0".into(),
                occurred_at: "2026-09-18T10:00:02Z".parse().unwrap(),
                event: RunEvent::LegacyStep {
                    summary: "Read f".into(),
                    ok: true,
                },
                truncated: false,
            },
        )
        .unwrap();
        assert_eq!(c.load_run("new-r1").unwrap().unwrap().steps.len(), 2);

        let tx = c.conn.unchecked_transaction().unwrap();
        drop_repeated_legacy_steps(&tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(c.load_run("new-r1").unwrap().unwrap().steps.len(), 1);
        assert_eq!(c.load_run("old-r1").unwrap().unwrap().steps, old.steps);
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
    fn text_between_tool_calls_loads_as_notes_in_order() {
        let c = Cache::open_in_memory().unwrap();
        let mut running = run("narrated", "thread", RunStatus::Running);
        running.steps.clear();
        c.insert_run(&running).unwrap();
        let events = [
            RunEvent::Text {
                text: "Looking at the log first.\n".into(),
            },
            RunEvent::ToolCall {
                id: "t1".into(),
                name: "Bash".into(),
                summary: "Ran git log".into(),
                parent_id: None,
            },
            RunEvent::ToolResult {
                id: "t1".into(),
                ok: true,
                summary: String::new(),
            },
            RunEvent::Text { text: "  ".into() },
            RunEvent::Text {
                text: "It came in last week.".into(),
            },
        ];
        for (i, e) in events.into_iter().enumerate() {
            c.append_run_event(
                &running.id,
                &event(&format!("e{i}"), "2026-09-18T10:00:01Z", e),
            )
            .unwrap();
        }

        let steps = c.load_run(&running.id).unwrap().unwrap().steps;
        let got: Vec<_> = steps.iter().map(|s| (s.summary.as_str(), s.note)).collect();
        assert_eq!(
            got,
            [
                ("Looking at the log first.", true),
                ("Ran git log", false),
                ("It came in last week.", true),
            ]
        );
    }

    #[test]
    fn a_steer_waits_at_the_end_until_read_and_then_stays_where_it_landed() {
        let c = Cache::open_in_memory().unwrap();
        let mut running = run("steered", "thread", RunStatus::Running);
        running.steps.clear();
        c.insert_run(&running).unwrap();
        let call = |id: &str| RunEvent::ToolCall {
            id: id.into(),
            name: "Bash".into(),
            summary: format!("Ran {id}"),
            parent_id: None,
        };
        let steer = |text: &str| RunEvent::Steer { text: text.into() };
        let landed = |text: &str| RunEvent::SteerLanded { text: text.into() };
        let steps = |c: &Cache| -> Vec<(String, bool, bool)> {
            c.load_run("steered")
                .unwrap()
                .unwrap()
                .steps
                .into_iter()
                .map(|s| (s.summary, s.user, s.pending))
                .collect()
        };
        let step = |summary: &str, user: bool, pending: bool| (summary.to_string(), user, pending);
        let events = [
            call("a"),
            steer("Skip the tests"),
            steer("And the docs"),
            call("b"),
        ];
        for (i, e) in events.into_iter().enumerate() {
            c.append_run_event(
                "steered",
                &event(&format!("e{i}"), "2026-09-18T10:00:01Z", e),
            )
            .unwrap();
        }
        assert_eq!(
            steps(&c),
            [
                step("Ran a", false, true),
                step("Ran b", false, true),
                step("Skip the tests", true, true),
                step("And the docs", true, true),
            ]
        );
        c.append_run_event(
            "steered",
            &event("e4", "2026-09-18T10:00:02Z", landed("Skip the tests")),
        )
        .unwrap();
        c.append_run_event("steered", &event("e5", "2026-09-18T10:00:02Z", call("c")))
            .unwrap();
        assert_eq!(
            steps(&c),
            [
                step("Ran a", false, true),
                step("Ran b", false, true),
                step("Skip the tests", true, false),
                step("Ran c", false, true),
                step("And the docs", true, true),
            ]
        );
        assert_eq!(
            c.load_run("steered")
                .unwrap()
                .unwrap()
                .latest_activity
                .as_deref(),
            Some("Ran c")
        );
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
            divergence: None,
            source: None,
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
                ok: true,
                note: false,
                pending: false,
                user: false,
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
            let (c, outcome) = Cache::open_with_outcome(&path, None).unwrap();
            assert_eq!(outcome, CacheOpen::Created, "a new file is not a reset");
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
        let (c, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(outcome, CacheOpen::Current, "a current database is kept");
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
            .dismiss_notification(&notification_thread("19", "2026-09-19T12:00:00Z"))
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
            divergence: None,
            source: None,
        });
        i.name = Some("the fence".into());
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
    fn pinned_and_deletion() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut cut = branch_item("perch/pr-7-2");
        cut.from_item_id = Some(pr_item(7).id);
        c.insert_item(&cut).unwrap();
        // A pin is a column of the row, written the way every other edit is.
        let mut marked = pr_item(7);
        marked.pinned_at = Some(stamp_of("2026-09-18T11:00:00Z"));
        c.update_item(&marked).unwrap();
        let i = c.load_item(&pr_item(7).id).unwrap().unwrap();
        assert!(i.is_pinned());

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

    #[test]
    fn grants_are_kept_per_repository_once_and_taken_back() {
        let c = Cache::open_in_memory().unwrap();
        let grant = |id: &str, repo: &str, key: &str| WorkGrantRecord {
            id: id.into(),
            repo: repo.into(),
            key: key.into(),
            label: key.into(),
            created_at: "2026-09-22T10:00:00Z".parse().unwrap(),
        };
        c.insert_grant(&grant("g1", "local/notes", "network:example.com"))
            .unwrap();
        c.insert_grant(&grant("g2", "LOCAL/notes", "network:example.com"))
            .unwrap();
        c.insert_grant(&grant("g3", "local/notes", "commit"))
            .unwrap();
        c.insert_grant(&grant("g4", "acme/widgets", "commit"))
            .unwrap();
        let keys: Vec<String> = c
            .grants_for("Local/Notes")
            .unwrap()
            .into_iter()
            .map(|g| g.key)
            .collect();
        assert_eq!(
            keys,
            ["network:example.com", "commit"],
            "one per key, whatever the case"
        );
        assert_eq!(
            c.delete_grant("g1").unwrap().as_deref(),
            Some("local/notes")
        );
        assert_eq!(c.grants_for("local/notes").unwrap().len(), 1);
        assert_eq!(c.delete_grant("gone").unwrap(), None);
    }

    #[test]
    fn schema_26_gains_the_grants_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.conn.execute_batch("DROP TABLE work_grants").unwrap();
            cache.set_meta("schema_version", "26").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 26,
                to: SCHEMA_VERSION
            }
        );
        assert!(cache.grants_for("a/b").unwrap().is_empty());
    }

    #[test]
    fn a_checkout_remembers_its_divergence_count() {
        let c = Cache::open_in_memory().unwrap();
        let mut i = branch_item("feature");
        c.insert_item(&i).unwrap();
        assert_eq!(
            c.load_item(&i.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .divergence,
            None
        );

        let counted = crate::items::Divergence {
            head: "abc".into(),
            pr_head: "def".into(),
            ahead: 3,
            behind: 2,
        };
        i.checkout.as_mut().unwrap().divergence = Some(counted.clone());
        c.update_item(&i).unwrap();
        assert_eq!(
            c.load_item(&i.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .divergence,
            Some(counted)
        );
    }

    #[test]
    fn schema_27_gains_the_divergence_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = branch_item("feature");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache
                .conn
                .execute_batch("ALTER TABLE items DROP COLUMN divergence")
                .unwrap();
            cache.set_meta("schema_version", "27").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 27,
                to: SCHEMA_VERSION
            }
        );
        cache.insert_item(&item).unwrap();
        assert_eq!(
            cache
                .load_item(&item.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .divergence,
            None
        );
    }

    #[test]
    fn item_activity_is_read_put_away_and_brought_back_by_a_newer_event() {
        let c = Cache::open_in_memory().unwrap();
        let item = branch_item("feature");
        c.insert_item(&item).unwrap();
        let moved = |group: &str| ActivityEvent::Review {
            group_id: group.into(),
            outcome: crate::activity::ActivityOutcome::Completed,
            findings: 1,
        };
        c.record_item_activity(&item.id, stamp_of("2026-09-23T10:00:00Z"), &moved("b1"))
            .unwrap();
        let a = c.item_activity(&item.id).unwrap().unwrap();
        assert!(a.is_unread() && a.is_visible());

        assert!(c.read_item_activity(&item.id).unwrap());
        assert!(!c.read_item_activity(&item.id).unwrap(), "already read");
        assert!(!c.item_activity(&item.id).unwrap().unwrap().is_unread());
        assert!(c.mark_item_activity_unread(&item.id).unwrap());
        assert!(c.item_activity(&item.id).unwrap().unwrap().is_unread());
        assert!(c.done_item_activity(&item.id).unwrap());
        assert!(!c.item_activity(&item.id).unwrap().unwrap().is_visible());

        // A newer event replaces the old one and is unread and visible.
        c.record_item_activity(&item.id, stamp_of("2026-09-23T11:00:00Z"), &moved("b2"))
            .unwrap();
        let a = c.item_activity(&item.id).unwrap().unwrap();
        assert_eq!(a.event, moved("b2"));
        assert!(a.is_unread() && a.is_visible());
        assert_eq!(c.load_item_activity().unwrap().len(), 1);

        // Nothing is left behind by the item.
        c.delete_item(&item.id).unwrap();
        assert!(c.load_item_activity().unwrap().is_empty());
        assert!(!c.done_item_activity(&item.id).unwrap());
    }

    #[test]
    fn schema_28_gains_an_empty_activity_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = branch_item("feature");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache
                .conn
                .execute_batch("DROP TABLE item_activity")
                .unwrap();
            cache.set_meta("schema_version", "28").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 28,
                to: SCHEMA_VERSION
            }
        );
        assert!(
            cache.load_item_activity().unwrap().is_empty(),
            "nothing is backfilled"
        );
    }

    #[test]
    fn schema_37_clears_the_activity_left_from_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = branch_item("feature");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache
                .conn
                .execute(
                    "INSERT INTO item_activity (item_id, at, event_json) VALUES (?1, ?2, ?3)",
                    params![
                        item.id,
                        "2026-09-27T18:25:05Z",
                        r#"{"kind":"head_moved","from":"a","to":"b","commits":1}"#
                    ],
                )
                .unwrap();
            cache.set_meta("schema_version", "36").unwrap();
        }
        let (cache, _) = Cache::open_with_outcome(&path, None).unwrap();
        let rows: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM item_activity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        assert!(
            cache.load_item(&item.id).unwrap().is_some(),
            "the item stays"
        );
    }

    #[test]
    fn an_item_s_code_trust_is_kept_and_goes_with_the_item() {
        let c = Cache::open_in_memory().unwrap();
        let item = branch_item("feature");
        c.insert_item(&item).unwrap();
        assert_eq!(c.item_code_trust(&item.id).unwrap(), None);

        c.trust_item_code(&item.id).unwrap();
        c.trust_item_code(&item.id).unwrap();
        assert_eq!(c.item_code_trust(&item.id).unwrap(), Some(true));

        // A whole-row write from a copy read before the trust leaves it,
        // and so does archiving.
        let mut stale = item.clone();
        stale.archived_at = Some(stamp_of("2026-09-24T10:00:00Z"));
        c.update_item(&stale).unwrap();
        assert_eq!(c.item_code_trust(&item.id).unwrap(), Some(true));

        c.delete_item(&item.id).unwrap();
        assert_eq!(c.item_code_trust(&item.id).unwrap(), None);
        assert!(
            c.trust_item_code("no-such-item").is_err(),
            "no row for no item"
        );
    }

    #[test]
    fn a_cut_branch_starts_with_its_answer_or_not_at_all() {
        let c = Cache::open_in_memory().unwrap();
        let a = branch_item("a");
        c.insert_cut_item(&a, true).unwrap();
        assert_eq!(c.item_code_trust(&a.id).unwrap(), Some(false));
        let b = branch_item("b");
        c.insert_cut_item(&b, false).unwrap();
        assert_eq!(c.item_code_trust(&b.id).unwrap(), None);
        c.trust_item_code(&a.id).unwrap();
        assert_eq!(c.item_code_trust(&a.id).unwrap(), Some(true));

        // A branch already taken refuses the row and leaves no answer.
        let mut again = branch_item("a");
        again.id = "item-a-again".into();
        assert!(c.insert_cut_item(&again, true).is_err());
        assert!(c.load_item(&again.id).unwrap().is_none());
        assert_eq!(c.item_code_trust(&again.id).unwrap(), None);

        // An answer that can't be written takes the row back with it.
        c.conn.execute_batch("DROP TABLE item_lsp_trust").unwrap();
        let d = branch_item("d");
        assert!(c.insert_cut_item(&d, true).is_err());
        assert!(c.load_item(&d.id).unwrap().is_none());
    }

    #[test]
    fn a_server_is_trusted_on_a_repository_one_server_at_a_time() {
        let c = Cache::open_in_memory().unwrap();
        let rust = |repo: &str| c.repo_server_trusted(repo, "rust-analyzer").unwrap();
        assert!(!rust("slatedb/slatedb"));

        c.set_repo_server_trust("slatedb/slatedb", "rust-analyzer", true)
            .unwrap();
        c.set_repo_server_trust("SlateDB/SlateDB", "rust-analyzer", true)
            .unwrap();
        assert!(rust("SLATEDB/slatedb"), "in any case");
        assert!(!rust("slatedb/other"));
        assert!(
            !c.repo_server_trusted("slatedb/slatedb", "jdtls").unwrap(),
            "one server"
        );
        assert_eq!(
            c.repo_server_trust().unwrap(),
            [("slatedb/slatedb".to_string(), "rust-analyzer".to_string())],
            "once, as first written"
        );

        c.set_repo_server_trust("slatedb/slatedb", "jdtls", true)
            .unwrap();
        c.set_repo_server_trust("SlateDB/slatedb", "rust-analyzer", false)
            .unwrap();
        assert!(!rust("slatedb/slatedb"));
        assert_eq!(
            c.repo_server_trust().unwrap(),
            [("slatedb/slatedb".to_string(), "jdtls".to_string())]
        );
    }

    #[test]
    fn schema_34_gains_a_table_of_servers_trusted_on_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache
                .conn
                .execute_batch("DROP TABLE repo_lsp_trust")
                .unwrap();
            cache.set_meta("schema_version", "34").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 34,
                to: SCHEMA_VERSION
            }
        );
        assert!(cache.repo_server_trust().unwrap().is_empty());
        cache.set_repo_server_trust("a/b", "jdtls", true).unwrap();
        assert!(cache.repo_server_trusted("a/b", "jdtls").unwrap());
    }

    #[test]
    fn schema_33_gains_a_trust_table_with_cut_branches_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let pr = pr_item(7);
        let mut work = branch_item("perch/pr-7-work");
        work.from_item_id = Some(pr.id.clone());
        let mut onward = branch_item("onward");
        onward.from_item_id = Some(work.id.clone());
        // Its pull request was deleted, which cleared the pointer.
        let orphan = branch_item("perch/pr-9-work");
        // Forked from a pull request's checkout, whose row went too.
        let forked = branch_item("perch/pr-11-2");
        // The same, with a pull request of its own since.
        let mut forked_and_opened = branch_item("perch/pr-13-2");
        forked_and_opened.subject = Some(ItemSubject::PullRequest(pull_request(14)));
        // A pull request's own checkout.
        let mut checked_out = branch_item("perch/pr-15");
        checked_out.subject = Some(ItemSubject::PullRequest(pull_request(15)));
        let feature = branch_item("feature");
        let mut from_feature = branch_item("feature-2");
        from_feature.from_item_id = Some(feature.id.clone());
        {
            let cache = Cache::open(&path, None).unwrap();
            for item in [
                &pr,
                &work,
                &onward,
                &orphan,
                &forked,
                &forked_and_opened,
                &checked_out,
                &feature,
                &from_feature,
            ] {
                cache.insert_item(item).unwrap();
            }
            cache
                .conn
                .execute_batch("DROP TABLE item_lsp_trust")
                .unwrap();
            cache.set_meta("schema_version", "33").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 33,
                to: SCHEMA_VERSION
            }
        );
        for cut in [&work, &onward, &orphan, &forked, &forked_and_opened] {
            assert_eq!(
                cache.item_code_trust(&cut.id).unwrap(),
                Some(false),
                "{}",
                cut.id
            );
        }
        for other in [&pr, &checked_out, &feature, &from_feature] {
            assert_eq!(
                cache.item_code_trust(&other.id).unwrap(),
                None,
                "{}",
                other.id
            );
        }
    }

    /// Save folded into Pin: a saved row is pinned as of its last open,
    /// one already pinned keeps its pin, and the column is left at 0.
    #[test]
    fn schema_30_pins_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let saved = pr_item(7);
        let mut both = pr_item(8);
        both.pinned_at = Some(stamp_of("2026-09-01T09:00:00Z"));
        let plain = pr_item(9);
        {
            let cache = Cache::open(&path, None).unwrap();
            for item in [&saved, &both, &plain] {
                cache.insert_item(item).unwrap();
            }
            cache
                .conn
                .execute(
                    "UPDATE items SET saved = 1 WHERE id IN (?1, ?2)",
                    params![saved.id, both.id],
                )
                .unwrap();
            cache.set_meta("schema_version", "30").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 30,
                to: SCHEMA_VERSION
            }
        );
        let pinned = |id: &str| cache.load_item(id).unwrap().unwrap().pinned_at;
        assert_eq!(pinned(&saved.id), Some(saved.last_opened_at));
        assert_eq!(pinned(&both.id), both.pinned_at, "an existing pin stays");
        assert_eq!(pinned(&plain.id), None);
        let still_saved: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM items WHERE saved != 0", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(still_saved, 0);
    }

    #[test]
    fn schema_31_keeps_its_done_marks_and_refetches_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let dismissed = notification_thread("1", "2026-09-19T12:00:00Z");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache
                .store_threads(&[], Some("Sat, 19 Sep 2026 12:00:00 GMT"))
                .unwrap();
            cache
                .conn
                .execute_batch(
                    "ALTER TABLE notification_dismissals DROP COLUMN json;
                     INSERT INTO notification_dismissals (thread_id, updated_at)
                     VALUES ('1', '2026-09-19T12:00:00Z');",
                )
                .unwrap();
            cache.set_meta("schema_version", "31").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 31,
                to: SCHEMA_VERSION
            }
        );
        assert_eq!(cache.notifications_last_modified().unwrap(), None);
        assert!(cache.load_done_threads().unwrap().is_empty());

        // The first poll finds the thread the old mark hides.
        let visible = cache
            .store_notification_snapshot(std::slice::from_ref(&dismissed), None)
            .unwrap();
        assert!(visible.is_empty());
        assert_eq!(cache.load_done_threads().unwrap(), vec![dismissed]);
    }

    #[test]
    fn schema_29_gains_the_snapshot_worktree_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache
                .conn
                .execute(
                    "ALTER TABLE review_snapshots DROP COLUMN worktree_parent_sha",
                    [],
                )
                .unwrap();
            cache.set_meta("schema_version", "29").unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 29,
                to: SCHEMA_VERSION
            }
        );
        let has_column: bool = cache
            .conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('review_snapshots')
                     WHERE name='worktree_parent_sha'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_column);
    }

    #[test]
    fn a_checkout_cut_from_a_root_remembers_the_root() {
        let c = Cache::open_in_memory().unwrap();
        let mut i = branch_item("feature");
        c.insert_item(&i).unwrap();
        assert_eq!(
            c.load_item(&i.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .source,
            None
        );

        i.checkout.as_mut().unwrap().source = Some("/Users/me/Code/notes".into());
        c.update_item(&i).unwrap();
        assert_eq!(
            c.load_item(&i.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .source,
            Some(PathBuf::from("/Users/me/Code/notes"))
        );
    }

    #[test]
    fn schema_25_gains_the_work_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = branch_item("feature");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache
                .insert_run(&run("run-25", "thread-25", RunStatus::Completed))
                .unwrap();
            for (table, column) in [
                ("items", "source_path"),
                ("runs", "mode"),
                ("runs", "work_before"),
                ("runs", "work_after"),
            ] {
                cache
                    .conn
                    .execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), [])
                    .unwrap();
            }
            cache.set_meta("schema_version", "25").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 25,
                to: SCHEMA_VERSION
            }
        );
        assert_eq!(
            cache
                .load_item(&item.id)
                .unwrap()
                .unwrap()
                .checkout
                .unwrap()
                .source,
            None
        );
        let mode: String = cache
            .conn
            .query_row("SELECT mode FROM runs WHERE id='run-25'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(mode, "ask", "a run from before was a question");
    }

    fn discussion() -> Discussion {
        use crate::discussion::{IssueComment, Review};
        use crate::timeline::{PullEvent, PullEventKind};
        Discussion {
            viewer_can_update: false,
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
                viewer_can_delete: false,
                viewer_can_update: false,
            }],
            reviews: vec![Review {
                viewer_can_update: false,
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
                PullEvent::new(
                    PullEventKind::Opened,
                    stamp_of("2026-09-14T14:02:00Z"),
                    "chris",
                ),
                PullEvent::new(
                    PullEventKind::Merged,
                    stamp_of("2026-09-17T15:00:00Z"),
                    "rohan",
                ),
            ],
            review_threads: Vec::new(),
        }
    }

    fn review_comment(
        id: u64,
        reply_to: Option<u64>,
        at: &str,
    ) -> crate::discussion::ReviewComment {
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
            viewer_can_delete: false,
            viewer_can_update: false,
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
        d.review_comments
            .push(review_comment(4, Some(3), "2026-09-16T11:39:00Z"));
        c.store_discussion("item-pr-7", &d).unwrap();

        let threads = c.load_comment_threads("item-pr-7").unwrap();
        assert_eq!(threads.len(), 1);
        let t = &threads[0];
        assert_eq!(
            (t.id.as_str(), t.origin, t.github_id),
            ("gh-3", ThreadOrigin::GitHub, Some(3))
        );
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
        assert_eq!(
            root.html_url.as_deref(),
            Some("https://github.com/acme/widgets/pull/7#discussion_r3")
        );
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
    fn a_github_thread_keeps_its_id_there_and_whether_the_reader_may_resolve_it() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut d = discussion();
        d.review_threads = vec![crate::discussion::ReviewThreadState {
            node_id: "PRRT_3".into(),
            root_comment_id: 3,
            resolved: false,
            outdated: false,
            can_resolve: true,
        }];
        c.store_discussion("item-pr-7", &d).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.node_id.as_deref(), Some("PRRT_3"));
        assert!(t.can_resolve && t.resolvable());
        assert!(!t.resolved);

        // Resolved from Perch, and kept so until a fetch says otherwise.
        c.set_thread_resolved("gh-3", true).unwrap();
        assert!(c.load_comment_threads("item-pr-7").unwrap()[0].resolved);
        d.review_threads[0].resolved = true;
        c.store_discussion("item-pr-7", &d).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert!(t.resolved && t.resolvable());

        // Gone from GitHub and kept for Perch's message under it, it is
        // resolved in Perch from here on.
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
        c.insert_comment_message(&question).unwrap();
        let mut gone = d.clone();
        gone.review_comments.clear();
        gone.review_threads.clear();
        c.store_discussion("item-pr-7", &gone).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(
            (t.id.as_str(), t.github_id, t.node_id.as_deref()),
            ("gh-3", None, None)
        );
        assert!(!t.can_resolve);
        assert!(t.resolved, "as GitHub last had it");
        assert!(t.resolvable(), "Perch's now, and resolved in Perch");
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
            author: Author::Agent {
                backend: "claude-code".into(),
                model: "sonnet".into(),
            },
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

        // Of all that, only the draft is for the review.
        let drafts = c.load_drafts("item-pr-7").unwrap();
        let ids: Vec<_> = drafts.iter().map(|d| d.message_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["d1"],
            "nothing an agent wrote, nothing asked of one"
        );

        c.delete_private_messages("gh-3").unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["gh-3", "d1"],
            "GitHub's comment and the draft stay"
        );
        assert_eq!(
            c.load_run("gh-3-r1").unwrap(),
            None,
            "the answer's run goes with it"
        );

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
        assert_eq!(
            t.origin,
            ThreadOrigin::Local,
            "nothing under it is bound for GitHub"
        );

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
        c.insert_draft(&draft("d1", "t1", "2026-09-18T10:01:00Z"))
            .unwrap();

        c.update_comment_message_body("d1", "I fixed it").unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        let d = t.messages.iter().find(|m| m.id == "d1").unwrap();
        assert_eq!(d.body, "I fixed it");
        assert_eq!(
            d.created_at,
            stamp_of("2026-09-18T10:01:00Z"),
            "its time stays"
        );

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
    fn a_findings_body_is_rewritten_and_an_answers_is_not() {
        let c = Cache::open_in_memory().unwrap();
        let mut t = comment_thread("t1", "w");
        t.messages[1].id = "finding".into();
        t.messages[1].run = None;
        c.insert_comment_thread(&t).unwrap();
        c.insert_comment_thread(&comment_thread("t2", "w")).unwrap();

        c.update_reviewer_message_body("finding", "trimmed")
            .unwrap();
        let threads = c.load_comment_threads("w").unwrap();
        let m = threads
            .iter()
            .flat_map(|t| &t.messages)
            .find(|m| m.id == "finding")
            .unwrap();
        assert_eq!(m.body, "trimmed");
        assert_eq!(
            m.created_at,
            stamp("2026-09-18T10:00:14Z"),
            "its time stays"
        );

        for id in ["t2-m2", "t1-m1", "nowhere"] {
            assert!(
                matches!(
                    c.update_reviewer_message_body(id, "no"),
                    Err(CacheError::Corrupt(_))
                ),
                "{id}"
            );
        }
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
        assert_eq!(
            c.load_comment_threads("w").unwrap()[0].origin,
            ThreadOrigin::Local
        );
        assert!(matches!(
            c.set_thread_origin("nowhere", ThreadOrigin::Local),
            Err(CacheError::Corrupt(_))
        ));
    }

    #[test]
    fn a_draft_under_perchs_thread_turns_it_towards_github() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        c.insert_draft(&draft("d1", "t1", "2026-09-18T10:01:00Z"))
            .unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(t.origin, ThreadOrigin::GitHub);
        assert_eq!(t.github_id, None);
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["t1-m1", "t1-m2", "d1"]);

        // A message that is not a draft is not what this is for.
        let mut answer = draft("d2", "t1", "2026-09-18T10:02:00Z");
        answer.origin = ThreadOrigin::Local;
        assert!(matches!(
            c.insert_draft(&answer),
            Err(CacheError::Corrupt(_))
        ));
        assert_eq!(c.load_comment_threads("w").unwrap()[0].messages.len(), 3);
    }

    #[test]
    fn deleting_a_draft_turns_its_thread_back_or_takes_it() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_comment_thread(&comment_thread("t1", "w")).unwrap();
        c.insert_draft(&draft("d1", "t1", "2026-09-18T10:01:00Z"))
            .unwrap();
        c.delete_draft("d1").unwrap();
        let t = c.load_comment_threads("w").unwrap().pop().unwrap();
        assert_eq!(
            t.origin,
            ThreadOrigin::Local,
            "only Perch's messages are left"
        );
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
        assert!(matches!(
            c.delete_draft("t1-m1"),
            Err(CacheError::Corrupt(_))
        ));
        assert_eq!(c.load_comment_threads("w").unwrap()[0].messages.len(), 2);
    }

    #[test]
    fn deleting_a_draft_under_githubs_thread_leaves_the_thread_as_it_is() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        c.insert_draft(&draft("d1", "gh-3", "2026-09-16T12:02:00Z"))
            .unwrap();
        c.delete_draft("d1").unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.origin, ThreadOrigin::GitHub);
        assert_eq!(t.github_id, Some(3));
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3"]);
    }

    /// The root is deleted on github.com and its reply is a root of its
    /// own now: the reply takes over the thread it was under, the thread
    /// keeps its id and what Perch put there, and points at the reply.
    #[test]
    fn a_reply_whose_root_left_github_takes_over_the_thread() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        let mut d = discussion();
        d.review_comments
            .push(review_comment(4, Some(3), "2026-09-16T11:39:00Z"));
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
        assert_eq!(
            threads.len(),
            1,
            "one thread, not the reply's beside the old one"
        );
        let t = &threads[0];
        assert_eq!(
            (t.id.as_str(), t.origin, t.github_id),
            ("gh-3", ThreadOrigin::GitHub, Some(4))
        );
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["gh-4", "q1"],
            "comment 3 is gone; the question stays with the reply"
        );
        assert_eq!(
            t.created_at,
            stamp_of("2026-09-16T11:38:00Z"),
            "the thread's time stays"
        );

        // A comment on GitHub whose row is a posted draft of the reader's
        // takes over the same way, and stays the reader's: one row for
        // comment 5, not the reader's under one thread and GitHub's copy
        // under another.
        c.insert_draft(&draft("d1", "gh-3", "2026-09-16T12:02:00Z"))
            .unwrap();
        let reply = crate::discussion::ReviewComment {
            in_reply_to_id: Some(4),
            user: "chris".into(),
            body: "I will fix it".into(),
            ..review_comment(5, Some(4), "2026-09-16T12:03:00Z")
        };
        c.mark_draft_posted("d1", &reply, false).unwrap();
        d.review_comments = vec![reply];
        c.store_discussion("item-pr-7", &d).unwrap();
        let threads = c.load_comment_threads("item-pr-7").unwrap();
        assert_eq!(threads.len(), 1);
        let t = &threads[0];
        assert_eq!((t.id.as_str(), t.github_id), ("gh-3", Some(5)));
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["q1", "d1"]);
        assert_eq!(t.messages[1].author, Author::User);
        assert_eq!(t.messages[1].github_id, Some(5));
    }

    /// Perch rewrites the kept discussion by its own hand, a reaction or
    /// an issue comment it just posted, and that copy can be behind
    /// GitHub: a reply the reader posted since the last fetch is on its
    /// row but not in the copy. Keeping the copy is not a refresh, so the
    /// posted reply's row stays; a refresh from GitHub is what replaces
    /// the rows.
    #[test]
    fn rewriting_the_kept_discussion_does_not_take_a_posted_reply_for_gone() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        c.insert_draft(&draft("d1", "gh-3", "2026-09-16T12:02:00Z"))
            .unwrap();
        let reply = crate::discussion::ReviewComment {
            in_reply_to_id: Some(3),
            user: "chris".into(),
            body: "I will fix it".into(),
            ..review_comment(5, Some(3), "2026-09-16T12:03:00Z")
        };
        c.mark_draft_posted("d1", &reply, false).unwrap();

        let mut kept = c.load_discussion("item-pr-7").unwrap().unwrap().1;
        kept.comments[0]
            .reactions
            .toggle(crate::reactions::ReactionKind::Rocket, true);
        c.rewrite_discussion("item-pr-7", &kept).unwrap();
        assert_eq!(
            c.load_discussion("item-pr-7").unwrap().unwrap().1,
            kept,
            "the copy is kept"
        );
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["gh-3", "d1"],
            "the posted reply is not taken for gone"
        );
        assert_eq!(t.messages[1].github_id, Some(5));

        // The same copy as a refresh from GitHub would take it, as the
        // comment is not listed there.
        c.store_discussion("item-pr-7", &kept).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3"]);
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

    /// A thread of the reader's own on `item-pr-7`, on its way to GitHub
    /// with one draft, `d1`, at `src/x.rs:41–58` of `abc`.
    fn draft_thread(id: &str, at: &str) -> CommentThread {
        let mut t = comment_thread(id, "item-pr-7");
        t.origin = ThreadOrigin::GitHub;
        t.created_at = stamp_of(at);
        t.messages = vec![draft(&format!("{id}-d1"), id, at)];
        t
    }

    /// A comment GitHub made of a draft at `comment_thread`'s anchor, in
    /// review `review`.
    fn posted_comment(
        id: u64,
        review: Option<u64>,
        body: &str,
    ) -> crate::discussion::ReviewComment {
        crate::discussion::ReviewComment {
            pull_request_review_id: review,
            user: "chris".into(),
            body: body.into(),
            path: "src/x.rs".into(),
            line: Some(58),
            start_line: Some(41),
            start_side: Some(Side::Right),
            commit_id: "abc123".into(),
            reactions: Reactions::default(),
            ..review_comment(id, None, "2026-09-19T09:00:00Z")
        }
    }

    #[test]
    fn a_posted_draft_keeps_its_ids_and_author_when_github_lists_it() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.insert_comment_thread(&draft_thread("t1", "2026-09-19T08:00:00Z"))
            .unwrap();
        c.insert_comment_message(&CommentMessage {
            id: "q1".into(),
            origin: ThreadOrigin::Local,
            body: "is this right?".into(),
            created_at: stamp_of("2026-09-19T08:01:00Z"),
            ..draft("q1", "t1", "2026-09-19T08:01:00Z")
        })
        .unwrap();
        let comment = posted_comment(500, Some(9), "I will fix it");
        c.mark_draft_posted("t1-d1", &comment, true).unwrap();

        let mut d = discussion();
        d.review_comments = vec![comment.clone()];
        c.store_discussion("item-pr-7", &d).unwrap();
        let threads = c.load_comment_threads("item-pr-7").unwrap();
        assert_eq!(threads.len(), 1, "GitHub's copy is the same thread");
        let t = &threads[0];
        assert_eq!((t.id.as_str(), t.github_id), ("t1", Some(500)));
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["t1-d1", "q1"],
            "the question asked here stays under it"
        );
        let posted = &t.messages[0];
        assert_eq!(
            posted.author,
            Author::User,
            "the reader's, not a GitHub user's"
        );
        assert_eq!(posted.github_id, Some(500));
        assert!(!posted.is_draft());
        assert_eq!(posted.reactions, comment.reactions);

        // An edit on github.com reaches the same row, and a reply lands
        // under the same thread.
        d.review_comments[0].body = "I fixed it".into();
        d.review_comments.push(crate::discussion::ReviewComment {
            in_reply_to_id: Some(500),
            ..posted_comment(501, None, "thanks")
        });
        c.store_discussion("item-pr-7", &d).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.id, "t1");
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["t1-d1", "q1", "gh-501"]);
        assert_eq!(t.messages[0].body, "I fixed it");
        assert_eq!(t.messages[0].author, Author::User);

        // Gone from GitHub, the posted comment goes as any of GitHub's
        // does, by the id it had there.
        d.review_comments.clear();
        c.store_discussion("item-pr-7", &d).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["q1"]);
        assert_eq!(t.github_id, None);
    }

    #[test]
    fn a_posted_reply_is_updated_in_place_under_githubs_thread() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        c.insert_draft(&draft("d1", "gh-3", "2026-09-16T12:02:00Z"))
            .unwrap();
        let reply = crate::discussion::ReviewComment {
            in_reply_to_id: Some(3),
            user: "chris".into(),
            body: "I will fix it".into(),
            ..review_comment(88, Some(3), "2026-09-16T12:03:00Z")
        };
        c.mark_draft_posted("d1", &reply, false).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        assert_eq!(t.github_id, Some(3), "the thread's root is still GitHub's");
        assert_eq!(t.messages[1].github_id, Some(88));
        assert_eq!(t.messages[1].node_id.as_deref(), Some("PRRC_88"));

        let mut d = discussion();
        d.review_comments.push(reply);
        c.store_discussion("item-pr-7", &d).unwrap();
        let t = c.load_comment_threads("item-pr-7").unwrap().pop().unwrap();
        let ids: Vec<_> = t.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gh-3", "d1"], "not a second row, gh-88");
        assert_eq!(t.messages[1].author, Author::User);
        assert_eq!(t.messages[1].reactions, d.review_comments[1].reactions);

        // Not a draft any more: nothing marks or deletes it as one.
        assert!(matches!(
            c.mark_draft_posted("d1", &d.review_comments[1], false),
            Err(CacheError::Corrupt(_))
        ));
        assert!(matches!(c.delete_draft("d1"), Err(CacheError::Corrupt(_))));
    }

    #[test]
    fn the_drafts_load_in_posting_order_with_their_threads_roots() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        c.store_discussion("item-pr-7", &discussion()).unwrap();
        c.insert_comment_thread(&draft_thread("t2", "2026-09-19T08:00:00Z"))
            .unwrap();
        c.insert_draft(&draft("d-reply", "gh-3", "2026-09-19T09:00:00Z"))
            .unwrap();
        c.insert_draft(&draft("t2-d2", "t2", "2026-09-19T08:30:00Z"))
            .unwrap();

        let drafts = c.load_drafts("item-pr-7").unwrap();
        let ids: Vec<_> = drafts.iter().map(|d| d.message_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["d-reply", "t2-d1", "t2-d2"],
            "GitHub's thread came first"
        );
        assert_eq!(drafts[0].thread_github_id, Some(3));
        assert_eq!(drafts[0].thread_id, "gh-3");
        assert_eq!(drafts[1].thread_github_id, None);
        assert_eq!(
            drafts[1].anchor.as_ref().map(|a| (a.start_line, a.line)),
            Some((41, 58))
        );
        assert_eq!(
            drafts[1].thread_created_at,
            stamp_of("2026-09-19T08:00:00Z")
        );
        assert_eq!(drafts[2].created_at, stamp_of("2026-09-19T08:30:00Z"));
        assert!(c.load_drafts("item-pr-8").unwrap().is_empty());

        assert_eq!(c.delete_all_drafts("item-pr-7").unwrap(), (3, vec![]));
        assert!(c.load_drafts("item-pr-7").unwrap().is_empty());
        let threads = c.load_comment_threads("item-pr-7").unwrap();
        assert_eq!(
            threads.len(),
            1,
            "the thread made for its drafts went with them"
        );
        assert_eq!(threads[0].id, "gh-3");
        assert_eq!(c.delete_all_drafts("item-pr-7").unwrap(), (0, vec![]));
    }

    #[test]
    fn reconciling_matches_a_draft_by_anchor_then_text_and_keeps_the_rest() {
        let c = Cache::open_in_memory().unwrap();
        c.insert_item(&pr_item(7)).unwrap();
        // Two threads at one anchor, one at another, one nowhere in the review.
        c.insert_comment_thread(&draft_thread("t1", "2026-09-19T08:00:00Z"))
            .unwrap();
        let mut twin = draft_thread("t1b", "2026-09-19T08:05:00Z");
        twin.messages[0].body = "and also this".into();
        c.insert_comment_thread(&twin).unwrap();
        let mut elsewhere = draft_thread("t2", "2026-09-19T08:10:00Z");
        elsewhere.anchor.as_mut().unwrap().path = "src/y.rs".into();
        elsewhere.anchor.as_mut().unwrap().start_line = 7;
        elsewhere.anchor.as_mut().unwrap().line = 7;
        c.insert_comment_thread(&elsewhere).unwrap();
        let mut lost = draft_thread("t3", "2026-09-19T08:20:00Z");
        lost.anchor.as_mut().unwrap().line = 60;
        c.insert_comment_thread(&lost).unwrap();

        let mut d = discussion();
        d.review_comments = vec![
            // Same anchor as t1's two drafts: the text tells them apart.
            posted_comment(600, Some(77), "and also this"),
            posted_comment(601, Some(77), "I will fix it"),
            // t2's anchor, one line.
            crate::discussion::ReviewComment {
                path: "src/y.rs".into(),
                line: Some(7),
                start_line: None,
                start_side: None,
                ..posted_comment(602, Some(77), "I will fix it")
            },
            // Someone else's review at t3's anchor: not this review's.
            crate::discussion::ReviewComment {
                line: Some(60),
                ..posted_comment(603, Some(9), "I will fix it")
            },
        ];
        let ids = ["t1-d1", "t1b-d1", "t2-d1", "t3-d1"].map(String::from);
        let reconciled = c
            .reconcile_review_drafts("item-pr-7", 77, &d, &ids)
            .unwrap();
        assert_eq!(
            reconciled.matched,
            ["t1-d1", "t1b-d1", "t2-d1"].map(String::from)
        );
        assert_eq!(reconciled.unmatched, vec!["t3-d1".to_string()]);

        let threads = c.load_comment_threads("item-pr-7").unwrap();
        let by_id = |id: &str| threads.iter().find(|t| t.id == id).unwrap().clone();
        assert_eq!(by_id("t1").github_id, Some(601), "matched by its text");
        assert_eq!(by_id("t1").messages[0].github_id, Some(601));
        assert_eq!(by_id("t1b").github_id, Some(600));
        assert_eq!(by_id("t2").github_id, Some(602));
        assert_eq!(by_id("t3").github_id, None);
        assert!(by_id("t3").messages[0].is_draft(), "kept, never dropped");

        // Only the drafts named are looked for.
        let again = c.reconcile_review_drafts("item-pr-7", 77, &d, &[]).unwrap();
        assert_eq!(again, Reconciled::default());

        // Ingesting what GitHub said afterwards changes no ids.
        c.store_discussion("item-pr-7", &d).unwrap();
        let threads = c.load_comment_threads("item-pr-7").unwrap();
        let mut ids: Vec<_> = threads.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["gh-603", "t1", "t1b", "t2", "t3"]);
    }

    #[test]
    fn schema_20_gains_the_github_message_columns_and_the_threads_of_kept_discussions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = pr_item(20);
        let local = comment_thread("local-20", &item.id);
        let mut kept = discussion();
        kept.review_threads = vec![crate::discussion::ReviewThreadState {
            node_id: "PRRT_3".into(),
            root_comment_id: 3,
            resolved: true,
            outdated: false,
            can_resolve: false,
        }];
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache.insert_comment_thread(&local).unwrap();
            for column in GITHUB_MESSAGE_COLUMNS {
                cache
                    .conn
                    .execute(
                        &format!("ALTER TABLE comment_messages DROP COLUMN {column}"),
                        [],
                    )
                    .unwrap();
            }
            // Schema 23's, which the ingestion here writes.
            for (column, _) in GITHUB_THREAD_COLUMNS {
                cache
                    .conn
                    .execute(
                        &format!("ALTER TABLE comment_threads DROP COLUMN {column}"),
                        [],
                    )
                    .unwrap();
            }
            cache
                .conn
                .execute(
                    "INSERT INTO discussions (item_id, fetched_at, json) VALUES (?1, ?2, ?3)",
                    params![
                        item.id,
                        "2026-09-18T09:00:00Z",
                        serde_json::to_string(&kept).unwrap()
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
        assert!(threads[0].resolved);
        assert_eq!(threads[0].node_id.as_deref(), Some("PRRT_3"));
        assert_eq!(
            threads[0].anchor.as_ref().map(|a| a.commit_sha.as_str()),
            Some("abc123")
        );
        assert_eq!(threads[1].messages.len(), 2);
        assert!(cache.load_comment_threads("broken").unwrap().is_empty());
    }

    #[test]
    fn schema_21_moves_the_merges_actor_onto_the_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = pr_item(21);
        // The shape schema 21 kept: the merge named who, and nothing else
        // named anyone.
        let kept = r#"{"node_id":"PR_1","opened_at":"2026-09-14T14:02:00Z","body":"b",
            "comments":[],"reviews":[],"review_comments":[],"events":[
              {"kind":"opened","at":"2026-09-14T14:02:00Z"},
              {"kind":{"merged":{"by":"rohan"}},"at":"2026-09-17T15:00:00Z"}]}"#;
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            for (id, json) in [(item.id.as_str(), kept), ("broken", "{")] {
                cache
                    .conn
                    .execute(
                        "INSERT INTO discussions (item_id, fetched_at, json) VALUES (?1, ?2, ?3)",
                        params![id, "2026-09-18T09:00:00Z", json],
                    )
                    .unwrap();
            }
            cache.set_meta("schema_version", "21").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 21,
                to: SCHEMA_VERSION
            }
        );
        let (_, d) = cache.load_discussion(&item.id).unwrap().unwrap();
        assert_eq!(
            d.events,
            vec![
                crate::timeline::PullEvent::new(
                    crate::timeline::PullEventKind::Opened,
                    stamp_of("2026-09-14T14:02:00Z"),
                    "",
                ),
                crate::timeline::PullEvent::new(
                    crate::timeline::PullEventKind::Merged,
                    stamp_of("2026-09-17T15:00:00Z"),
                    "rohan",
                ),
            ]
        );
        assert!(cache.load_discussion("broken").unwrap().is_none());
    }

    #[test]
    fn schema_22_gains_each_threads_id_on_github_and_leaves_the_rest_to_the_next_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        let item = pr_item(22);
        let local = comment_thread("local-22", &item.id);
        {
            let cache = Cache::open(&path, None).unwrap();
            cache.insert_item(&item).unwrap();
            cache.insert_comment_thread(&local).unwrap();
            for (column, _) in GITHUB_THREAD_COLUMNS {
                cache
                    .conn
                    .execute(
                        &format!("ALTER TABLE comment_threads DROP COLUMN {column}"),
                        [],
                    )
                    .unwrap();
            }
            cache.set_meta("schema_version", "22").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 22,
                to: SCHEMA_VERSION
            }
        );
        let threads = cache.load_comment_threads(&item.id).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(
            (threads[0].node_id.as_deref(), threads[0].can_resolve),
            (None, false)
        );
        assert_eq!(
            threads[0].messages.len(),
            local.messages.len(),
            "nothing under it lost"
        );
        cache.set_thread_resolved("local-22", true).unwrap();
        assert!(cache.load_comment_threads(&item.id).unwrap()[0].resolved);
    }

    #[test]
    fn schema_23_gains_review_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache
                .conn
                .execute("ALTER TABLE review_groups DROP COLUMN instructions", [])
                .unwrap();
            cache.set_meta("schema_version", "23").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 23,
                to: SCHEMA_VERSION
            }
        );
        let has_column: bool = cache
            .conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('review_groups') WHERE name='instructions'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_column);
    }

    #[test]
    fn schema_24_gains_each_run_s_discussion_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perch.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            cache
                .insert_run(&run("run-24", "thread-24", RunStatus::Completed))
                .unwrap();
            for column in RUN_CONTEXT_COLUMNS {
                cache
                    .conn
                    .execute(&format!("ALTER TABLE runs DROP COLUMN {column}"), [])
                    .unwrap();
            }
            cache.set_meta("schema_version", "24").unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 24,
                to: SCHEMA_VERSION
            }
        );
        assert_eq!(
            cache.run_context("run-24").unwrap(),
            Some(RunContext::default())
        );
        cache
            .set_run_context("run-24", Some("/perch/context/run-24"), Some("{}"))
            .unwrap();
        assert_eq!(
            cache.run_context("run-24").unwrap(),
            Some(RunContext {
                dir: Some("/perch/context/run-24".into()),
                manifest: Some("{}".into()),
            })
        );
        assert_eq!(cache.run_context_dirs().unwrap(), ["/perch/context/run-24"]);
        assert_eq!(cache.run_context("gone").unwrap(), None);
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
            c.dismiss_notification(&dismissed).unwrap();
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
        c.dismiss_notification(&dismissed).unwrap();

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
        c.dismiss_notification(&dismissed).unwrap();

        let stale = notification_thread("1", "2026-09-19T11:00:00Z");
        c.dismiss_notification(&stale).unwrap();
        assert_eq!(
            c.notification_dismissals()
                .unwrap()
                .get(&dismissed.id)
                .map(|mark| mark.updated_at),
            Some(dismissed.updated_at)
        );

        let newer = notification_thread("1", "2026-09-20T09:00:00Z");
        c.store_threads(std::slice::from_ref(&newer), None).unwrap();
        c.dismiss_notification(&stale).unwrap();
        assert_eq!(c.load_threads().unwrap(), vec![newer]);
    }

    #[test]
    fn done_threads_list_newest_first_and_restore_takes_the_mark_back() {
        let c = Cache::open_in_memory().unwrap();
        let older = notification_thread("1", "2026-09-19T11:00:00Z");
        let newer = notification_thread("2", "2026-09-19T12:00:00Z");
        let other = notification_thread("3", "2026-09-19T10:00:00Z");
        c.store_threads(&[newer.clone(), older.clone(), other.clone()], None)
            .unwrap();
        c.dismiss_notification(&older).unwrap();
        c.dismiss_notification(&newer).unwrap();
        assert_eq!(
            c.load_done_threads().unwrap(),
            vec![newer.clone(), older.clone()]
        );
        assert_eq!(c.load_threads().unwrap(), vec![other.clone()]);

        assert_eq!(
            c.restore_notification(&older.id).unwrap(),
            Some(older.clone())
        );
        assert_eq!(c.load_done_threads().unwrap(), vec![newer.clone()]);
        assert_eq!(c.restore_notification(&older.id).unwrap(), None);

        // With the mark gone, the next poll lists it again.
        let visible = c
            .store_notification_snapshot(&[newer, older.clone(), other.clone()], None)
            .unwrap();
        assert_eq!(visible, vec![older, other]);
    }

    #[test]
    fn a_snooze_hides_the_thread_until_its_time_then_brings_it_back_unread() {
        let c = Cache::open_in_memory().unwrap();
        let mut read = notification_thread("1", "2026-09-19T12:00:00Z");
        read.unread = false;
        let other = notification_thread("2", "2026-09-19T11:00:00Z");
        c.store_threads(&[read.clone(), other.clone()], None)
            .unwrap();
        let until = Timestamp::now() + jiff::SignedDuration::from_hours(3);
        c.snooze_notification(&read, until).unwrap();

        assert_eq!(c.load_threads().unwrap(), vec![other.clone()]);
        assert!(
            c.load_done_threads().unwrap().is_empty(),
            "a snooze is not Done"
        );
        let snoozed = c.load_snoozed_threads().unwrap();
        assert_eq!(snoozed.len(), 1);
        assert_eq!(snoozed[0].id, read.id);
        assert_eq!(snoozed[0].snoozed_until, Some(until));
        let visible = c
            .store_notification_snapshot(&[read.clone(), other.clone()], None)
            .unwrap();
        assert_eq!(
            visible,
            vec![other.clone()],
            "the same revision stays snoozed"
        );

        // Nothing is due yet.
        assert!(c
            .wake_snoozed_notifications(Timestamp::now())
            .unwrap()
            .is_empty());
        let woken = c.wake_snoozed_notifications(until).unwrap();
        assert_eq!(woken.len(), 1);
        assert!(
            !woken[0].unread && !woken[0].local_unread,
            "back read, as it went"
        );
        assert_eq!(woken[0].snoozed_until, None);
        assert!(c.load_snoozed_threads().unwrap().is_empty());
        assert!(c.notification_dismissals().unwrap().is_empty());
    }

    #[test]
    fn new_unread_activity_lifts_a_snooze_early_and_read_activity_does_not() {
        let c = Cache::open_in_memory().unwrap();
        let snoozed = notification_thread("1", "2026-09-19T12:00:00Z");
        c.snooze_notification(
            &snoozed,
            Timestamp::now() + jiff::SignedDuration::from_hours(3),
        )
        .unwrap();

        let mut read_newer = notification_thread("1", "2026-09-19T13:00:00Z");
        read_newer.unread = false;
        let visible = c
            .store_notification_snapshot(std::slice::from_ref(&read_newer), None)
            .unwrap();
        assert!(visible.is_empty());

        let unread_newer = notification_thread("1", "2026-09-19T14:00:00Z");
        let visible = c
            .store_notification_snapshot(std::slice::from_ref(&unread_newer), None)
            .unwrap();
        assert_eq!(visible, vec![unread_newer]);
        assert!(c.load_snoozed_threads().unwrap().is_empty());
    }

    #[test]
    fn a_lapsed_snooze_found_by_a_poll_returns_the_thread() {
        let c = Cache::open_in_memory().unwrap();
        let mut thread = notification_thread("1", "2026-09-19T12:00:00Z");
        thread.unread = false;
        c.snooze_notification(
            &thread,
            Timestamp::now() - jiff::SignedDuration::from_secs(1),
        )
        .unwrap();
        let visible = c
            .store_notification_snapshot(std::slice::from_ref(&thread), None)
            .unwrap();
        assert_eq!(visible, vec![thread]);
    }

    #[test]
    fn done_replaces_a_snooze_and_restore_takes_either_back() {
        let c = Cache::open_in_memory().unwrap();
        let thread = notification_thread("1", "2026-09-19T12:00:00Z");
        c.snooze_notification(
            &thread,
            Timestamp::now() + jiff::SignedDuration::from_hours(3),
        )
        .unwrap();
        c.dismiss_notification(&thread).unwrap();
        assert!(c.load_snoozed_threads().unwrap().is_empty());
        assert_eq!(c.load_done_threads().unwrap(), vec![thread.clone()]);

        c.snooze_notification(
            &thread,
            Timestamp::now() + jiff::SignedDuration::from_hours(3),
        )
        .unwrap();
        assert_eq!(c.restore_notification(&thread.id).unwrap(), Some(thread));
        assert!(c.load_snoozed_threads().unwrap().is_empty());
    }

    #[test]
    fn a_poll_refreshes_done_threads_and_drops_those_github_no_longer_lists() {
        let c = Cache::open_in_memory().unwrap();
        let listed = notification_thread("1", "2026-09-19T12:00:00Z");
        let gone = notification_thread("2", "2026-09-19T11:00:00Z");
        c.dismiss_notification(&listed).unwrap();
        c.dismiss_notification(&gone).unwrap();

        let mut renamed = listed.clone();
        renamed.subject.title = "renamed".into();
        // Editing the inbox leaves the Done list alone.
        c.store_threads(&[], None).unwrap();
        assert_eq!(c.load_done_threads().unwrap().len(), 2);

        let visible = c
            .store_notification_snapshot(std::slice::from_ref(&renamed), None)
            .unwrap();
        assert!(visible.is_empty());
        assert_eq!(c.load_done_threads().unwrap(), vec![renamed]);
        // The mark stays, so the thread stays hidden if GitHub lists it again.
        assert!(c.notification_dismissals().unwrap().contains_key(&gone.id));
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
