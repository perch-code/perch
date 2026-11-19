//! Repository history fetched for the Markdown archive.

use rusqlite::{params, OptionalExtension};

use super::{Cache, CacheError};
use crate::client::HistorySubject;
use crate::discussion::Discussion;
use crate::items::{Subject, SubjectKind};

pub(super) const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_subjects (
    id              TEXT PRIMARY KEY NOT NULL,
    repo            TEXT NOT NULL,
    number          INTEGER NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('issue', 'pr')),
    subject_json    TEXT NOT NULL,
    discussion_json TEXT NOT NULL
);
"#;

/// Distinguishes archived GitHub subject keys from interactive item IDs.
pub const HISTORY_SUBJECT_ID_PREFIX: &str = "github-history:";

/// Stable archive change key for an imported issue or pull request.
/// GitHub numbers issues and pull requests in a single sequence.
pub fn history_subject_id(repo: &str, number: u64) -> String {
    format!(
        "{HISTORY_SUBJECT_ID_PREFIX}{}:{number}",
        repo.to_ascii_lowercase()
    )
}

impl Cache {
    /// Keep the GitHub conversation and subject together. Imported
    /// history has no item lifecycle, checkout, or local comment threads.
    pub fn store_history_subject(
        &self,
        subject: &Subject,
        discussion: &Discussion,
    ) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT INTO history_subjects (id, repo, number, kind, subject_json, discussion_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 repo = excluded.repo,
                 kind = excluded.kind,
                 subject_json = excluded.subject_json,
                 discussion_json = excluded.discussion_json
             WHERE history_subjects.subject_json != excluded.subject_json
                OR history_subjects.discussion_json != excluded.discussion_json",
            params![
                history_subject_id(subject.repo_full_name(), subject.number()),
                subject.repo_full_name(),
                subject.number(),
                subject.kind().column(),
                serde_json::to_string(subject)?,
                serde_json::to_string(discussion)?,
            ],
        )?;
        Ok(())
    }

    /// Remove an imported snapshot for a GitHub item that has gone.
    pub fn delete_history_subject(&self, id: &str) -> Result<(), CacheError> {
        self.conn
            .execute("DELETE FROM history_subjects WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn load_history_subject(
        &self,
        id: &str,
    ) -> Result<Option<(Subject, Discussion)>, CacheError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT subject_json, discussion_json FROM history_subjects WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((subject, discussion)) = row else {
            return Ok(None);
        };
        let subject = read_snapshot_json(id, &subject);
        let discussion = read_snapshot_json(id, &discussion);
        Ok(subject.zip(discussion))
    }

    /// Locate archived subjects without reading their descriptions or conversations.
    pub fn load_history_locations(&self) -> Result<Vec<(String, HistorySubject)>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT repo, number, kind = 'pr' FROM history_subjects")?;
        let rows = stmt.query_map([], |row| {
            let kind = if row.get::<_, bool>(2)? {
                SubjectKind::PullRequest
            } else {
                SubjectKind::Issue
            };
            Ok((
                row.get(0)?,
                HistorySubject {
                    number: row.get(1)?,
                    kind,
                },
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn load_history_ids(&self) -> Result<Vec<String>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT id FROM history_subjects")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

fn read_snapshot_json<T: serde::de::DeserializeOwned>(id: &str, json: &str) -> Option<T> {
    match serde_json::from_str(json) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(id, %error, "skipping unreadable GitHub history snapshot");
            None
        }
    }
}
