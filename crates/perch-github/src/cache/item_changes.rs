//! Connection-local export marks. Trigger writes roll back with the cache
//! mutation, including mutations made by other cache methods in a transaction.

use std::cell::Cell;

use super::{Cache, CacheError};

pub(super) struct ItemChanges {
    publish: Box<dyn Fn(Vec<String>) + Send>,
    seen: Cell<u64>,
}

impl Cache {
    /// Observe committed changes to the data exposed by the item archive.
    /// Install only on the engine's writer. No archive state persists in SQLite.
    pub fn watch_item_changes(
        &mut self,
        publish: impl Fn(Vec<String>) + Send + 'static,
    ) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("CREATE TEMP TABLE item_changes (id TEXT PRIMARY KEY NOT NULL)")?;
        for (table, owner) in [
            ("items", "$.id"),
            ("discussions", "$.item_id"),
            ("checks", "$.item_id"),
            ("comment_threads", "$.worktree_id"),
            (
                "comment_messages",
                "(SELECT worktree_id FROM comment_threads WHERE id=$.thread_id)",
            ),
            ("review_snapshots", "$.item_id"),
            ("review_groups", "$.item_id"),
            (
                "reviewer_attempts",
                "(SELECT item_id FROM review_groups WHERE id=$.group_id)",
            ),
            ("review_findings", "$.item_id"),
            (
                "review_observations",
                "(SELECT item_id FROM review_findings WHERE id=$.finding_id)",
            ),
            (
                "finding_audit_entries",
                "(SELECT item_id FROM review_findings WHERE id=$.finding_id)",
            ),
            ("runs", "$.owner_item_id"),
        ] {
            for (event, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
                let condition = match (table, event) {
                    ("runs", "UPDATE") => "WHEN NEW.ended_at IS NOT OLD.ended_at",
                    ("runs", _) => continue,
                    ("items", "UPDATE") => {
                        "WHEN NEW.repo_full_name IS NOT OLD.repo_full_name
                        OR NEW.subject_json IS NOT OLD.subject_json OR NEW.branch IS NOT OLD.branch
                        OR NEW.base_ref IS NOT OLD.base_ref OR NEW.head_sha IS NOT OLD.head_sha
                        OR NEW.checkout_path IS NOT OLD.checkout_path
                        OR NEW.name IS NOT OLD.name OR NEW.archived_at IS NOT OLD.archived_at"
                    }
                    _ => "",
                };
                let owner = owner.replace('$', row);
                tx.execute_batch(&format!(
                    "CREATE TEMP TRIGGER mark_{table}_{event} AFTER {event} ON main.{table}
                     {condition} BEGIN
                       INSERT INTO item_changes SELECT {owner} WHERE {owner} IS NOT NULL
                       ON CONFLICT(id) DO NOTHING;
                     END;"
                ))?;
            }
        }
        tx.commit()?;
        self.item_changes = Some(ItemChanges {
            publish: Box::new(publish),
            seen: Cell::new(self.conn.total_changes()),
        });
        Ok(())
    }

    /// Hand committed marks to the engine's queue. Reads and run event
    /// appends that leave the database unchanged need no mark query.
    pub fn publish_item_changes(&self) {
        let Some(changes) = &self.item_changes else {
            return;
        };
        if changes.seen.get() == self.conn.total_changes() || !self.conn.is_autocommit() {
            return;
        }
        let result = (|| -> Result<Vec<String>, CacheError> {
            let mut stmt = self
                .conn
                .prepare_cached("DELETE FROM item_changes RETURNING id")?;
            let ids = stmt.query_map([], |row| row.get(0))?;
            Ok(ids.collect::<Result<Vec<_>, _>>()?)
        })();
        match result {
            Ok(ids) => {
                changes.seen.set(self.conn.total_changes());
                if !ids.is_empty() {
                    (changes.publish)(ids);
                }
            }
            Err(error) => tracing::warn!(%error, "could not publish item archive changes"),
        }
    }
}
