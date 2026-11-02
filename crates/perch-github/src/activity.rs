//! What happened on an item while the reader was looking elsewhere: an
//! agent run or a review that ended.
//!
//! One record per item, replaced by each new event. It is read and put
//! away the way a GitHub notification is, keyed on the event's time
//! rather than the thread's `updated_at`: opening the item reads it,
//! Done hides it, and a newer event brings it back.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::comments::RunMode;

/// How a run or a review ended, for the ones that count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOutcome {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEvent {
    /// An agent run on the item ended.
    Run {
        run_id: String,
        /// The backend's name, as `RunRecord::backend` has it.
        backend: String,
        mode: RunMode,
        outcome: ActivityOutcome,
        /// Files a Work run changed, when that was counted.
        files: Option<u32>,
    },
    /// A worktree review ended, once for all its reviewers.
    Review {
        group_id: String,
        outcome: ActivityOutcome,
        findings: u32,
    },
}

impl ActivityEvent {
    /// A run or a review that failed, which the row says in the failure
    /// colour.
    pub fn failed(&self) -> bool {
        matches!(
            self,
            ActivityEvent::Run {
                outcome: ActivityOutcome::Failed,
                ..
            } | ActivityEvent::Review {
                outcome: ActivityOutcome::Failed,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemActivity {
    pub item_id: String,
    /// When the latest event happened.
    pub at: Timestamp,
    pub event: ActivityEvent,
    /// The event time the reader last opened the item at.
    pub read_at: Option<Timestamp>,
    /// The event time the reader marked Done at.
    pub done_at: Option<Timestamp>,
    /// Mark Unread, for the current event.
    pub local_unread: bool,
}

impl ItemActivity {
    pub fn is_unread(&self) -> bool {
        self.local_unread || self.read_at.is_none_or(|read| read < self.at)
    }

    /// Not put away with Done since the latest event.
    pub fn is_visible(&self) -> bool {
        self.done_at.is_none_or(|done| done < self.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn activity() -> ItemActivity {
        ItemActivity {
            item_id: "i".into(),
            at: at("2026-09-23T10:00:00Z"),
            event: ActivityEvent::Review {
                group_id: "g".into(),
                outcome: ActivityOutcome::Completed,
                findings: 1,
            },
            read_at: None,
            done_at: None,
            local_unread: false,
        }
    }

    #[test]
    fn read_and_done_hold_for_the_event_they_were_given_at() {
        let mut a = activity();
        assert!(a.is_unread());
        assert!(a.is_visible());
        a.read_at = Some(a.at);
        a.done_at = Some(a.at);
        assert!(!a.is_unread());
        assert!(!a.is_visible());
        a.local_unread = true;
        assert!(a.is_unread());
        a.local_unread = false;
        // A newer event.
        a.at = at("2026-09-23T11:00:00Z");
        assert!(a.is_unread());
        assert!(a.is_visible());
    }

    #[test]
    fn the_event_round_trips_as_json() {
        let event = ActivityEvent::Run {
            run_id: "r".into(),
            backend: "codex".into(),
            mode: RunMode::Work,
            outcome: ActivityOutcome::Failed,
            files: Some(3),
        };
        assert!(event.failed());
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<ActivityEvent>(&json).unwrap(), event);
    }
}
