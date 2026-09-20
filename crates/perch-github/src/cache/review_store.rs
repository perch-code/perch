use std::collections::HashSet;

use jiff::Timestamp;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use super::{insert_comment_message_row, load_run_projection_on, Cache, CacheError};
use crate::comments::{self, Author, CommentMessage, RunOwner, RunPurpose, ThreadOrigin};
use crate::reviews::*;

pub(super) const REVIEW_SCHEMA: &str = r#"
CREATE TABLE review_snapshots (
    id                   TEXT PRIMARY KEY,
    item_id              TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    repository           TEXT NOT NULL,
    branch_label         TEXT,
    requested_base_ref   TEXT NOT NULL,
    base_tip_sha         TEXT NOT NULL,
    merge_base_sha       TEXT NOT NULL,
    head_sha             TEXT NOT NULL,
    dirty_edits_excluded INTEGER NOT NULL DEFAULT 0,
    retention_key        TEXT NOT NULL,
    retained_ref         TEXT,
    manifest_path        TEXT,
    diff_path            TEXT,
    created_at           TEXT NOT NULL,
    UNIQUE (id, item_id)
);
CREATE INDEX review_snapshots_item ON review_snapshots(item_id, created_at DESC, id DESC);
CREATE INDEX review_snapshots_retention ON review_snapshots(retention_key);

ALTER TABLE comment_threads ADD COLUMN anchor_snapshot_id TEXT
    REFERENCES review_snapshots(id) ON DELETE SET NULL;
ALTER TABLE comment_threads ADD COLUMN anchor_blob_sha TEXT;
CREATE INDEX comment_threads_anchor_snapshot ON comment_threads(anchor_snapshot_id);

CREATE TABLE review_groups (
    id               TEXT PRIMARY KEY,
    item_id          TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    snapshot_id      TEXT NOT NULL,
    number           INTEGER NOT NULL,
    status           TEXT NOT NULL,
    new_findings     INTEGER NOT NULL DEFAULT 0,
    still_present    INTEGER NOT NULL DEFAULT 0,
    verified_fixed   INTEGER NOT NULL DEFAULT 0,
    not_verified     INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT NOT NULL,
    started_at       TEXT,
    ended_at         TEXT,
    FOREIGN KEY (snapshot_id, item_id) REFERENCES review_snapshots(id, item_id) ON DELETE CASCADE,
    UNIQUE (id, item_id),
    UNIQUE (item_id, number),
    CHECK (status IN ('preparing','reviewing','stopping','reconciling','empty','completed','failed','stopped','interrupted'))
);
CREATE UNIQUE INDEX review_groups_one_active_item ON review_groups(item_id)
    WHERE status IN ('preparing','reviewing','stopping','reconciling');
CREATE INDEX review_groups_history ON review_groups(item_id, number DESC);
CREATE INDEX review_groups_snapshot ON review_groups(snapshot_id);

CREATE TABLE reviewer_attempts (
    id                    TEXT PRIMARY KEY,
    group_id              TEXT NOT NULL REFERENCES review_groups(id) ON DELETE CASCADE,
    run_id                TEXT UNIQUE REFERENCES runs(id) ON DELETE SET NULL,
    backend               TEXT NOT NULL,
    executable            TEXT,
    executable_version    TEXT,
    model                 TEXT NOT NULL,
    effort                TEXT NOT NULL,
    adapter_version       TEXT NOT NULL,
    output_format_version TEXT NOT NULL,
    attempt_number        INTEGER NOT NULL,
    retry_of_id           TEXT REFERENCES reviewer_attempts(id) ON DELETE SET NULL,
    phase                 TEXT NOT NULL,
    ingestion_state       TEXT NOT NULL,
    result_kind           TEXT,
    output_id             TEXT,
    raw_result            TEXT,
    diagnostics           TEXT,
    verification_input_json TEXT NOT NULL DEFAULT '{"finding_ids":[],"truncated":false}',
    output_saved_at       TEXT,
    created_at            TEXT NOT NULL,
    started_at            TEXT,
    ended_at              TEXT,
    UNIQUE (group_id, backend, attempt_number),
    CHECK (phase IN ('queued','preparing','reviewing','extracting','verifying','reconciling','stopping','completed','failed','stopped','interrupted')),
    CHECK (ingestion_state IN ('pending','raw_saved','complete','incomplete','failed')),
    CHECK (result_kind IS NULL OR result_kind IN ('findings','clean','incomplete'))
);
CREATE UNIQUE INDEX reviewer_attempts_one_active_backend
    ON reviewer_attempts(group_id, backend)
    WHERE phase IN ('queued','preparing','reviewing','extracting','verifying','reconciling','stopping');
CREATE INDEX reviewer_attempts_group ON reviewer_attempts(group_id, backend, attempt_number);
CREATE INDEX reviewer_attempts_retry ON reviewer_attempts(retry_of_id);
CREATE INDEX reviewer_attempts_ingestion ON reviewer_attempts(ingestion_state)
    WHERE ingestion_state IN ('raw_saved','failed');

CREATE TABLE review_findings (
    id                         TEXT PRIMARY KEY,
    item_id                    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    stable_key                 TEXT NOT NULL,
    thread_id                  TEXT NOT NULL UNIQUE REFERENCES comment_threads(id) ON DELETE CASCADE,
    created_group_id           TEXT REFERENCES review_groups(id) ON DELETE SET NULL,
    disposition                TEXT NOT NULL,
    verification               TEXT NOT NULL,
    dismissal_reason           TEXT,
    possible_duplicate_of_id   TEXT REFERENCES review_findings(id) ON DELETE SET NULL,
    duplicate_of_id            TEXT REFERENCES review_findings(id) ON DELETE SET NULL,
    last_verified_snapshot_id  TEXT REFERENCES review_snapshots(id) ON DELETE SET NULL,
    last_verified_head_sha     TEXT,
    last_checked_snapshot_id   TEXT REFERENCES review_snapshots(id) ON DELETE SET NULL,
    last_checked_head_sha      TEXT,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    UNIQUE (item_id, stable_key),
    CHECK (disposition IN ('open','verified_fixed','dismissed')),
    CHECK (verification IN ('unverified','present','fixed','not_verified','disputed')),
    CHECK (possible_duplicate_of_id IS NULL OR possible_duplicate_of_id <> id),
    CHECK (duplicate_of_id IS NULL OR duplicate_of_id <> id)
);
CREATE INDEX review_findings_item ON review_findings(item_id, disposition, updated_at DESC, id);
CREATE INDEX review_findings_possible_duplicate ON review_findings(possible_duplicate_of_id);
CREATE INDEX review_findings_duplicate ON review_findings(duplicate_of_id);

CREATE TABLE review_observations (
    id               TEXT PRIMARY KEY,
    finding_id       TEXT NOT NULL REFERENCES review_findings(id) ON DELETE CASCADE,
    attempt_id       TEXT NOT NULL REFERENCES reviewer_attempts(id) ON DELETE CASCADE,
    snapshot_id      TEXT NOT NULL REFERENCES review_snapshots(id) ON DELETE CASCADE,
    source_output_id TEXT NOT NULL,
    kind             TEXT NOT NULL,
    title            TEXT NOT NULL,
    body             TEXT NOT NULL,
    severity         TEXT NOT NULL,
    anchor_json      TEXT,
    evidence_json    TEXT NOT NULL,
    message_id       TEXT REFERENCES comment_messages(id) ON DELETE SET NULL,
    created_at       TEXT NOT NULL,
    UNIQUE (attempt_id, source_output_id),
    CHECK (kind IN ('reported','verified_present','verified_fixed','not_verified','disagreement')),
    CHECK (severity IN ('unknown','note','low','medium','high','critical'))
);
CREATE INDEX review_observations_finding ON review_observations(finding_id, created_at, id);
CREATE INDEX review_observations_attempt ON review_observations(attempt_id, source_output_id);
CREATE INDEX review_observations_snapshot ON review_observations(snapshot_id);

CREATE TABLE review_anchor_mappings (
    id                    TEXT PRIMARY KEY,
    finding_id            TEXT NOT NULL REFERENCES review_findings(id) ON DELETE CASCADE,
    source_snapshot_id    TEXT NOT NULL REFERENCES review_snapshots(id) ON DELETE CASCADE,
    target_snapshot_id    TEXT NOT NULL REFERENCES review_snapshots(id) ON DELETE CASCADE,
    status                TEXT NOT NULL,
    method                TEXT NOT NULL,
    confidence            REAL,
    source_anchor_json    TEXT NOT NULL,
    target_anchor_json    TEXT,
    evidence_json         TEXT NOT NULL,
    created_by_attempt_id TEXT REFERENCES reviewer_attempts(id) ON DELETE SET NULL,
    created_at            TEXT NOT NULL,
    CHECK (status IN ('mapped','unmapped')),
    CHECK (method IN ('exact','diff','rename','symbol','semantic','manual','unavailable')),
    CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    CHECK ((status='mapped' AND target_anchor_json IS NOT NULL) OR status='unmapped')
);
CREATE INDEX review_anchor_mappings_finding
    ON review_anchor_mappings(finding_id, target_snapshot_id, created_at DESC, id DESC);
CREATE INDEX review_anchor_mappings_snapshots
    ON review_anchor_mappings(source_snapshot_id, target_snapshot_id);

CREATE TABLE finding_audit_entries (
    id                    TEXT PRIMARY KEY,
    finding_id            TEXT NOT NULL REFERENCES review_findings(id) ON DELETE CASCADE,
    action                TEXT NOT NULL,
    from_disposition      TEXT NOT NULL,
    to_disposition        TEXT NOT NULL,
    related_finding_id    TEXT REFERENCES review_findings(id) ON DELETE SET NULL,
    snapshot_id           TEXT REFERENCES review_snapshots(id) ON DELETE SET NULL,
    reason                TEXT,
    evidence_json         TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    CHECK (action IN ('dismissed','reopened','verified_fixed','not_verified','recurrence','possible_duplicate_set','possible_duplicate_cleared','duplicate_linked','duplicate_unlinked')),
    CHECK (from_disposition IN ('open','verified_fixed','dismissed')),
    CHECK (to_disposition IN ('open','verified_fixed','dismissed'))
);
CREATE INDEX finding_audit_finding ON finding_audit_entries(finding_id, created_at, id);

CREATE TABLE review_preferences (
    backend    TEXT PRIMARY KEY,
    model      TEXT,
    effort     TEXT,
    enabled    INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
"#;

pub(super) fn migrate_17_to_18(tx: &Transaction<'_>) -> Result<(), CacheError> {
    tx.execute_batch(REVIEW_SCHEMA)?;
    Ok(())
}

pub(super) fn migrate_18_to_19(tx: &Transaction<'_>) -> Result<(), CacheError> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('reviewer_attempts')
             WHERE name='verification_input_json'
         )",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        tx.execute(
            "ALTER TABLE reviewer_attempts ADD COLUMN verification_input_json TEXT NOT NULL
             DEFAULT '{\"finding_ids\":[],\"truncated\":false}'",
            [],
        )?;
    }
    Ok(())
}

const SNAPSHOT_COLUMNS: &str = "id, item_id, repository, branch_label, requested_base_ref,
    base_tip_sha, merge_base_sha, head_sha, dirty_edits_excluded, retention_key, retained_ref,
    manifest_path, diff_path, created_at";
const GROUP_COLUMNS: &str = "id, item_id, snapshot_id, number, status, new_findings,
    still_present, verified_fixed, not_verified, created_at, started_at, ended_at";
const ATTEMPT_COLUMNS: &str = "id, group_id, run_id, backend, executable, executable_version,
    model, effort, adapter_version, output_format_version, attempt_number, retry_of_id, phase,
    ingestion_state, result_kind, output_id, raw_result, diagnostics, verification_input_json,
    output_saved_at, created_at, started_at, ended_at";
const FINDING_COLUMNS: &str = "id, item_id, stable_key, thread_id, created_group_id,
    disposition, verification, dismissal_reason, possible_duplicate_of_id, duplicate_of_id,
    last_verified_snapshot_id, last_verified_head_sha, last_checked_snapshot_id,
    last_checked_head_sha, created_at, updated_at";
const OBSERVATION_COLUMNS: &str = "id, finding_id, attempt_id, snapshot_id, source_output_id,
    kind, title, body, severity, anchor_json, evidence_json, message_id, created_at";
const MAPPING_COLUMNS: &str = "id, finding_id, source_snapshot_id, target_snapshot_id, status,
    method, confidence, source_anchor_json, target_anchor_json, evidence_json,
    created_by_attempt_id, created_at";
const AUDIT_COLUMNS: &str = "id, finding_id, action, from_disposition, to_disposition,
    related_finding_id, snapshot_id, reason, evidence_json, created_at";

fn timestamp(value: String, field: &str) -> Result<Timestamp, CacheError> {
    value
        .parse()
        .map_err(|_| CacheError::Corrupt(field.to_string()))
}

fn optional_timestamp(value: Option<String>, field: &str) -> Result<Option<Timestamp>, CacheError> {
    value.map(|value| timestamp(value, field)).transpose()
}

fn corrupt_enum(field: &str) -> CacheError {
    CacheError::Corrupt(field.to_string())
}

fn read_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewSnapshot> {
    let created: String = row.get(13)?;
    Ok(ReviewSnapshot {
        id: row.get(0)?,
        item_id: row.get(1)?,
        repository: row.get(2)?,
        branch_label: row.get(3)?,
        requested_base_ref: row.get(4)?,
        base_tip_sha: row.get(5)?,
        merge_base_sha: row.get(6)?,
        head_sha: row.get(7)?,
        dirty_edits_excluded: row.get::<_, i64>(8)? != 0,
        retention_key: row.get(9)?,
        retained_ref: row.get(10)?,
        manifest_path: row.get(11)?,
        diff_path: row.get(12)?,
        created_at: created.parse().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
    })
}

type GroupRow = (
    String,
    String,
    String,
    i64,
    String,
    i64,
    i64,
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
);

fn read_group_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroupRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn group_from_row(row: GroupRow) -> Result<ReviewGroup, CacheError> {
    Ok(ReviewGroup {
        id: row.0,
        item_id: row.1,
        snapshot_id: row.2,
        number: row.3.max(0) as u64,
        status: ReviewGroupStatus::parse(&row.4).ok_or_else(|| corrupt_enum("review status"))?,
        new_findings: row.5.max(0) as u64,
        still_present: row.6.max(0) as u64,
        verified_fixed: row.7.max(0) as u64,
        not_verified: row.8.max(0) as u64,
        created_at: timestamp(row.9, "review created_at")?,
        started_at: optional_timestamp(row.10, "review started_at")?,
        ended_at: optional_timestamp(row.11, "review ended_at")?,
    })
}

type AttemptRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

fn read_attempt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttemptRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
        row.get(19)?,
        row.get(20)?,
        row.get(21)?,
        row.get(22)?,
    ))
}

fn attempt_from_row(row: AttemptRow) -> Result<ReviewerAttempt, CacheError> {
    Ok(ReviewerAttempt {
        id: row.0,
        group_id: row.1,
        run_id: row.2,
        backend: row.3,
        executable: row.4,
        executable_version: row.5,
        model: row.6,
        effort: row.7,
        adapter_version: row.8,
        output_format_version: row.9,
        attempt_number: row.10.max(0) as u64,
        retry_of: row.11,
        phase: ReviewerPhase::parse(&row.12).ok_or_else(|| corrupt_enum("reviewer phase"))?,
        ingestion_state: ReviewIngestionState::parse(&row.13)
            .ok_or_else(|| corrupt_enum("review ingestion state"))?,
        result_kind: row
            .14
            .map(|value| {
                ReviewResultKind::parse(&value).ok_or_else(|| corrupt_enum("review result kind"))
            })
            .transpose()?,
        output_id: row.15,
        raw_result: row.16,
        diagnostics: row.17,
        verification_input: serde_json::from_str(&row.18)
            .map_err(|_| CacheError::Corrupt("attempt verification input".into()))?,
        output_saved_at: optional_timestamp(row.19, "attempt output_saved_at")?,
        created_at: timestamp(row.20, "attempt created_at")?,
        started_at: optional_timestamp(row.21, "attempt started_at")?,
        ended_at: optional_timestamp(row.22, "attempt ended_at")?,
    })
}

type FindingRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn read_finding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FindingRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
    ))
}

fn finding_from_row(row: FindingRow) -> Result<Finding, CacheError> {
    Ok(Finding {
        id: row.0,
        item_id: row.1,
        stable_key: row.2,
        thread_id: row.3,
        created_group_id: row.4,
        disposition: FindingDisposition::parse(&row.5)
            .ok_or_else(|| corrupt_enum("finding disposition"))?,
        verification: FindingVerification::parse(&row.6)
            .ok_or_else(|| corrupt_enum("finding verification"))?,
        dismissal_reason: row.7,
        possible_duplicate_of: row.8,
        duplicate_of: row.9,
        last_verified_snapshot_id: row.10,
        last_verified_head_sha: row.11,
        last_checked_snapshot_id: row.12,
        last_checked_head_sha: row.13,
        created_at: timestamp(row.14, "finding created_at")?,
        updated_at: timestamp(row.15, "finding updated_at")?,
    })
}

type ObservationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
);

fn read_observation(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObservationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

fn observation_from_row(row: ObservationRow) -> Result<Observation, CacheError> {
    Ok(Observation {
        id: row.0,
        finding_id: row.1,
        attempt_id: row.2,
        snapshot_id: row.3,
        source_output_id: row.4,
        kind: ObservationKind::parse(&row.5).ok_or_else(|| corrupt_enum("observation kind"))?,
        title: row.6,
        body: row.7,
        severity: FindingSeverity::parse(&row.8).ok_or_else(|| corrupt_enum("finding severity"))?,
        anchor: row.9.map(|json| serde_json::from_str(&json)).transpose()?,
        evidence: serde_json::from_str(&row.10)?,
        message_id: row.11,
        created_at: timestamp(row.12, "observation created_at")?,
    })
}

fn insert_snapshot_on(
    conn: &rusqlite::Connection,
    snapshot: &ReviewSnapshot,
) -> Result<(), CacheError> {
    if snapshot.id.is_empty()
        || snapshot.item_id.is_empty()
        || snapshot.repository.is_empty()
        || snapshot.requested_base_ref.is_empty()
        || snapshot.base_tip_sha.is_empty()
        || snapshot.merge_base_sha.is_empty()
        || snapshot.head_sha.is_empty()
        || snapshot.retention_key.is_empty()
    {
        return Err(CacheError::ReviewConflict(
            "a review snapshot is missing required identity".into(),
        ));
    }
    conn.execute(
        "INSERT INTO review_snapshots
         (id, item_id, repository, branch_label, requested_base_ref, base_tip_sha,
          merge_base_sha, head_sha, dirty_edits_excluded, retention_key, retained_ref,
          manifest_path, diff_path, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            snapshot.id,
            snapshot.item_id,
            snapshot.repository,
            snapshot.branch_label,
            snapshot.requested_base_ref,
            snapshot.base_tip_sha,
            snapshot.merge_base_sha,
            snapshot.head_sha,
            snapshot.dirty_edits_excluded as i64,
            snapshot.retention_key,
            snapshot.retained_ref,
            snapshot.manifest_path,
            snapshot.diff_path,
            snapshot.created_at.to_string(),
        ],
    )?;
    Ok(())
}

fn load_snapshot_on(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<ReviewSnapshot>, CacheError> {
    let sql = format!("SELECT {SNAPSHOT_COLUMNS} FROM review_snapshots WHERE id=?1");
    Ok(conn
        .query_row(&sql, params![id], read_snapshot)
        .optional()?)
}

fn load_group_on(conn: &rusqlite::Connection, id: &str) -> Result<Option<ReviewGroup>, CacheError> {
    let sql = format!("SELECT {GROUP_COLUMNS} FROM review_groups WHERE id=?1");
    conn.query_row(&sql, params![id], read_group_row)
        .optional()?
        .map(group_from_row)
        .transpose()
}

fn load_attempt_on(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<ReviewerAttempt>, CacheError> {
    let sql = format!("SELECT {ATTEMPT_COLUMNS} FROM reviewer_attempts WHERE id=?1");
    conn.query_row(&sql, params![id], read_attempt_row)
        .optional()?
        .map(attempt_from_row)
        .transpose()
}

fn load_finding_on(conn: &rusqlite::Connection, id: &str) -> Result<Option<Finding>, CacheError> {
    let sql = format!("SELECT {FINDING_COLUMNS} FROM review_findings WHERE id=?1");
    conn.query_row(&sql, params![id], read_finding_row)
        .optional()?
        .map(finding_from_row)
        .transpose()
}

fn validate_attempt_run_on(
    conn: &rusqlite::Connection,
    item_id: &str,
    attempt_id: &str,
    run_id: Option<&str>,
) -> Result<(), CacheError> {
    let Some(run_id) = run_id else { return Ok(()) };
    let run = load_run_projection_on(conn, run_id)?
        .ok_or_else(|| CacheError::ReviewConflict(format!("no review run {run_id}")))?;
    if run.purpose != RunPurpose::WorktreeReview
        || !matches!(
            &run.owner,
            RunOwner::Review {
                item_id: owner_item,
                reviewer_run_id,
            } if owner_item == item_id && reviewer_run_id == attempt_id
        )
    {
        return Err(CacheError::ReviewConflict(format!(
            "run {run_id} does not belong to reviewer attempt {attempt_id}"
        )));
    }
    Ok(())
}

fn insert_attempt_on(
    conn: &rusqlite::Connection,
    group_id: &str,
    item_id: &str,
    attempt: &NewReviewerAttempt,
    attempt_number: u64,
    retry_of: Option<&str>,
) -> Result<ReviewerAttempt, CacheError> {
    if attempt.id.is_empty()
        || attempt.backend.is_empty()
        || attempt.model.is_empty()
        || attempt.effort.is_empty()
        || attempt.adapter_version.is_empty()
        || attempt.output_format_version.is_empty()
    {
        return Err(CacheError::ReviewConflict(
            "a reviewer attempt is missing required settings".into(),
        ));
    }
    validate_attempt_run_on(conn, item_id, &attempt.id, attempt.run_id.as_deref())?;
    conn.execute(
        "INSERT INTO reviewer_attempts
         (id, group_id, run_id, backend, executable, executable_version, model, effort,
          adapter_version, output_format_version, attempt_number, retry_of_id, phase,
          ingestion_state, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'queued','pending',?13)",
        params![
            attempt.id,
            group_id,
            attempt.run_id,
            attempt.backend,
            attempt.executable,
            attempt.executable_version,
            attempt.model,
            attempt.effort,
            attempt.adapter_version,
            attempt.output_format_version,
            attempt_number as i64,
            retry_of,
            attempt.created_at.to_string(),
        ],
    )?;
    Ok(ReviewerAttempt {
        id: attempt.id.clone(),
        group_id: group_id.to_string(),
        run_id: attempt.run_id.clone(),
        backend: attempt.backend.clone(),
        executable: attempt.executable.clone(),
        executable_version: attempt.executable_version.clone(),
        model: attempt.model.clone(),
        effort: attempt.effort.clone(),
        adapter_version: attempt.adapter_version.clone(),
        output_format_version: attempt.output_format_version.clone(),
        attempt_number,
        retry_of: retry_of.map(str::to_string),
        phase: ReviewerPhase::Queued,
        ingestion_state: ReviewIngestionState::Pending,
        result_kind: None,
        output_id: None,
        raw_result: None,
        diagnostics: None,
        verification_input: ReviewVerificationInput::default(),
        output_saved_at: None,
        created_at: attempt.created_at,
        started_at: None,
        ended_at: None,
    })
}

fn validate_verification_input_on(
    conn: &rusqlite::Connection,
    attempt: &ReviewerAttempt,
    input: &ReviewVerificationInput,
) -> Result<(), CacheError> {
    if input.finding_ids.len() > MAX_REVIEW_VERIFICATION_FINDINGS {
        return Err(CacheError::ReviewConflict(format!(
            "prior-finding verification is limited to {MAX_REVIEW_VERIFICATION_FINDINGS} findings"
        )));
    }
    if input.truncated && input.finding_ids.len() != MAX_REVIEW_VERIFICATION_FINDINGS {
        return Err(CacheError::ReviewConflict(
            "a truncated prior-finding verification input must fill its bound".into(),
        ));
    }
    let group = load_group_on(conn, &attempt.group_id)?
        .ok_or_else(|| CacheError::Corrupt("reviewer attempt group".into()))?;
    let mut unique = HashSet::new();
    for finding_id in &input.finding_ids {
        if finding_id.is_empty() || !unique.insert(finding_id) {
            return Err(CacheError::ReviewConflict(
                "prior-finding verification ids must be non-empty and unique".into(),
            ));
        }
        let finding = load_finding_on(conn, finding_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no verification finding {finding_id}"))
        })?;
        if finding.item_id != group.item_id {
            return Err(CacheError::ReviewConflict(
                "a verifier input finding must belong to the review item".into(),
            ));
        }
    }
    Ok(())
}

fn active_group_for_item_on(
    conn: &rusqlite::Connection,
    item_id: &str,
) -> Result<Option<ReviewGroup>, CacheError> {
    let sql = format!(
        "SELECT {GROUP_COLUMNS} FROM review_groups
         WHERE item_id=?1 AND status IN ('preparing','reviewing','stopping','reconciling')
         ORDER BY number DESC LIMIT 1"
    );
    conn.query_row(&sql, params![item_id], read_group_row)
        .optional()?
        .map(group_from_row)
        .transpose()
}

impl Cache {
    /// Atomically admit one immutable snapshot, its group, and every selected
    /// reviewer. An immediate transaction plus the partial unique index
    /// serializes competing admissions for the same item.
    pub fn admit_review_group(
        &self,
        admission: &ReviewAdmission,
    ) -> Result<ReviewGroup, CacheError> {
        if admission.group_id.is_empty() {
            return Err(CacheError::ReviewConflict(
                "a review needs a group id".into(),
            ));
        }
        let mut backends = HashSet::new();
        if admission
            .attempts
            .iter()
            .any(|attempt| !backends.insert(attempt.backend.as_str()))
        {
            return Err(CacheError::ReviewConflict(
                "a review cannot admit the same backend twice".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let item_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE id=?1)",
            params![admission.snapshot.item_id],
            |row| row.get(0),
        )?;
        if !item_exists {
            return Err(CacheError::ReviewConflict(format!(
                "no item {}",
                admission.snapshot.item_id
            )));
        }
        if active_group_for_item_on(&tx, &admission.snapshot.item_id)?.is_some() {
            return Err(CacheError::ReviewConflict(format!(
                "item {} already has an active review",
                admission.snapshot.item_id
            )));
        }
        insert_snapshot_on(&tx, &admission.snapshot)?;
        let number: i64 = tx.query_row(
            "SELECT COALESCE(MAX(number),0)+1 FROM review_groups WHERE item_id=?1",
            params![admission.snapshot.item_id],
            |row| row.get(0),
        )?;
        let status = if admission.attempts.is_empty() {
            ReviewGroupStatus::Empty
        } else {
            ReviewGroupStatus::Preparing
        };
        tx.execute(
            "INSERT INTO review_groups
             (id, item_id, snapshot_id, number, status, created_at, ended_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                admission.group_id,
                admission.snapshot.item_id,
                admission.snapshot.id,
                number,
                status.name(),
                admission.admitted_at.to_string(),
                (status == ReviewGroupStatus::Empty).then(|| admission.admitted_at.to_string()),
            ],
        )?;
        for attempt in &admission.attempts {
            insert_attempt_on(
                &tx,
                &admission.group_id,
                &admission.snapshot.item_id,
                attempt,
                1,
                None,
            )?;
        }
        let group = ReviewGroup {
            id: admission.group_id.clone(),
            item_id: admission.snapshot.item_id.clone(),
            snapshot_id: admission.snapshot.id.clone(),
            number: number.max(0) as u64,
            status,
            new_findings: 0,
            still_present: 0,
            verified_fixed: 0,
            not_verified: 0,
            created_at: admission.admitted_at,
            started_at: None,
            ended_at: (status == ReviewGroupStatus::Empty).then_some(admission.admitted_at),
        };
        tx.commit()?;
        Ok(group)
    }

    pub fn load_review_snapshot(&self, id: &str) -> Result<Option<ReviewSnapshot>, CacheError> {
        load_snapshot_on(&self.conn, id)
    }

    pub fn load_review_group(&self, id: &str) -> Result<Option<ReviewGroup>, CacheError> {
        load_group_on(&self.conn, id)
    }

    pub fn active_review_group(&self, item_id: &str) -> Result<Option<ReviewGroup>, CacheError> {
        active_group_for_item_on(&self.conn, item_id)
    }

    /// Review history newest first. `before_number` is exclusive.
    pub fn list_review_groups(
        &self,
        item_id: &str,
        before_number: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ReviewGroup>, CacheError> {
        let sql = format!(
            "SELECT {GROUP_COLUMNS} FROM review_groups
             WHERE item_id=?1 AND (?2 IS NULL OR number < ?2)
             ORDER BY number DESC LIMIT ?3"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(
                params![
                    item_id,
                    before_number.map(|value| value as i64),
                    limit.clamp(1, 500) as i64
                ],
                read_group_row,
            )?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(group_from_row).collect()
    }

    pub fn load_reviewer_attempt(&self, id: &str) -> Result<Option<ReviewerAttempt>, CacheError> {
        load_attempt_on(&self.conn, id)
    }

    pub fn list_reviewer_attempts(
        &self,
        group_id: &str,
    ) -> Result<Vec<ReviewerAttempt>, CacheError> {
        let sql = format!(
            "SELECT {ATTEMPT_COLUMNS} FROM reviewer_attempts
             WHERE group_id=?1 ORDER BY backend, attempt_number, created_at, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params![group_id], read_attempt_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(attempt_from_row).collect()
    }

    /// Admit a new attempt against the original group snapshot. Settings are
    /// required to match the attempt being retried; only identity and run
    /// ownership may change.
    pub fn admit_reviewer_retry(
        &self,
        retry_of: &str,
        attempt: &NewReviewerAttempt,
    ) -> Result<ReviewerAttempt, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let previous = load_attempt_on(&tx, retry_of)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no reviewer attempt {retry_of}")))?;
        if !previous.phase.is_terminal() {
            return Err(CacheError::ReviewConflict(format!(
                "reviewer attempt {retry_of} is still active"
            )));
        }
        if previous.backend != attempt.backend
            || previous.executable != attempt.executable
            || previous.executable_version != attempt.executable_version
            || previous.model != attempt.model
            || previous.effort != attempt.effort
            || previous.adapter_version != attempt.adapter_version
            || previous.output_format_version != attempt.output_format_version
        {
            return Err(CacheError::ReviewConflict(
                "a reviewer retry must preserve its captured settings".into(),
            ));
        }
        let group = load_group_on(&tx, &previous.group_id)?
            .ok_or_else(|| CacheError::Corrupt("reviewer attempt group".into()))?;
        if !group.status.is_terminal() {
            return Err(CacheError::ReviewConflict(format!(
                "review group {} is still active",
                group.id
            )));
        }
        if let Some(active) = active_group_for_item_on(&tx, &group.item_id)? {
            return Err(CacheError::ReviewConflict(format!(
                "item {} already has active review {}",
                group.item_id, active.id
            )));
        }
        let attempt_number: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt_number),0)+1 FROM reviewer_attempts
             WHERE group_id=?1 AND backend=?2",
            params![group.id, attempt.backend],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE review_groups SET status='preparing', ended_at=NULL WHERE id=?1",
            params![group.id],
        )?;
        let stored = insert_attempt_on(
            &tx,
            &group.id,
            &group.item_id,
            attempt,
            attempt_number.max(1) as u64,
            Some(retry_of),
        )?;
        tx.commit()?;
        Ok(stored)
    }

    /// Link an attempt to its Milestone 12 run after scheduler admission.
    /// Repeating the same link is harmless; replacing one is not.
    pub fn attach_reviewer_run(
        &self,
        attempt_id: &str,
        run_id: &str,
    ) -> Result<ReviewerAttempt, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let attempt = load_attempt_on(&tx, attempt_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no reviewer attempt {attempt_id}"))
        })?;
        let group = load_group_on(&tx, &attempt.group_id)?
            .ok_or_else(|| CacheError::Corrupt("reviewer attempt group".into()))?;
        match attempt.run_id.as_deref() {
            Some(existing) if existing == run_id => return Ok(attempt),
            Some(existing) => {
                return Err(CacheError::ReviewConflict(format!(
                    "reviewer attempt {attempt_id} already owns run {existing}"
                )))
            }
            None => {}
        }
        validate_attempt_run_on(&tx, &group.item_id, attempt_id, Some(run_id))?;
        tx.execute(
            "UPDATE reviewer_attempts SET run_id=?2 WHERE id=?1",
            params![attempt_id, run_id],
        )?;
        let stored = load_attempt_on(&tx, attempt_id)?
            .ok_or_else(|| CacheError::Corrupt("attached reviewer attempt".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn transition_reviewer_attempt(
        &self,
        attempt_id: &str,
        next: ReviewerPhase,
        at: Timestamp,
    ) -> Result<ReviewerAttempt, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let attempt = load_attempt_on(&tx, attempt_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no reviewer attempt {attempt_id}"))
        })?;
        if !attempt.phase.can_transition_to(next) {
            return Err(CacheError::InvalidReviewTransition(format!(
                "reviewer attempt {attempt_id} cannot move from {} to {}",
                attempt.phase.name(),
                next.name()
            )));
        }
        let started_at = attempt
            .started_at
            .or_else(|| (!matches!(next, ReviewerPhase::Queued)).then_some(at));
        let ended_at = next.is_terminal().then_some(at);
        tx.execute(
            "UPDATE reviewer_attempts SET phase=?2, started_at=?3, ended_at=?4 WHERE id=?1",
            params![
                attempt_id,
                next.name(),
                started_at.map(|value| value.to_string()),
                ended_at.map(|value| value.to_string()),
            ],
        )?;
        if next.is_active() && next != ReviewerPhase::Queued {
            tx.execute(
                "UPDATE review_groups SET
                     status=CASE WHEN status='preparing' THEN 'reviewing' ELSE status END,
                     started_at=COALESCE(started_at, ?2)
                 WHERE id=?1",
                params![attempt.group_id, at.to_string()],
            )?;
        }
        let stored = load_attempt_on(&tx, attempt_id)?
            .ok_or_else(|| CacheError::Corrupt("transitioned reviewer attempt".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn transition_review_group(
        &self,
        group_id: &str,
        next: ReviewGroupStatus,
        at: Timestamp,
    ) -> Result<ReviewGroup, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let group = load_group_on(&tx, group_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no review group {group_id}")))?;
        if !group.status.can_transition_to(next) {
            return Err(CacheError::InvalidReviewTransition(format!(
                "review group {group_id} cannot move from {} to {}",
                group.status.name(),
                next.name()
            )));
        }
        if next.is_terminal() {
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM reviewer_attempts
                 WHERE group_id=?1 AND phase IN
                   ('queued','preparing','reviewing','extracting','verifying','reconciling','stopping'))",
                params![group_id],
                |row| row.get(0),
            )?;
            if active {
                return Err(CacheError::ReviewConflict(format!(
                    "review group {group_id} still has active attempts"
                )));
            }
        }
        let started_at = group
            .started_at
            .or_else(|| (!matches!(next, ReviewGroupStatus::Preparing)).then_some(at));
        let ended_at = next.is_terminal().then_some(at);
        tx.execute(
            "UPDATE review_groups SET status=?2, started_at=?3, ended_at=?4 WHERE id=?1",
            params![
                group_id,
                next.name(),
                started_at.map(|value| value.to_string()),
                ended_at.map(|value| value.to_string()),
            ],
        )?;
        let stored = load_group_on(&tx, group_id)?
            .ok_or_else(|| CacheError::Corrupt("transitioned review group".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn finish_review_group(
        &self,
        group_id: &str,
        outcome: ReviewGroupStatus,
        at: Timestamp,
    ) -> Result<ReviewGroup, CacheError> {
        if !outcome.is_terminal() {
            return Err(CacheError::InvalidReviewTransition(
                "a finished review group needs a terminal outcome".into(),
            ));
        }
        self.transition_review_group(group_id, outcome, at)
    }

    /// Save the native result before local ingestion. The exact same output
    /// is idempotent; a different payload under the same attempt is rejected.
    pub fn save_reviewer_output(
        &self,
        output: &StoredReviewerOutput,
    ) -> Result<ReviewerOutputSave, CacheError> {
        if output.output_id.is_empty() {
            return Err(CacheError::ReviewConflict(
                "reviewer output needs an identity".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let attempt = load_attempt_on(&tx, &output.attempt_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no reviewer attempt {}", output.attempt_id))
        })?;
        validate_verification_input_on(&tx, &attempt, &output.verification_input)?;
        if let Some(existing_id) = attempt.output_id.as_deref() {
            if existing_id == output.output_id
                && attempt.raw_result.as_deref() == Some(output.raw_result.as_str())
                && attempt.diagnostics == output.diagnostics
                && attempt.verification_input == output.verification_input
            {
                return Ok(ReviewerOutputSave::Duplicate);
            }
            return Err(CacheError::ReviewConflict(format!(
                "reviewer attempt {} already saved a different output",
                output.attempt_id
            )));
        }
        if !matches!(
            attempt.ingestion_state,
            ReviewIngestionState::Pending | ReviewIngestionState::Failed
        ) {
            return Err(CacheError::ReviewConflict(format!(
                "reviewer attempt {} can no longer replace its output",
                output.attempt_id
            )));
        }
        let verification_input = serde_json::to_string(&output.verification_input)?;
        tx.execute(
            "UPDATE reviewer_attempts SET output_id=?2, raw_result=?3, diagnostics=?4,
             verification_input_json=?5, output_saved_at=?6,
             ingestion_state='raw_saved' WHERE id=?1",
            params![
                output.attempt_id,
                output.output_id,
                output.raw_result,
                output.diagnostics,
                verification_input,
                output.saved_at.to_string(),
            ],
        )?;
        tx.commit()?;
        Ok(ReviewerOutputSave::Stored)
    }

    pub fn fail_reviewer_ingestion(
        &self,
        attempt_id: &str,
        diagnostic: &str,
        at: Timestamp,
    ) -> Result<ReviewerAttempt, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let attempt = load_attempt_on(&tx, attempt_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no reviewer attempt {attempt_id}"))
        })?;
        if matches!(
            attempt.ingestion_state,
            ReviewIngestionState::Complete | ReviewIngestionState::Incomplete
        ) {
            return Err(CacheError::ReviewConflict(format!(
                "reviewer attempt {attempt_id} is already ingested"
            )));
        }
        if matches!(
            attempt.phase,
            ReviewerPhase::Stopping | ReviewerPhase::Stopped | ReviewerPhase::Interrupted
        ) {
            return Err(CacheError::ReviewConflict(format!(
                "cancelled reviewer attempt {attempt_id} cannot be marked as an ingestion failure"
            )));
        }
        tx.execute(
            "UPDATE reviewer_attempts SET ingestion_state='failed', diagnostics=?2,
             phase='failed', ended_at=?3 WHERE id=?1",
            params![attempt_id, diagnostic, at.to_string()],
        )?;
        let stored = load_attempt_on(&tx, attempt_id)?
            .ok_or_else(|| CacheError::Corrupt("failed reviewer ingestion".into()))?;
        tx.commit()?;
        Ok(stored)
    }
}

type MappingRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<f64>,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
);

fn read_mapping_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MappingRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn mapping_from_row(row: MappingRow) -> Result<AnchorMapping, CacheError> {
    Ok(AnchorMapping {
        id: row.0,
        finding_id: row.1,
        source_snapshot_id: row.2,
        target_snapshot_id: row.3,
        status: AnchorMappingStatus::parse(&row.4)
            .ok_or_else(|| corrupt_enum("anchor mapping status"))?,
        method: AnchorMappingMethod::parse(&row.5)
            .ok_or_else(|| corrupt_enum("anchor mapping method"))?,
        confidence: row.6,
        source_anchor: serde_json::from_str(&row.7)?,
        target_anchor: row.8.map(|json| serde_json::from_str(&json)).transpose()?,
        evidence: serde_json::from_str(&row.9)?,
        created_by_attempt_id: row.10,
        created_at: timestamp(row.11, "anchor mapping created_at")?,
    })
}

fn load_mapping_on(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<AnchorMapping>, CacheError> {
    let sql = format!("SELECT {MAPPING_COLUMNS} FROM review_anchor_mappings WHERE id=?1");
    conn.query_row(&sql, params![id], read_mapping_row)
        .optional()?
        .map(mapping_from_row)
        .transpose()
}

fn insert_anchor_mapping_on(
    conn: &rusqlite::Connection,
    mapping: &AnchorMapping,
) -> Result<AnchorMapping, CacheError> {
    if let Some(existing) = load_mapping_on(conn, &mapping.id)? {
        if existing == *mapping {
            return Ok(existing);
        }
        return Err(CacheError::ReviewConflict(format!(
            "anchor mapping {} already contains different data",
            mapping.id
        )));
    }
    if mapping.id.is_empty()
        || !mapping.source_anchor.is_valid()
        || mapping.source_anchor.snapshot_id != mapping.source_snapshot_id
        || mapping
            .confidence
            .is_some_and(|value| !(0.0..=1.0).contains(&value))
        || (mapping.status == AnchorMappingStatus::Mapped
            && mapping.target_anchor.as_ref().is_none_or(|anchor| {
                !anchor.is_valid() || anchor.snapshot_id != mapping.target_snapshot_id
            }))
        || (mapping.status == AnchorMappingStatus::Unmapped && mapping.target_anchor.is_some())
    {
        return Err(CacheError::ReviewConflict(
            "anchor mapping identity or anchors are invalid".into(),
        ));
    }
    let finding = load_finding_on(conn, &mapping.finding_id)?
        .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {}", mapping.finding_id)))?;
    let source = load_snapshot_on(conn, &mapping.source_snapshot_id)?.ok_or_else(|| {
        CacheError::ReviewConflict(format!("no review snapshot {}", mapping.source_snapshot_id))
    })?;
    let target = load_snapshot_on(conn, &mapping.target_snapshot_id)?.ok_or_else(|| {
        CacheError::ReviewConflict(format!("no review snapshot {}", mapping.target_snapshot_id))
    })?;
    if source.item_id != finding.item_id || target.item_id != finding.item_id {
        return Err(CacheError::ReviewConflict(
            "anchor mappings must stay within a finding's item".into(),
        ));
    }
    if let Some(attempt_id) = mapping.created_by_attempt_id.as_deref() {
        let attempt = load_attempt_on(conn, attempt_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no reviewer attempt {attempt_id}"))
        })?;
        let group = load_group_on(conn, &attempt.group_id)?
            .ok_or_else(|| CacheError::Corrupt("anchor mapping attempt group".into()))?;
        if group.item_id != finding.item_id {
            return Err(CacheError::ReviewConflict(
                "an anchor mapping attempt must belong to the finding's item".into(),
            ));
        }
    }
    conn.execute(
        "INSERT INTO review_anchor_mappings
         (id,finding_id,source_snapshot_id,target_snapshot_id,status,method,confidence,
          source_anchor_json,target_anchor_json,evidence_json,created_by_attempt_id,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            mapping.id,
            mapping.finding_id,
            mapping.source_snapshot_id,
            mapping.target_snapshot_id,
            mapping.status.name(),
            mapping.method.name(),
            mapping.confidence,
            serde_json::to_string(&mapping.source_anchor)?,
            mapping
                .target_anchor
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            serde_json::to_string(&mapping.evidence)?,
            mapping.created_by_attempt_id,
            mapping.created_at.to_string(),
        ],
    )?;
    Ok(mapping.clone())
}

type AuditRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn read_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn audit_from_row(row: AuditRow) -> Result<FindingAuditEntry, CacheError> {
    Ok(FindingAuditEntry {
        id: row.0,
        finding_id: row.1,
        action: FindingAuditAction::parse(&row.2)
            .ok_or_else(|| corrupt_enum("finding audit action"))?,
        from_disposition: FindingDisposition::parse(&row.3)
            .ok_or_else(|| corrupt_enum("finding audit from disposition"))?,
        to_disposition: FindingDisposition::parse(&row.4)
            .ok_or_else(|| corrupt_enum("finding audit to disposition"))?,
        related_finding_id: row.5,
        snapshot_id: row.6,
        reason: row.7,
        evidence: serde_json::from_str(&row.8)?,
        created_at: timestamp(row.9, "finding audit created_at")?,
    })
}

impl Cache {
    pub fn list_review_snapshots(&self, item_id: &str) -> Result<Vec<ReviewSnapshot>, CacheError> {
        let sql = format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM review_snapshots
             WHERE item_id=?1 ORDER BY created_at DESC, id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![item_id], read_snapshot)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn pending_review_ingestions(&self) -> Result<Vec<ReviewerAttempt>, CacheError> {
        let sql = format!(
            "SELECT {ATTEMPT_COLUMNS} FROM reviewer_attempts
             WHERE output_id IS NOT NULL AND ingestion_state IN ('raw_saved','failed')
             ORDER BY COALESCE(output_saved_at, created_at), id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map([], read_attempt_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(attempt_from_row).collect()
    }

    pub fn load_finding(&self, id: &str) -> Result<Option<Finding>, CacheError> {
        load_finding_on(&self.conn, id)
    }

    pub fn load_finding_by_stable_key(
        &self,
        item_id: &str,
        stable_key: &str,
    ) -> Result<Option<Finding>, CacheError> {
        finding_by_stable_key_on(&self.conn, item_id, stable_key)
    }

    pub fn list_findings(
        &self,
        item_id: &str,
        disposition: Option<FindingDisposition>,
    ) -> Result<Vec<Finding>, CacheError> {
        let sql = format!(
            "SELECT {FINDING_COLUMNS} FROM review_findings
             WHERE item_id=?1 AND (?2 IS NULL OR disposition=?2)
             ORDER BY updated_at DESC, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(
                params![item_id, disposition.map(FindingDisposition::name)],
                read_finding_row,
            )?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(finding_from_row).collect()
    }

    pub fn list_finding_observations(
        &self,
        finding_id: &str,
    ) -> Result<Vec<Observation>, CacheError> {
        let sql = format!(
            "SELECT {OBSERVATION_COLUMNS} FROM review_observations
             WHERE finding_id=?1 ORDER BY created_at, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params![finding_id], read_observation)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(observation_from_row).collect()
    }

    pub fn list_attempt_observations(
        &self,
        attempt_id: &str,
    ) -> Result<Vec<Observation>, CacheError> {
        observations_for_attempt_on(&self.conn, attempt_id)
    }

    /// Append an immutable mapping decision. Replaying the exact same mapping
    /// id is harmless; reusing its id for different evidence is rejected.
    pub fn insert_anchor_mapping(
        &self,
        mapping: &AnchorMapping,
    ) -> Result<AnchorMapping, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let stored = insert_anchor_mapping_on(&tx, mapping)?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn list_anchor_mappings(
        &self,
        finding_id: &str,
        target_snapshot_id: Option<&str>,
    ) -> Result<Vec<AnchorMapping>, CacheError> {
        let sql = format!(
            "SELECT {MAPPING_COLUMNS} FROM review_anchor_mappings
             WHERE finding_id=?1 AND (?2 IS NULL OR target_snapshot_id=?2)
             ORDER BY created_at DESC, id DESC"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped =
                stmt.query_map(params![finding_id, target_snapshot_id], read_mapping_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(mapping_from_row).collect()
    }

    pub fn list_finding_audit(
        &self,
        finding_id: &str,
    ) -> Result<Vec<FindingAuditEntry>, CacheError> {
        let sql = format!(
            "SELECT {AUDIT_COLUMNS} FROM finding_audit_entries
             WHERE finding_id=?1 ORDER BY created_at, id"
        );
        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params![finding_id], read_audit_row)?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter().map(audit_from_row).collect()
    }

    pub fn store_review_preference(&self, preference: &ReviewPreference) -> Result<(), CacheError> {
        if preference.backend.is_empty() {
            return Err(CacheError::ReviewConflict(
                "a review preference needs a backend".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO review_preferences (backend,model,effort,enabled,updated_at)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(backend) DO UPDATE SET model=excluded.model, effort=excluded.effort,
               enabled=excluded.enabled, updated_at=excluded.updated_at",
            params![
                preference.backend,
                preference.model,
                preference.effort,
                preference.enabled,
                preference.updated_at.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn load_review_preference(
        &self,
        backend: &str,
    ) -> Result<Option<ReviewPreference>, CacheError> {
        let row = self
            .conn
            .query_row(
                "SELECT backend,model,effort,enabled,updated_at
                 FROM review_preferences WHERE backend=?1",
                params![backend],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            Ok(ReviewPreference {
                backend: row.0,
                model: row.1,
                effort: row.2,
                enabled: row.3,
                updated_at: timestamp(row.4, "review preference updated_at")?,
            })
        })
        .transpose()
    }

    pub fn load_review_preferences(&self) -> Result<Vec<ReviewPreference>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT backend,model,effort,enabled,updated_at
             FROM review_preferences ORDER BY backend",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|row| {
                Ok(ReviewPreference {
                    backend: row.0,
                    model: row.1,
                    effort: row.2,
                    enabled: row.3,
                    updated_at: timestamp(row.4, "review preference updated_at")?,
                })
            })
            .collect()
    }
}

fn ensure_related_finding_on(
    conn: &rusqlite::Connection,
    finding: &Finding,
    related_id: &str,
) -> Result<Finding, CacheError> {
    let related = load_finding_on(conn, related_id)?
        .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {related_id}")))?;
    if related.id == finding.id || related.item_id != finding.item_id {
        return Err(CacheError::ReviewConflict(
            "related findings must be different findings on the same item".into(),
        ));
    }
    Ok(related)
}

impl Cache {
    pub fn dismiss_finding(
        &self,
        finding_id: &str,
        reason: &str,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        if reason.trim().is_empty() {
            return Err(CacheError::ReviewConflict(
                "dismissing a finding requires a reason".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        if finding.disposition == FindingDisposition::Dismissed
            && finding.dismissal_reason.as_deref() == Some(reason)
        {
            return Ok(finding);
        }
        tx.execute(
            "UPDATE review_findings SET disposition='dismissed', dismissal_reason=?2,
             updated_at=?3 WHERE id=?1",
            params![finding_id, reason, at.to_string()],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::Dismissed,
                from: finding.disposition,
                to: FindingDisposition::Dismissed,
                related_finding_id: None,
                snapshot_id: finding.last_checked_snapshot_id.as_deref(),
                reason: Some(reason),
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("dismissed finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn reopen_finding(
        &self,
        finding_id: &str,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        if finding.disposition == FindingDisposition::Open {
            return Ok(finding);
        }
        tx.execute(
            "UPDATE review_findings SET disposition='open', dismissal_reason=NULL,
             updated_at=?2 WHERE id=?1",
            params![finding_id, at.to_string()],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::Reopened,
                from: finding.disposition,
                to: FindingDisposition::Open,
                related_finding_id: None,
                snapshot_id: finding.last_checked_snapshot_id.as_deref(),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("reopened finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn mark_finding_verified_fixed(
        &self,
        finding_id: &str,
        snapshot_id: &str,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        let snapshot = load_snapshot_on(&tx, snapshot_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no review snapshot {snapshot_id}"))
        })?;
        if snapshot.item_id != finding.item_id {
            return Err(CacheError::ReviewConflict(
                "a finding can only be verified on its own item".into(),
            ));
        }
        if !incoming_snapshot_is_current_on(&tx, &finding, &snapshot)? {
            return Err(CacheError::ReviewConflict(
                "an older snapshot cannot replace a newer finding verification".into(),
            ));
        }
        let disposition = if finding.disposition == FindingDisposition::Dismissed {
            FindingDisposition::Dismissed
        } else {
            FindingDisposition::VerifiedFixed
        };
        tx.execute(
            "UPDATE review_findings SET disposition=?2, verification='fixed',
             last_checked_snapshot_id=?3, last_checked_head_sha=?4,
             last_verified_snapshot_id=?3, last_verified_head_sha=?4, updated_at=?5
             WHERE id=?1",
            params![
                finding_id,
                disposition.name(),
                snapshot.id,
                snapshot.head_sha,
                at.to_string(),
            ],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::VerifiedFixed,
                from: finding.disposition,
                to: disposition,
                related_finding_id: None,
                snapshot_id: Some(snapshot_id),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("verified finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn record_finding_not_verified(
        &self,
        finding_id: &str,
        snapshot_id: &str,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        let snapshot = load_snapshot_on(&tx, snapshot_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no review snapshot {snapshot_id}"))
        })?;
        if snapshot.item_id != finding.item_id {
            return Err(CacheError::ReviewConflict(
                "a finding can only be checked on its own item".into(),
            ));
        }
        if !incoming_snapshot_is_current_on(&tx, &finding, &snapshot)? {
            return Ok(finding);
        }
        tx.execute(
            "UPDATE review_findings SET verification='not_verified',
             last_checked_snapshot_id=?2, last_checked_head_sha=?3, updated_at=?4
             WHERE id=?1",
            params![finding_id, snapshot.id, snapshot.head_sha, at.to_string()],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::NotVerified,
                from: finding.disposition,
                to: finding.disposition,
                related_finding_id: None,
                snapshot_id: Some(snapshot_id),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("checked finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn record_finding_recurrence(
        &self,
        finding_id: &str,
        snapshot_id: &str,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        let snapshot = load_snapshot_on(&tx, snapshot_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no review snapshot {snapshot_id}"))
        })?;
        if snapshot.item_id != finding.item_id {
            return Err(CacheError::ReviewConflict(
                "a finding can only recur on its own item".into(),
            ));
        }
        if !incoming_snapshot_is_current_on(&tx, &finding, &snapshot)? {
            return Ok(finding);
        }
        let disposition = if finding.disposition == FindingDisposition::Dismissed {
            FindingDisposition::Dismissed
        } else {
            FindingDisposition::Open
        };
        tx.execute(
            "UPDATE review_findings SET disposition=?2, verification='present',
             last_checked_snapshot_id=?3, last_checked_head_sha=?4,
             last_verified_snapshot_id=?3, last_verified_head_sha=?4,
             updated_at=?5 WHERE id=?1",
            params![
                finding_id,
                disposition.name(),
                snapshot.id,
                snapshot.head_sha,
                at.to_string(),
            ],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::Recurrence,
                from: finding.disposition,
                to: disposition,
                related_finding_id: None,
                snapshot_id: Some(snapshot_id),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("recurrent finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn set_possible_duplicate(
        &self,
        finding_id: &str,
        possible_duplicate_of: Option<&str>,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        if finding.possible_duplicate_of.as_deref() == possible_duplicate_of {
            return Ok(finding);
        }
        if let Some(related_id) = possible_duplicate_of {
            ensure_related_finding_on(&tx, &finding, related_id)?;
        }
        tx.execute(
            "UPDATE review_findings SET possible_duplicate_of_id=?2, updated_at=?3 WHERE id=?1",
            params![finding_id, possible_duplicate_of, at.to_string()],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: if possible_duplicate_of.is_some() {
                    FindingAuditAction::PossibleDuplicateSet
                } else {
                    FindingAuditAction::PossibleDuplicateCleared
                },
                from: finding.disposition,
                to: finding.disposition,
                related_finding_id: possible_duplicate_of
                    .or(finding.possible_duplicate_of.as_deref()),
                snapshot_id: finding.last_checked_snapshot_id.as_deref(),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("possible duplicate finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn link_duplicate(
        &self,
        finding_id: &str,
        canonical_finding_id: &str,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        ensure_related_finding_on(&tx, &finding, canonical_finding_id)?;
        if finding.duplicate_of.as_deref() == Some(canonical_finding_id) {
            return Ok(finding);
        }
        let cycle: bool = tx.query_row(
            "WITH RECURSIVE chain(id) AS (
                SELECT ?1 UNION ALL
                SELECT f.duplicate_of_id FROM review_findings f JOIN chain c ON f.id=c.id
                WHERE f.duplicate_of_id IS NOT NULL
             ) SELECT EXISTS(SELECT 1 FROM chain WHERE id=?2)",
            params![canonical_finding_id, finding_id],
            |row| row.get(0),
        )?;
        if cycle {
            return Err(CacheError::ReviewConflict(
                "duplicate links cannot form a cycle".into(),
            ));
        }
        tx.execute(
            "UPDATE review_findings SET duplicate_of_id=?2, possible_duplicate_of_id=NULL,
             updated_at=?3 WHERE id=?1",
            params![finding_id, canonical_finding_id, at.to_string()],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::DuplicateLinked,
                from: finding.disposition,
                to: finding.disposition,
                related_finding_id: Some(canonical_finding_id),
                snapshot_id: finding.last_checked_snapshot_id.as_deref(),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("linked duplicate finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn unlink_duplicate(
        &self,
        finding_id: &str,
        reason: Option<&str>,
        evidence: &ReviewEvidence,
        at: Timestamp,
    ) -> Result<Finding, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let finding = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::ReviewConflict(format!("no finding {finding_id}")))?;
        let Some(previous) = finding.duplicate_of.as_deref() else {
            return Ok(finding);
        };
        tx.execute(
            "UPDATE review_findings SET duplicate_of_id=NULL, updated_at=?2 WHERE id=?1",
            params![finding_id, at.to_string()],
        )?;
        insert_audit_on(
            &tx,
            FindingAuditInsert {
                finding_id,
                action: FindingAuditAction::DuplicateUnlinked,
                from: finding.disposition,
                to: finding.disposition,
                related_finding_id: Some(previous),
                snapshot_id: finding.last_checked_snapshot_id.as_deref(),
                reason,
                evidence,
                at,
            },
        )?;
        let stored = load_finding_on(&tx, finding_id)?
            .ok_or_else(|| CacheError::Corrupt("unlinked duplicate finding".into()))?;
        tx.commit()?;
        Ok(stored)
    }
}

pub(super) fn delete_review_history_on(
    conn: &rusqlite::Connection,
    item_id: &str,
) -> Result<DeletedReviewHistory, CacheError> {
    if active_group_for_item_on(conn, item_id)?.is_some() {
        return Err(CacheError::ReviewConflict(format!(
            "item {item_id} has an active review"
        )));
    }
    let snapshots = {
        let sql = format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM review_snapshots
             WHERE item_id=?1 ORDER BY created_at, id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![item_id], read_snapshot)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let thread_ids = {
        let mut stmt = conn
            .prepare("SELECT thread_id FROM review_findings WHERE item_id=?1 ORDER BY thread_id")?;
        let rows = stmt.query_map(params![item_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let run_ids = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT r.id FROM runs r
             WHERE (r.owner_item_id=?1 AND r.purpose='worktree_review')
                OR r.owner_thread_id IN
                   (SELECT thread_id FROM review_findings WHERE item_id=?1)
             ORDER BY r.id",
        )?;
        let rows = stmt.query_map(params![item_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    conn.execute(
        "DELETE FROM run_events WHERE run_id IN (
           SELECT r.id FROM runs r
           WHERE (r.owner_item_id=?1 AND r.purpose='worktree_review')
              OR r.owner_thread_id IN
                 (SELECT thread_id FROM review_findings WHERE item_id=?1)
         )",
        params![item_id],
    )?;
    conn.execute(
        "DELETE FROM runs WHERE (owner_item_id=?1 AND purpose='worktree_review')
           OR owner_thread_id IN
              (SELECT thread_id FROM review_findings WHERE item_id=?1)",
        params![item_id],
    )?;
    conn.execute(
        "DELETE FROM comment_messages WHERE thread_id IN
           (SELECT thread_id FROM review_findings WHERE item_id=?1)",
        params![item_id],
    )?;
    conn.execute(
        "DELETE FROM review_findings WHERE item_id=?1",
        params![item_id],
    )?;
    for thread_id in &thread_ids {
        conn.execute(
            "DELETE FROM comment_threads WHERE id=?1",
            params![thread_id],
        )?;
    }
    conn.execute(
        "DELETE FROM review_snapshots WHERE item_id=?1",
        params![item_id],
    )?;
    Ok(DeletedReviewHistory {
        snapshots,
        thread_ids,
        run_ids,
    })
}

impl Cache {
    /// Delete database-owned review history in one transaction and return
    /// exact retained resources for caller-managed Git/file cleanup.
    pub fn delete_review_history(&self, item_id: &str) -> Result<DeletedReviewHistory, CacheError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let deleted = delete_review_history_on(&tx, item_id)?;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn review_snapshot_reference_count(&self, retention_key: &str) -> Result<u64, CacheError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM review_snapshots WHERE retention_key=?1",
            params![retention_key],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }
}

fn observations_for_attempt_on(
    conn: &rusqlite::Connection,
    attempt_id: &str,
) -> Result<Vec<Observation>, CacheError> {
    let sql = format!(
        "SELECT {OBSERVATION_COLUMNS} FROM review_observations
         WHERE attempt_id=?1 ORDER BY created_at, id"
    );
    let rows = {
        let mut stmt = conn.prepare(&sql)?;
        let mapped = stmt.query_map(params![attempt_id], read_observation)?;
        mapped.collect::<Result<Vec<_>, _>>()?
    };
    rows.into_iter().map(observation_from_row).collect()
}

fn validate_evidence(evidence: &ReviewEvidence, raw_result: &str) -> Result<(), CacheError> {
    for span in &evidence.spans {
        let start = usize::try_from(span.start).map_err(|_| {
            CacheError::ReviewConflict("review evidence offset is out of range".into())
        })?;
        let end = usize::try_from(span.end).map_err(|_| {
            CacheError::ReviewConflict("review evidence offset is out of range".into())
        })?;
        if start > end || raw_result.get(start..end) != Some(span.excerpt.as_str()) {
            return Err(CacheError::ReviewConflict(
                "review evidence is not an exact span of the saved reviewer output".into(),
            ));
        }
    }
    Ok(())
}

struct FindingAuditInsert<'a> {
    finding_id: &'a str,
    action: FindingAuditAction,
    from: FindingDisposition,
    to: FindingDisposition,
    related_finding_id: Option<&'a str>,
    snapshot_id: Option<&'a str>,
    reason: Option<&'a str>,
    evidence: &'a ReviewEvidence,
    at: Timestamp,
}

fn insert_audit_on(
    conn: &rusqlite::Connection,
    audit: FindingAuditInsert<'_>,
) -> Result<(), CacheError> {
    conn.execute(
        "INSERT INTO finding_audit_entries
         (id, finding_id, action, from_disposition, to_disposition,
          related_finding_id, snapshot_id, reason, evidence_json, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            comments::new_id(),
            audit.finding_id,
            audit.action.name(),
            audit.from.name(),
            audit.to.name(),
            audit.related_finding_id,
            audit.snapshot_id,
            audit.reason,
            serde_json::to_string(audit.evidence)?,
            audit.at.to_string(),
        ],
    )?;
    Ok(())
}

fn incoming_snapshot_is_current_on(
    conn: &rusqlite::Connection,
    finding: &Finding,
    incoming: &ReviewSnapshot,
) -> Result<bool, CacheError> {
    let Some(current_id) = finding.last_checked_snapshot_id.as_deref() else {
        return Ok(true);
    };
    let Some(current) = load_snapshot_on(conn, current_id)? else {
        return Ok(true);
    };
    let review_number = |snapshot_id: &str| -> Result<Option<i64>, CacheError> {
        Ok(conn.query_row(
            "SELECT MAX(number) FROM review_groups WHERE snapshot_id=?1",
            params![snapshot_id],
            |row| row.get(0),
        )?)
    };
    match (review_number(&incoming.id)?, review_number(&current.id)?) {
        (Some(incoming_number), Some(current_number)) => Ok(incoming_number >= current_number),
        _ => Ok(incoming.created_at >= current.created_at),
    }
}

fn apply_observation_projection_on(
    conn: &rusqlite::Connection,
    finding_id: &str,
    observation: &NewObservation,
    snapshot: &ReviewSnapshot,
) -> Result<(), CacheError> {
    let finding = load_finding_on(conn, finding_id)?
        .ok_or_else(|| CacheError::Corrupt(format!("no finding {finding_id}")))?;
    if !incoming_snapshot_is_current_on(conn, &finding, snapshot)? {
        return Ok(());
    }

    let contradictory_same_snapshot = finding.last_checked_snapshot_id.as_deref()
        == Some(snapshot.id.as_str())
        && matches!(
            (finding.verification, observation.kind),
            (
                FindingVerification::Fixed,
                ObservationKind::Reported | ObservationKind::VerifiedPresent
            ) | (FindingVerification::Present, ObservationKind::VerifiedFixed)
        );
    if contradictory_same_snapshot {
        let disposition = if finding.disposition == FindingDisposition::Dismissed {
            FindingDisposition::Dismissed
        } else {
            FindingDisposition::Open
        };
        conn.execute(
            "UPDATE review_findings SET disposition=?2, verification='disputed',
             last_checked_snapshot_id=?3, last_checked_head_sha=?4, updated_at=?5
             WHERE id=?1",
            params![
                finding_id,
                disposition.name(),
                snapshot.id,
                snapshot.head_sha,
                observation.created_at.to_string(),
            ],
        )?;
        return Ok(());
    }

    let mut disposition = finding.disposition;
    let verification = match observation.kind {
        ObservationKind::Reported | ObservationKind::VerifiedPresent => {
            if disposition == FindingDisposition::VerifiedFixed {
                disposition = FindingDisposition::Open;
                insert_audit_on(
                    conn,
                    FindingAuditInsert {
                        finding_id,
                        action: FindingAuditAction::Recurrence,
                        from: finding.disposition,
                        to: disposition,
                        related_finding_id: None,
                        snapshot_id: Some(&snapshot.id),
                        reason: Some("reported again on a newer review snapshot"),
                        evidence: &observation.evidence,
                        at: observation.created_at,
                    },
                )?;
            }
            FindingVerification::Present
        }
        ObservationKind::VerifiedFixed => {
            if disposition != FindingDisposition::Dismissed {
                disposition = FindingDisposition::VerifiedFixed;
            }
            if disposition != finding.disposition {
                insert_audit_on(
                    conn,
                    FindingAuditInsert {
                        finding_id,
                        action: FindingAuditAction::VerifiedFixed,
                        from: finding.disposition,
                        to: disposition,
                        related_finding_id: None,
                        snapshot_id: Some(&snapshot.id),
                        reason: None,
                        evidence: &observation.evidence,
                        at: observation.created_at,
                    },
                )?;
            }
            FindingVerification::Fixed
        }
        ObservationKind::NotVerified => {
            insert_audit_on(
                conn,
                FindingAuditInsert {
                    finding_id,
                    action: FindingAuditAction::NotVerified,
                    from: finding.disposition,
                    to: finding.disposition,
                    related_finding_id: None,
                    snapshot_id: Some(&snapshot.id),
                    reason: None,
                    evidence: &observation.evidence,
                    at: observation.created_at,
                },
            )?;
            FindingVerification::NotVerified
        }
        ObservationKind::Disagreement => FindingVerification::Disputed,
    };
    let verifies = matches!(
        observation.kind,
        ObservationKind::Reported
            | ObservationKind::VerifiedPresent
            | ObservationKind::VerifiedFixed
    );
    conn.execute(
        "UPDATE review_findings SET disposition=?2, verification=?3,
         last_checked_snapshot_id=?4, last_checked_head_sha=?5,
         last_verified_snapshot_id=CASE WHEN ?6 THEN ?4 ELSE last_verified_snapshot_id END,
         last_verified_head_sha=CASE WHEN ?6 THEN ?5 ELSE last_verified_head_sha END,
         dismissal_reason=CASE WHEN ?2='open' THEN NULL ELSE dismissal_reason END,
         updated_at=?7 WHERE id=?1",
        params![
            finding_id,
            disposition.name(),
            verification.name(),
            snapshot.id,
            snapshot.head_sha,
            verifies,
            observation.created_at.to_string(),
        ],
    )?;
    Ok(())
}

fn finding_by_stable_key_on(
    conn: &rusqlite::Connection,
    item_id: &str,
    stable_key: &str,
) -> Result<Option<Finding>, CacheError> {
    let sql =
        format!("SELECT {FINDING_COLUMNS} FROM review_findings WHERE item_id=?1 AND stable_key=?2");
    conn.query_row(&sql, params![item_id, stable_key], read_finding_row)
        .optional()?
        .map(finding_from_row)
        .transpose()
}

fn insert_finding_thread_on(
    conn: &rusqlite::Connection,
    item_id: &str,
    attempt: &ReviewerAttempt,
    new: &NewFinding,
    observation: &NewObservation,
) -> Result<(), CacheError> {
    if new.thread_id.is_empty()
        || new.initial_message.id.is_empty()
        || new.initial_message.body.trim().is_empty()
    {
        return Err(CacheError::ReviewConflict(
            "a new finding needs a thread and initial message".into(),
        ));
    }
    let anchor = observation.anchor.as_ref();
    conn.execute(
        "INSERT INTO comment_threads
         (id, worktree_id, origin, path, start_line, line, side, commit_sha,
          outdated, resolved, created_at, anchor_snapshot_id, anchor_blob_sha)
         VALUES (?1,?2,'local',?3,?4,?5,?6,?7,0,0,?8,?9,?10)",
        params![
            new.thread_id,
            item_id,
            anchor.map(|value| value.path.as_str()),
            anchor.map(|value| value.start_line as i64),
            anchor.map(|value| value.end_line as i64),
            anchor.map(|value| value.side.name()),
            anchor.map(|value| value.commit_sha.as_str()),
            observation.created_at.to_string(),
            anchor.map(|value| value.snapshot_id.as_str()),
            anchor.map(|value| value.blob_sha.as_str()),
        ],
    )?;
    insert_comment_message_row(
        conn,
        &CommentMessage {
            id: new.initial_message.id.clone(),
            thread_id: new.thread_id.clone(),
            origin: ThreadOrigin::Local,
            author: Author::Agent {
                backend: attempt.backend.clone(),
                model: attempt.model.clone(),
            },
            body: new.initial_message.body.clone(),
            created_at: observation.created_at,
            run: None,
        },
    )
}

fn append_review_message_on(
    conn: &rusqlite::Connection,
    attempt: &ReviewerAttempt,
    thread_id: &str,
    message: &ReviewThreadMessage,
    at: Timestamp,
) -> Result<(), CacheError> {
    if message.id.is_empty() || message.body.trim().is_empty() {
        return Err(CacheError::ReviewConflict(
            "a finding-thread message needs an id and body".into(),
        ));
    }
    insert_comment_message_row(
        conn,
        &CommentMessage {
            id: message.id.clone(),
            thread_id: thread_id.to_string(),
            origin: ThreadOrigin::Local,
            author: Author::Agent {
                backend: attempt.backend.clone(),
                model: attempt.model.clone(),
            },
            body: message.body.clone(),
            created_at: at,
            run: None,
        },
    )
}

fn recompute_group_counts_on(
    conn: &rusqlite::Connection,
    group_id: &str,
) -> Result<(), CacheError> {
    conn.execute(
        "UPDATE review_groups SET
           new_findings=(SELECT COUNT(*) FROM review_findings WHERE created_group_id=?1),
           still_present=(SELECT COUNT(DISTINCT o.finding_id)
             FROM review_observations o JOIN reviewer_attempts a ON a.id=o.attempt_id
             JOIN review_findings f ON f.id=o.finding_id
             WHERE a.group_id=?1 AND o.kind IN ('reported','verified_present')
               AND (f.created_group_id IS NULL OR f.created_group_id<>?1)),
           verified_fixed=(SELECT COUNT(DISTINCT o.finding_id)
             FROM review_observations o JOIN reviewer_attempts a ON a.id=o.attempt_id
             WHERE a.group_id=?1 AND o.kind='verified_fixed'),
           not_verified=(SELECT COUNT(DISTINCT o.finding_id)
             FROM review_observations o JOIN reviewer_attempts a ON a.id=o.attempt_id
             WHERE a.group_id=?1 AND o.kind IN ('not_verified','disagreement'))
         WHERE id=?1",
        params![group_id],
    )?;
    Ok(())
}

fn validate_ingestion_mapping_context(
    mapping: &AnchorMapping,
    attempt_id: &str,
    target_snapshot_id: &str,
) -> Result<(), CacheError> {
    if mapping.target_snapshot_id != target_snapshot_id {
        return Err(CacheError::ReviewConflict(format!(
            "anchor mapping {} does not target the reviewed snapshot",
            mapping.id
        )));
    }
    if mapping.created_by_attempt_id.as_deref() != Some(attempt_id) {
        return Err(CacheError::ReviewConflict(format!(
            "anchor mapping {} must belong to the ingesting reviewer attempt",
            mapping.id
        )));
    }
    Ok(())
}

fn repair_recovered_group_on(
    conn: &rusqlite::Connection,
    group: &ReviewGroup,
    at: Timestamp,
) -> Result<(), CacheError> {
    if !matches!(
        group.status,
        ReviewGroupStatus::Failed | ReviewGroupStatus::Interrupted
    ) {
        return Ok(());
    }
    let active: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM reviewer_attempts
         WHERE group_id=?1 AND phase IN
           ('queued','preparing','reviewing','extracting','verifying','reconciling','stopping'))",
        params![group.id],
        |row| row.get(0),
    )?;
    if active {
        return Ok(());
    }
    let any_completed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM reviewer_attempts
         WHERE group_id=?1 AND phase='completed')",
        params![group.id],
        |row| row.get(0),
    )?;
    let status = if any_completed {
        ReviewGroupStatus::Completed
    } else {
        group.status
    };
    conn.execute(
        "UPDATE review_groups SET status=?2, ended_at=?3 WHERE id=?1",
        params![group.id, status.name(), at.to_string()],
    )?;
    Ok(())
}

impl Cache {
    /// Reconcile one preserved native reviewer output. Every finding,
    /// observation, anchor mapping, thread/message, audit entry, attempt
    /// projection, and group count commits together or not at all.
    pub fn ingest_review_output(
        &self,
        ingestion: &ReviewIngestion,
    ) -> Result<ReviewIngestionResult, CacheError> {
        match ingestion.result_kind {
            ReviewResultKind::Clean if !ingestion.observations.is_empty() => {
                return Err(CacheError::ReviewConflict(
                    "an explicit clean result cannot contain observations".into(),
                ));
            }
            ReviewResultKind::Findings if ingestion.observations.is_empty() => {
                return Err(CacheError::ReviewConflict(
                    "a findings result needs at least one observation".into(),
                ));
            }
            _ => {}
        }
        let mut source_ids = HashSet::new();
        if ingestion.observations.iter().any(|entry| {
            entry.observation.source_output_id.is_empty()
                || !source_ids.insert(entry.observation.source_output_id.as_str())
        }) {
            return Err(CacheError::ReviewConflict(
                "observation source identities must be non-empty and unique per output".into(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let attempt = load_attempt_on(&tx, &ingestion.attempt_id)?.ok_or_else(|| {
            CacheError::ReviewConflict(format!("no reviewer attempt {}", ingestion.attempt_id))
        })?;
        if matches!(
            attempt.phase,
            ReviewerPhase::Stopping | ReviewerPhase::Stopped | ReviewerPhase::Interrupted
        ) {
            return Err(CacheError::ReviewConflict(format!(
                "reviewer attempt {} was cancelled and cannot be ingested",
                attempt.id
            )));
        }
        if attempt.output_id.as_deref() != Some(ingestion.output_id.as_str()) {
            return Err(CacheError::ReviewConflict(format!(
                "reviewer attempt {} did not save output {}",
                ingestion.attempt_id, ingestion.output_id
            )));
        }
        let group = load_group_on(&tx, &attempt.group_id)?
            .ok_or_else(|| CacheError::Corrupt("reviewer attempt group".into()))?;
        let snapshot = load_snapshot_on(&tx, &group.snapshot_id)?
            .ok_or_else(|| CacheError::Corrupt("review group snapshot".into()))?;
        if matches!(
            attempt.ingestion_state,
            ReviewIngestionState::Complete | ReviewIngestionState::Incomplete
        ) {
            if attempt.result_kind != Some(ingestion.result_kind) {
                return Err(CacheError::ReviewConflict(
                    "an ingested reviewer output cannot change result kind".into(),
                ));
            }
            let observations = observations_for_attempt_on(&tx, &attempt.id)?;
            return Ok(ReviewIngestionResult {
                duplicate: true,
                finding_ids: observations
                    .iter()
                    .map(|value| value.finding_id.clone())
                    .collect(),
                observation_ids: observations.into_iter().map(|value| value.id).collect(),
            });
        }
        if matches!(
            group.status,
            ReviewGroupStatus::Stopping | ReviewGroupStatus::Stopped
        ) {
            return Err(CacheError::ReviewConflict(format!(
                "review group {} was cancelled and cannot be ingested",
                group.id
            )));
        }
        if !matches!(
            attempt.ingestion_state,
            ReviewIngestionState::RawSaved | ReviewIngestionState::Failed
        ) {
            return Err(CacheError::ReviewConflict(
                "reviewer output must be saved before it is ingested".into(),
            ));
        }
        if attempt.phase == ReviewerPhase::Completed {
            return Err(CacheError::ReviewConflict(format!(
                "completed reviewer attempt {} has no completed ingestion",
                attempt.id
            )));
        }
        let raw_result = attempt.raw_result.as_deref().ok_or_else(|| {
            CacheError::Corrupt(format!("reviewer attempt {} has no raw result", attempt.id))
        })?;

        let mut finding_ids = Vec::with_capacity(ingestion.observations.len());
        let mut observation_ids = Vec::with_capacity(ingestion.observations.len());
        for entry in &ingestion.observations {
            let observation = &entry.observation;
            if observation.id.is_empty()
                || observation.title.trim().is_empty()
                || observation.body.trim().is_empty()
            {
                return Err(CacheError::ReviewConflict(
                    "an observation needs an id, title, and body".into(),
                ));
            }
            if let Some(anchor) = observation.anchor.as_ref() {
                if !anchor.is_valid() || anchor.snapshot_id != snapshot.id {
                    return Err(CacheError::ReviewConflict(
                        "an observation anchor must belong to the reviewed snapshot".into(),
                    ));
                }
            }
            validate_evidence(&observation.evidence, raw_result)?;

            let (finding, was_new) = match &entry.finding {
                FindingTarget::Existing { finding_id } => {
                    let finding = load_finding_on(&tx, finding_id)?.ok_or_else(|| {
                        CacheError::ReviewConflict(format!("no finding {finding_id}"))
                    })?;
                    if finding.item_id != group.item_id {
                        return Err(CacheError::ReviewConflict(
                            "a review observation cannot target another item's finding".into(),
                        ));
                    }
                    (finding, false)
                }
                FindingTarget::New(new) => {
                    if new.id.is_empty() || new.stable_key.trim().is_empty() {
                        return Err(CacheError::ReviewConflict(
                            "a new finding needs an id and stable key".into(),
                        ));
                    }
                    if observation.kind != ObservationKind::Reported {
                        return Err(CacheError::ReviewConflict(
                            "a new finding must start with a reported observation".into(),
                        ));
                    }
                    if let Some(existing) =
                        finding_by_stable_key_on(&tx, &group.item_id, &new.stable_key)?
                    {
                        (existing, false)
                    } else {
                        if let Some(candidate_id) = new.possible_duplicate_of.as_deref() {
                            let candidate =
                                load_finding_on(&tx, candidate_id)?.ok_or_else(|| {
                                    CacheError::ReviewConflict(format!(
                                        "no possible duplicate finding {candidate_id}"
                                    ))
                                })?;
                            if candidate.item_id != group.item_id || candidate.id == new.id {
                                return Err(CacheError::ReviewConflict(
                                    "a possible duplicate must be another finding on the same item"
                                        .into(),
                                ));
                            }
                        }
                        insert_finding_thread_on(&tx, &group.item_id, &attempt, new, observation)?;
                        tx.execute(
                            "INSERT INTO review_findings
                             (id,item_id,stable_key,thread_id,created_group_id,disposition,
                              verification,possible_duplicate_of_id,created_at,updated_at)
                             VALUES (?1,?2,?3,?4,?5,'open','unverified',?6,?7,?7)",
                            params![
                                new.id,
                                group.item_id,
                                new.stable_key,
                                new.thread_id,
                                group.id,
                                new.possible_duplicate_of,
                                observation.created_at.to_string(),
                            ],
                        )?;
                        if let Some(candidate) = new.possible_duplicate_of.as_deref() {
                            insert_audit_on(
                                &tx,
                                FindingAuditInsert {
                                    finding_id: &new.id,
                                    action: FindingAuditAction::PossibleDuplicateSet,
                                    from: FindingDisposition::Open,
                                    to: FindingDisposition::Open,
                                    related_finding_id: Some(candidate),
                                    snapshot_id: Some(&snapshot.id),
                                    reason: None,
                                    evidence: &new.possible_duplicate_evidence,
                                    at: observation.created_at,
                                },
                            )?;
                        }
                        (
                            load_finding_on(&tx, &new.id)?
                                .ok_or_else(|| CacheError::Corrupt("new review finding".into()))?,
                            true,
                        )
                    }
                }
            };

            let message_id = if was_new {
                match &entry.finding {
                    FindingTarget::New(new) => Some(new.initial_message.id.clone()),
                    FindingTarget::Existing { .. } => None,
                }
            } else if let Some(message) = observation.additional_message.as_ref() {
                append_review_message_on(
                    &tx,
                    &attempt,
                    &finding.thread_id,
                    message,
                    observation.created_at,
                )?;
                Some(message.id.clone())
            } else {
                None
            };
            tx.execute(
                "INSERT INTO review_observations
                 (id,finding_id,attempt_id,snapshot_id,source_output_id,kind,title,body,severity,
                  anchor_json,evidence_json,message_id,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    observation.id,
                    finding.id,
                    attempt.id,
                    snapshot.id,
                    observation.source_output_id,
                    observation.kind.name(),
                    observation.title,
                    observation.body,
                    observation.severity.name(),
                    observation
                        .anchor
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    serde_json::to_string(&observation.evidence)?,
                    message_id,
                    observation.created_at.to_string(),
                ],
            )?;
            apply_observation_projection_on(&tx, &finding.id, observation, &snapshot)?;
            finding_ids.push(finding.id);
            observation_ids.push(observation.id.clone());
        }
        let mut mapping_ids = HashSet::new();
        for mapping in &ingestion.anchor_mappings {
            if !mapping_ids.insert(mapping.id.as_str()) {
                return Err(CacheError::ReviewConflict(
                    "anchor mapping identities must be unique per ingestion".into(),
                ));
            }
            validate_ingestion_mapping_context(mapping, &attempt.id, &snapshot.id)?;
            insert_anchor_mapping_on(&tx, mapping)?;
        }
        let ingestion_state = match ingestion.result_kind {
            ReviewResultKind::Incomplete => ReviewIngestionState::Incomplete,
            ReviewResultKind::Findings | ReviewResultKind::Clean => ReviewIngestionState::Complete,
        };
        tx.execute(
            "UPDATE reviewer_attempts SET result_kind=?2, ingestion_state=?3,
             phase='completed', ended_at=?4 WHERE id=?1",
            params![
                attempt.id,
                ingestion.result_kind.name(),
                ingestion_state.name(),
                ingestion.completed_at.to_string(),
            ],
        )?;
        tx.execute(
            "UPDATE review_groups SET
               status=CASE WHEN status='preparing' THEN 'reconciling' ELSE status END,
               started_at=COALESCE(started_at, ?2)
             WHERE id=?1",
            params![group.id, ingestion.completed_at.to_string()],
        )?;
        repair_recovered_group_on(&tx, &group, ingestion.completed_at)?;
        recompute_group_counts_on(&tx, &group.id)?;
        tx.commit()?;
        Ok(ReviewIngestionResult {
            duplicate: false,
            finding_ids,
            observation_ids,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::cache::{CacheOpen, BASE_SCHEMA, RUN_SCHEMA, SCHEMA_VERSION};
    use crate::items::{Checkout, Item};

    fn stamp(second: u8) -> Timestamp {
        format!("2026-09-19T12:00:{second:02}Z").parse().unwrap()
    }

    fn insert_item(cache: &Cache, id: &str) {
        let mut item = Item::new("acme/widgets", stamp(0));
        item.id = id.into();
        item.checkout = Some(Checkout {
            branch: format!("feature-{id}"),
            base_ref: "main".into(),
            head_sha: "head-1".into(),
            path: PathBuf::from(format!("/tmp/{id}")),
            owned: false,
            on_disk: true,
            churn: Some((3, 1)),
            last_seen_head: None,
        });
        cache.insert_item(&item).unwrap();
    }

    fn snapshot(id: &str, item_id: &str, second: u8) -> ReviewSnapshot {
        ReviewSnapshot {
            id: id.into(),
            item_id: item_id.into(),
            repository: "acme/widgets".into(),
            branch_label: Some("feature".into()),
            requested_base_ref: "main".into(),
            base_tip_sha: format!("base-{second}"),
            merge_base_sha: format!("merge-{second}"),
            head_sha: format!("head-{second}"),
            dirty_edits_excluded: true,
            retention_key: format!("retained-{second}"),
            retained_ref: Some(format!("refs/perch/{id}")),
            manifest_path: Some(format!("{id}/manifest.json")),
            diff_path: Some(format!("{id}/review.diff")),
            created_at: stamp(second),
        }
    }

    fn attempt(id: &str, backend: &str, second: u8) -> NewReviewerAttempt {
        NewReviewerAttempt {
            id: id.into(),
            run_id: None,
            backend: backend.into(),
            executable: Some(backend.into()),
            executable_version: Some("1.0".into()),
            model: format!("{backend}-model"),
            effort: "medium".into(),
            adapter_version: "adapter-v1".into(),
            output_format_version: "review-v1".into(),
            created_at: stamp(second),
        }
    }

    fn admission(
        group_id: &str,
        snapshot_id: &str,
        item_id: &str,
        attempts: Vec<NewReviewerAttempt>,
        second: u8,
    ) -> ReviewAdmission {
        ReviewAdmission {
            group_id: group_id.into(),
            snapshot: snapshot(snapshot_id, item_id, second),
            attempts,
            admitted_at: stamp(second),
        }
    }

    fn anchor(snapshot_id: &str) -> ReviewAnchor {
        ReviewAnchor {
            snapshot_id: snapshot_id.into(),
            path: "src/lib.rs".into(),
            side: comments::Side::Right,
            start_line: 7,
            end_line: 9,
            commit_sha: "head-1".into(),
            blob_sha: "blob-1".into(),
        }
    }

    fn evidence() -> ReviewEvidence {
        ReviewEvidence {
            spans: vec![ReviewEvidenceSpan {
                start: 0,
                end: 3,
                excerpt: "bug".into(),
            }],
            symbol: Some("broken".into()),
            fingerprint: Some("candidate-only".into()),
            note: None,
        }
    }

    fn mapping(
        id: &str,
        finding_id: &str,
        source_snapshot_id: &str,
        target_snapshot_id: &str,
        attempt_id: Option<&str>,
        second: u8,
    ) -> AnchorMapping {
        AnchorMapping {
            id: id.into(),
            finding_id: finding_id.into(),
            source_snapshot_id: source_snapshot_id.into(),
            target_snapshot_id: target_snapshot_id.into(),
            status: AnchorMappingStatus::Mapped,
            method: AnchorMappingMethod::Exact,
            confidence: Some(1.0),
            source_anchor: anchor(source_snapshot_id),
            target_anchor: Some(anchor(target_snapshot_id)),
            evidence: ReviewEvidence::default(),
            created_by_attempt_id: attempt_id.map(str::to_owned),
            created_at: stamp(second),
        }
    }

    fn new_report(
        finding_id: &str,
        stable_key: &str,
        thread_id: &str,
        observation_id: &str,
        source_id: &str,
        snapshot_id: &str,
        second: u8,
    ) -> ReconciledObservation {
        ReconciledObservation {
            finding: FindingTarget::New(Box::new(NewFinding {
                id: finding_id.into(),
                stable_key: stable_key.into(),
                thread_id: thread_id.into(),
                initial_message: ReviewThreadMessage {
                    id: format!("message-{observation_id}"),
                    body: format!("Finding {stable_key}"),
                },
                possible_duplicate_of: None,
                possible_duplicate_evidence: ReviewEvidence::default(),
            })),
            observation: NewObservation {
                id: observation_id.into(),
                source_output_id: source_id.into(),
                kind: ObservationKind::Reported,
                title: format!("Problem {stable_key}"),
                body: "This is incorrect.".into(),
                severity: FindingSeverity::High,
                anchor: Some(anchor(snapshot_id)),
                evidence: evidence(),
                additional_message: None,
                created_at: stamp(second),
            },
        }
    }

    fn save_output(cache: &Cache, attempt_id: &str, output_id: &str, second: u8) {
        assert_eq!(
            cache
                .save_reviewer_output(&StoredReviewerOutput {
                    attempt_id: attempt_id.into(),
                    output_id: output_id.into(),
                    raw_result: "bug".into(),
                    diagnostics: Some("parsed locally".into()),
                    verification_input: ReviewVerificationInput::default(),
                    saved_at: stamp(second),
                })
                .unwrap(),
            ReviewerOutputSave::Stored
        );
    }

    #[test]
    fn empty_scope_is_a_terminal_history_entry_and_does_not_hold_admission() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        let empty = cache
            .admit_review_group(&admission("g1", "s1", "item", vec![], 1))
            .unwrap();
        assert_eq!(empty.status, ReviewGroupStatus::Empty);
        assert_eq!(empty.ended_at, Some(stamp(1)));
        assert!(cache.list_reviewer_attempts("g1").unwrap().is_empty());
        assert!(cache.active_review_group("item").unwrap().is_none());

        let next = cache
            .admit_review_group(&admission(
                "g2",
                "s2",
                "item",
                vec![attempt("a2", "codex", 2)],
                2,
            ))
            .unwrap();
        assert_eq!(next.number, 2);
        assert_eq!(
            cache.list_review_groups("item", None, 10).unwrap()[1].status,
            ReviewGroupStatus::Empty
        );
    }

    #[test]
    fn one_active_group_and_retry_lineage_are_enforced() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        cache
            .admit_review_group(&admission(
                "g1",
                "s1",
                "item",
                vec![attempt("a1", "codex", 1)],
                1,
            ))
            .unwrap();
        assert!(matches!(
            cache.admit_review_group(&admission(
                "g2",
                "s2",
                "item",
                vec![attempt("a2", "claude", 2)],
                2,
            )),
            Err(CacheError::ReviewConflict(_))
        ));
        cache
            .transition_reviewer_attempt("a1", ReviewerPhase::Stopped, stamp(2))
            .unwrap();
        cache
            .finish_review_group("g1", ReviewGroupStatus::Stopped, stamp(2))
            .unwrap();
        let retry = NewReviewerAttempt {
            id: "a1-retry".into(),
            created_at: stamp(3),
            ..attempt("unused", "codex", 3)
        };
        let stored = cache.admit_reviewer_retry("a1", &retry).unwrap();
        assert_eq!(stored.retry_of.as_deref(), Some("a1"));
        assert_eq!(stored.attempt_number, 2);
        assert_eq!(cache.active_review_group("item").unwrap().unwrap().id, "g1");
    }

    #[test]
    fn reviewer_output_preserves_the_exact_bounded_verification_input() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        cache
            .admit_review_group(&admission(
                "g1",
                "s1",
                "item",
                vec![attempt("a1", "codex", 1), attempt("a2", "claude", 1)],
                1,
            ))
            .unwrap();
        for id in ["a1", "a2"] {
            cache
                .transition_reviewer_attempt(id, ReviewerPhase::Reviewing, stamp(2))
                .unwrap();
        }
        save_output(&cache, "a1", "first-output", 3);
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "a1".into(),
                output_id: "first-output".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![new_report("f1", "stable", "t1", "o1", "source-1", "s1", 4)],
                anchor_mappings: vec![],
                completed_at: stamp(4),
            })
            .unwrap();

        let output = StoredReviewerOutput {
            attempt_id: "a2".into(),
            output_id: "second-output".into(),
            raw_result: "review plus verification".into(),
            diagnostics: None,
            verification_input: ReviewVerificationInput {
                finding_ids: vec!["f1".into()],
                truncated: false,
            },
            saved_at: stamp(5),
        };
        assert_eq!(
            cache.save_reviewer_output(&output).unwrap(),
            ReviewerOutputSave::Stored
        );
        assert_eq!(
            cache
                .load_reviewer_attempt("a2")
                .unwrap()
                .unwrap()
                .verification_input,
            output.verification_input
        );
        assert_eq!(
            cache.save_reviewer_output(&output).unwrap(),
            ReviewerOutputSave::Duplicate
        );

        let mut changed = output.clone();
        changed.verification_input = ReviewVerificationInput::default();
        assert!(matches!(
            cache.save_reviewer_output(&changed),
            Err(CacheError::ReviewConflict(_))
        ));
    }

    #[test]
    fn ingestion_is_atomic_idempotent_and_reuses_stable_findings() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        cache
            .admit_review_group(&admission(
                "g1",
                "s1",
                "item",
                vec![
                    attempt("a1", "codex", 1),
                    attempt("a2", "claude", 1),
                    attempt("a3", "other", 1),
                ],
                1,
            ))
            .unwrap();
        for id in ["a1", "a2", "a3"] {
            cache
                .transition_reviewer_attempt(id, ReviewerPhase::Reviewing, stamp(2))
                .unwrap();
        }
        save_output(&cache, "a1", "output-1", 3);
        let ingestion = ReviewIngestion {
            attempt_id: "a1".into(),
            output_id: "output-1".into(),
            result_kind: ReviewResultKind::Findings,
            observations: vec![new_report("f1", "stable", "t1", "o1", "source-1", "s1", 4)],
            anchor_mappings: vec![mapping("m1", "f1", "s1", "s1", Some("a1"), 4)],
            completed_at: stamp(4),
        };
        let first = cache.ingest_review_output(&ingestion).unwrap();
        assert!(!first.duplicate);
        assert_eq!(
            cache.list_anchor_mappings("f1", Some("s1")).unwrap(),
            ingestion.anchor_mappings
        );
        assert!(cache.ingest_review_output(&ingestion).unwrap().duplicate);
        let mut regenerated_replay = ingestion.clone();
        regenerated_replay.anchor_mappings[0].id = "regenerated-mapping-id".into();
        assert!(
            cache
                .ingest_review_output(&regenerated_replay)
                .unwrap()
                .duplicate
        );
        assert_eq!(
            cache.list_anchor_mappings("f1", Some("s1")).unwrap().len(),
            1
        );
        assert!(load_mapping_on(&cache.conn, "regenerated-mapping-id")
            .unwrap()
            .is_none());

        save_output(&cache, "a2", "output-2", 5);
        let second = ReviewIngestion {
            attempt_id: "a2".into(),
            output_id: "output-2".into(),
            result_kind: ReviewResultKind::Findings,
            observations: vec![new_report(
                "ignored-id",
                "stable",
                "ignored-thread",
                "o2",
                "source-2",
                "s1",
                6,
            )],
            anchor_mappings: vec![],
            completed_at: stamp(6),
        };
        assert_eq!(
            cache.ingest_review_output(&second).unwrap().finding_ids,
            ["f1"]
        );
        save_output(&cache, "a3", "output-3", 6);
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "a3".into(),
                output_id: "output-3".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![ReconciledObservation {
                    finding: FindingTarget::Existing {
                        finding_id: "f1".into(),
                    },
                    observation: NewObservation {
                        id: "o3".into(),
                        source_output_id: "source-3".into(),
                        kind: ObservationKind::VerifiedFixed,
                        title: "Fixed according to another reviewer".into(),
                        body: "This reviewer could not reproduce it.".into(),
                        severity: FindingSeverity::High,
                        anchor: Some(anchor("s1")),
                        evidence: evidence(),
                        additional_message: None,
                        created_at: stamp(7),
                    },
                }],
                anchor_mappings: vec![],
                completed_at: stamp(7),
            })
            .unwrap();
        assert_eq!(cache.list_findings("item", None).unwrap().len(), 1);
        assert_eq!(cache.list_finding_observations("f1").unwrap().len(), 3);
        assert_eq!(
            cache.load_finding("f1").unwrap().unwrap().verification,
            FindingVerification::Disputed
        );
        assert_eq!(cache.load_comment_threads("item").unwrap().len(), 1);
        let identity: (String, String) = cache
            .conn
            .query_row(
                "SELECT anchor_snapshot_id,anchor_blob_sha FROM comment_threads WHERE id='t1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(identity, ("s1".into(), "blob-1".into()));
        let group = cache.load_review_group("g1").unwrap().unwrap();
        assert_eq!(group.new_findings, 1);
    }

    #[test]
    fn clean_and_incomplete_results_stay_distinct_and_bad_evidence_rolls_back() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        cache
            .admit_review_group(&admission(
                "g1",
                "s1",
                "item",
                vec![
                    attempt("clean", "codex", 1),
                    attempt("partial", "claude", 1),
                    attempt("bad", "other", 1),
                    attempt("bad-map", "mapper", 1),
                ],
                1,
            ))
            .unwrap();
        save_output(&cache, "clean", "clean-output", 2);
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "clean".into(),
                output_id: "clean-output".into(),
                result_kind: ReviewResultKind::Clean,
                observations: vec![],
                anchor_mappings: vec![],
                completed_at: stamp(3),
            })
            .unwrap();
        save_output(&cache, "partial", "partial-output", 3);
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "partial".into(),
                output_id: "partial-output".into(),
                result_kind: ReviewResultKind::Incomplete,
                observations: vec![],
                anchor_mappings: vec![],
                completed_at: stamp(4),
            })
            .unwrap();
        assert_eq!(
            cache
                .load_reviewer_attempt("clean")
                .unwrap()
                .unwrap()
                .result_kind,
            Some(ReviewResultKind::Clean)
        );
        let partial = cache.load_reviewer_attempt("partial").unwrap().unwrap();
        assert_eq!(partial.result_kind, Some(ReviewResultKind::Incomplete));
        assert_eq!(partial.ingestion_state, ReviewIngestionState::Incomplete);

        save_output(&cache, "bad", "bad-output", 4);
        let mut bad = new_report(
            "bad-finding",
            "bad",
            "bad-thread",
            "bad-observation",
            "bad-source",
            "s1",
            5,
        );
        bad.observation.evidence.spans[0].excerpt = "invented".into();
        assert!(matches!(
            cache.ingest_review_output(&ReviewIngestion {
                attempt_id: "bad".into(),
                output_id: "bad-output".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![bad],
                anchor_mappings: vec![],
                completed_at: stamp(5),
            }),
            Err(CacheError::ReviewConflict(_))
        ));
        assert!(cache.load_finding("bad-finding").unwrap().is_none());
        assert_eq!(
            cache
                .load_reviewer_attempt("bad")
                .unwrap()
                .unwrap()
                .ingestion_state,
            ReviewIngestionState::RawSaved
        );

        save_output(&cache, "bad-map", "bad-map-output", 5);
        assert!(matches!(
            cache.ingest_review_output(&ReviewIngestion {
                attempt_id: "bad-map".into(),
                output_id: "bad-map-output".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![new_report(
                    "rolled-back-finding",
                    "rolled-back",
                    "rolled-back-thread",
                    "rolled-back-observation",
                    "rolled-back-source",
                    "s1",
                    6,
                )],
                anchor_mappings: vec![mapping(
                    "rolled-back-mapping",
                    "rolled-back-finding",
                    "s1",
                    "not-the-reviewed-snapshot",
                    Some("bad-map"),
                    6,
                )],
                completed_at: stamp(6),
            }),
            Err(CacheError::ReviewConflict(_))
        ));
        assert!(cache.load_finding("rolled-back-finding").unwrap().is_none());
        assert!(load_mapping_on(&cache.conn, "rolled-back-mapping")
            .unwrap()
            .is_none());
        assert!(cache.load_comment_threads("item").unwrap().is_empty());
        assert_eq!(
            cache
                .load_reviewer_attempt("bad-map")
                .unwrap()
                .unwrap()
                .ingestion_state,
            ReviewIngestionState::RawSaved
        );

        assert!(matches!(
            cache.ingest_review_output(&ReviewIngestion {
                attempt_id: "clean".into(),
                output_id: "clean-output".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![],
                anchor_mappings: vec![],
                completed_at: stamp(5),
            }),
            Err(CacheError::ReviewConflict(_))
        ));
    }

    #[test]
    fn cancellation_fences_ingestion_and_failed_raw_output_can_recover() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "cancel-item");
        cache
            .admit_review_group(&admission(
                "cancel-group",
                "cancel-snapshot",
                "cancel-item",
                vec![
                    attempt("completed", "codex", 1),
                    attempt("stopping", "gemini", 1),
                    attempt("stopped", "claude", 1),
                    attempt("interrupted", "other", 1),
                ],
                1,
            ))
            .unwrap();
        for id in ["completed", "stopping", "stopped", "interrupted"] {
            cache
                .transition_reviewer_attempt(id, ReviewerPhase::Reviewing, stamp(2))
                .unwrap();
            save_output(&cache, id, &format!("{id}-output"), 2);
        }
        let completed = ReviewIngestion {
            attempt_id: "completed".into(),
            output_id: "completed-output".into(),
            result_kind: ReviewResultKind::Clean,
            observations: vec![],
            anchor_mappings: vec![],
            completed_at: stamp(3),
        };
        cache.ingest_review_output(&completed).unwrap();
        cache
            .transition_reviewer_attempt("stopping", ReviewerPhase::Stopping, stamp(4))
            .unwrap();
        cache
            .transition_reviewer_attempt("stopped", ReviewerPhase::Stopped, stamp(4))
            .unwrap();
        cache
            .transition_reviewer_attempt("interrupted", ReviewerPhase::Interrupted, stamp(4))
            .unwrap();
        cache
            .transition_review_group("cancel-group", ReviewGroupStatus::Stopping, stamp(4))
            .unwrap();
        assert!(cache.ingest_review_output(&completed).unwrap().duplicate);

        for (id, phase) in [
            ("stopping", ReviewerPhase::Stopping),
            ("stopped", ReviewerPhase::Stopped),
            ("interrupted", ReviewerPhase::Interrupted),
        ] {
            assert!(matches!(
                cache.ingest_review_output(&ReviewIngestion {
                    attempt_id: id.into(),
                    output_id: format!("{id}-output"),
                    result_kind: ReviewResultKind::Clean,
                    observations: vec![],
                    anchor_mappings: vec![],
                    completed_at: stamp(5),
                }),
                Err(CacheError::ReviewConflict(_))
            ));
            let attempt = cache.load_reviewer_attempt(id).unwrap().unwrap();
            assert_eq!(attempt.phase, phase);
            assert_eq!(attempt.ingestion_state, ReviewIngestionState::RawSaved);
            assert!(matches!(
                cache.fail_reviewer_ingestion(id, "late parser failure", stamp(5)),
                Err(CacheError::ReviewConflict(_))
            ));
            assert_eq!(
                cache.load_reviewer_attempt(id).unwrap().unwrap().phase,
                phase
            );
        }

        insert_item(&cache, "recover-item");
        cache
            .admit_review_group(&admission(
                "recover-group",
                "recover-snapshot",
                "recover-item",
                vec![attempt("recover", "codex", 6)],
                6,
            ))
            .unwrap();
        cache
            .transition_reviewer_attempt("recover", ReviewerPhase::Reviewing, stamp(7))
            .unwrap();
        save_output(&cache, "recover", "recover-output", 7);
        cache
            .fail_reviewer_ingestion("recover", "temporary parse failure", stamp(8))
            .unwrap();
        cache
            .finish_review_group("recover-group", ReviewGroupStatus::Failed, stamp(8))
            .unwrap();
        let recovered = ReviewIngestion {
            attempt_id: "recover".into(),
            output_id: "recover-output".into(),
            result_kind: ReviewResultKind::Clean,
            observations: vec![],
            anchor_mappings: vec![],
            completed_at: stamp(9),
        };
        assert!(!cache.ingest_review_output(&recovered).unwrap().duplicate);
        let recovered_attempt = cache.load_reviewer_attempt("recover").unwrap().unwrap();
        assert_eq!(recovered_attempt.phase, ReviewerPhase::Completed);
        assert_eq!(
            recovered_attempt.ingestion_state,
            ReviewIngestionState::Complete
        );
        let group = cache.load_review_group("recover-group").unwrap().unwrap();
        assert_eq!(group.status, ReviewGroupStatus::Completed);
        assert_eq!(group.ended_at, Some(stamp(9)));
        assert!(cache.ingest_review_output(&recovered).unwrap().duplicate);

        insert_item(&cache, "interrupted-recovery-item");
        cache
            .admit_review_group(&admission(
                "interrupted-recovery-group",
                "interrupted-recovery-snapshot",
                "interrupted-recovery-item",
                vec![attempt("interrupted-recovery", "codex", 10)],
                10,
            ))
            .unwrap();
        cache
            .transition_reviewer_attempt(
                "interrupted-recovery",
                ReviewerPhase::Reviewing,
                stamp(11),
            )
            .unwrap();
        save_output(
            &cache,
            "interrupted-recovery",
            "interrupted-recovery-output",
            11,
        );
        cache
            .fail_reviewer_ingestion("interrupted-recovery", "temporary parse failure", stamp(12))
            .unwrap();
        cache
            .finish_review_group(
                "interrupted-recovery-group",
                ReviewGroupStatus::Interrupted,
                stamp(12),
            )
            .unwrap();
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "interrupted-recovery".into(),
                output_id: "interrupted-recovery-output".into(),
                result_kind: ReviewResultKind::Clean,
                observations: vec![],
                anchor_mappings: vec![],
                completed_at: stamp(13),
            })
            .unwrap();
        assert_eq!(
            cache
                .load_review_group("interrupted-recovery-group")
                .unwrap()
                .unwrap()
                .status,
            ReviewGroupStatus::Completed
        );
    }

    #[test]
    fn dispositions_duplicates_mappings_and_preferences_are_auditable() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        cache
            .admit_review_group(&admission(
                "g1",
                "s1",
                "item",
                vec![attempt("a1", "codex", 1)],
                1,
            ))
            .unwrap();
        cache
            .transition_reviewer_attempt("a1", ReviewerPhase::Reviewing, stamp(2))
            .unwrap();
        save_output(&cache, "a1", "output", 2);
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "a1".into(),
                output_id: "output".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![
                    new_report("f1", "one", "t1", "o1", "source-1", "s1", 3),
                    new_report("f2", "two", "t2", "o2", "source-2", "s1", 3),
                ],
                anchor_mappings: vec![],
                completed_at: stamp(3),
            })
            .unwrap();
        cache
            .finish_review_group("g1", ReviewGroupStatus::Completed, stamp(4))
            .unwrap();
        cache
            .admit_review_group(&admission("g2", "s2", "item", vec![], 5))
            .unwrap();

        let empty = ReviewEvidence::default();
        assert_eq!(
            cache
                .mark_finding_verified_fixed("f1", "s2", None, &empty, stamp(6))
                .unwrap()
                .disposition,
            FindingDisposition::VerifiedFixed
        );
        assert_eq!(
            cache
                .record_finding_recurrence("f1", "s2", None, &empty, stamp(7))
                .unwrap()
                .disposition,
            FindingDisposition::Open
        );
        let before_late = cache.load_finding("f1").unwrap().unwrap();
        let after_late = cache
            .record_finding_not_verified("f1", "s1", None, &empty, stamp(8))
            .unwrap();
        assert_eq!(after_late, before_late);
        cache
            .dismiss_finding("f1", "not actionable", &empty, stamp(9))
            .unwrap();
        assert_eq!(
            cache
                .record_finding_recurrence("f1", "s2", None, &empty, stamp(10))
                .unwrap()
                .disposition,
            FindingDisposition::Dismissed
        );
        cache
            .reopen_finding("f1", Some("new evidence"), &empty, stamp(11))
            .unwrap();
        cache
            .set_possible_duplicate("f2", Some("f1"), None, &empty, stamp(12))
            .unwrap();
        cache
            .link_duplicate("f2", "f1", None, &empty, stamp(13))
            .unwrap();
        assert!(matches!(
            cache.link_duplicate("f1", "f2", None, &empty, stamp(14)),
            Err(CacheError::ReviewConflict(_))
        ));
        cache
            .unlink_duplicate("f2", None, &empty, stamp(15))
            .unwrap();
        assert!(cache.list_finding_audit("f1").unwrap().len() >= 4);
        assert!(cache.list_finding_audit("f2").unwrap().len() >= 3);

        let mapping = AnchorMapping {
            id: "mapping".into(),
            finding_id: "f1".into(),
            source_snapshot_id: "s1".into(),
            target_snapshot_id: "s2".into(),
            status: AnchorMappingStatus::Mapped,
            method: AnchorMappingMethod::Diff,
            confidence: Some(0.9),
            source_anchor: anchor("s1"),
            target_anchor: Some(ReviewAnchor {
                commit_sha: "head-5".into(),
                blob_sha: "blob-2".into(),
                ..anchor("s2")
            }),
            evidence: empty.clone(),
            created_by_attempt_id: Some("a1".into()),
            created_at: stamp(16),
        };
        assert_eq!(cache.insert_anchor_mapping(&mapping).unwrap(), mapping);
        assert_eq!(cache.insert_anchor_mapping(&mapping).unwrap(), mapping);
        assert_eq!(
            cache.list_anchor_mappings("f1", Some("s2")).unwrap().len(),
            1
        );

        let preference = ReviewPreference {
            backend: "codex".into(),
            model: Some("saved-even-if-unavailable".into()),
            effort: Some("xhigh".into()),
            enabled: false,
            updated_at: stamp(17),
        };
        cache.store_review_preference(&preference).unwrap();
        assert_eq!(
            cache.load_review_preference("codex").unwrap(),
            Some(preference)
        );
        assert_eq!(cache.load_review_preferences().unwrap().len(), 1);
    }

    #[test]
    fn deletion_is_exact_and_finding_threads_cannot_be_removed_directly() {
        let cache = Cache::open_in_memory().unwrap();
        insert_item(&cache, "item");
        cache
            .admit_review_group(&admission(
                "g1",
                "s1",
                "item",
                vec![attempt("a1", "codex", 1)],
                1,
            ))
            .unwrap();
        save_output(&cache, "a1", "output", 2);
        cache
            .ingest_review_output(&ReviewIngestion {
                attempt_id: "a1".into(),
                output_id: "output".into(),
                result_kind: ReviewResultKind::Findings,
                observations: vec![new_report("f1", "one", "t1", "o1", "source", "s1", 3)],
                anchor_mappings: vec![],
                completed_at: stamp(3),
            })
            .unwrap();
        assert!(matches!(
            cache.delete_comment_thread("t1"),
            Err(CacheError::ReviewConflict(_))
        ));
        cache
            .finish_review_group("g1", ReviewGroupStatus::Completed, stamp(4))
            .unwrap();
        let deleted = cache.delete_review_history("item").unwrap();
        assert_eq!(deleted.thread_ids, ["t1"]);
        assert_eq!(deleted.snapshots.len(), 1);
        assert!(cache.load_item("item").unwrap().is_some());
        assert!(cache.list_review_snapshots("item").unwrap().is_empty());
        assert!(cache.load_comment_threads("item").unwrap().is_empty());
    }

    #[test]
    fn version_18_to_19_adds_verification_input_without_losing_saved_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache-v18.db");
        {
            let cache = Cache::open(&path, None).unwrap();
            insert_item(&cache, "item");
            cache
                .admit_review_group(&admission(
                    "g1",
                    "s1",
                    "item",
                    vec![attempt("a1", "codex", 1)],
                    1,
                ))
                .unwrap();
            cache
                .transition_reviewer_attempt("a1", ReviewerPhase::Reviewing, stamp(2))
                .unwrap();
            save_output(&cache, "a1", "saved-output", 3);
            cache
                .conn
                .execute_batch(
                    "ALTER TABLE reviewer_attempts DROP COLUMN verification_input_json;
                     UPDATE meta SET value='18' WHERE key='schema_version';",
                )
                .unwrap();
        }

        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 18,
                to: SCHEMA_VERSION,
            }
        );
        let attempt = cache.load_reviewer_attempt("a1").unwrap().unwrap();
        assert_eq!(attempt.output_id.as_deref(), Some("saved-output"));
        assert_eq!(attempt.raw_result.as_deref(), Some("bug"));
        assert_eq!(
            attempt.verification_input,
            ReviewVerificationInput::default()
        );
    }

    #[test]
    fn version_17_migrates_in_place_and_a_failed_migration_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(BASE_SCHEMA).unwrap();
            conn.execute_batch(RUN_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO meta(key,value) VALUES ('schema_version','17'),('kept','yes')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO comment_threads
                 (id,worktree_id,origin,outdated,resolved,created_at)
                 VALUES ('legacy','item','local',0,0,?1)",
                params![stamp(0).to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO runs
                 (id,owner_kind,owner_item_id,owner_reviewer_run_id,purpose,backend,
                  requested_model,effective_model,requested_effort,effective_effort,
                  original_request,prompt_snapshot,revision_json,repository_label,item_label,
                  status,queued_at)
                 VALUES ('legacy-review','review','item','legacy-attempt','worktree_review',
                  'codex','model','model','medium','medium','review','review',
                  '{\"kind\":\"unknown\"}','acme/widgets','item','completed',?1)",
                params![stamp(0).to_string()],
            )
            .unwrap();
        }
        let (cache, outcome) = Cache::open_with_outcome(&path, None).unwrap();
        assert_eq!(
            outcome,
            CacheOpen::Migrated {
                from: 17,
                to: SCHEMA_VERSION
            }
        );
        assert_eq!(cache.meta("kept").unwrap().as_deref(), Some("yes"));
        assert!(matches!(
            cache.load_run("legacy-review").unwrap().unwrap().owner,
            RunOwner::Review {
                item_id,
                reviewer_run_id,
            } if item_id == "item" && reviewer_run_id == "legacy-attempt"
        ));
        let columns: i64 = cache
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('comment_threads')
                 WHERE name IN ('anchor_snapshot_id','anchor_blob_sha')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 2);
        drop(cache);

        let bad_path = dir.path().join("bad.db");
        {
            let conn = rusqlite::Connection::open(&bad_path).unwrap();
            conn.execute_batch(BASE_SCHEMA).unwrap();
            conn.execute_batch(RUN_SCHEMA).unwrap();
            conn.execute(
                "ALTER TABLE comment_threads ADD COLUMN anchor_snapshot_id TEXT",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meta(key,value) VALUES ('schema_version','17')",
                [],
            )
            .unwrap();
        }
        assert!(Cache::open(&bad_path, None).is_err());
        let conn = rusqlite::Connection::open(&bad_path).unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "17");
        let review_table: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='review_snapshots')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!review_table);
    }

    #[test]
    fn concurrent_admission_leaves_one_active_group() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let cache = Cache::open(&path, None).unwrap();
        insert_item(&cache, "item");
        drop(cache);

        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for index in 1..=2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let cache = Cache::open(&path, None).unwrap();
                cache
                    .conn
                    .busy_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                let request = admission(
                    &format!("g{index}"),
                    &format!("s{index}"),
                    "item",
                    vec![attempt(
                        &format!("a{index}"),
                        &format!("backend-{index}"),
                        index,
                    )],
                    index,
                );
                barrier.wait();
                cache.admit_review_group(&request).is_ok()
            }));
        }
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|success| *success)
            .count();
        assert_eq!(successes, 1);
        let cache = Cache::open(&path, None).unwrap();
        assert_eq!(cache.list_review_groups("item", None, 10).unwrap().len(), 1);
    }
}
