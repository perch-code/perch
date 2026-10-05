//! Data model mirroring the GitHub notifications REST API.
//!
//! Enums deserialize unknown strings into an `Other` variant rather than
//! failing. GitHub adds notification reasons and subject types without
//! notice, and a new one must never take down the poll.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

use crate::items::SubjectKind as ItemKind;
use crate::workflow_runs::WorkflowRunQuery;

/// A notification thread, as returned by `GET /notifications`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub unread: bool,
    /// A Perch-only unread reminder. GitHub's public API can mark a thread
    /// read, but has no inverse operation. Cached threads retain this bit so
    /// the reminder survives restarts and unrelated inbox refreshes.
    #[serde(default)]
    pub local_unread: bool,
    /// Perch marked this revision of the thread read, and GitHub took the
    /// request. GitHub's list can go on reporting the revision unread for
    /// a while after, and a snapshot taken then must not undo what it
    /// accepted: the bit is carried while the revision stands, and goes
    /// with newer activity or Mark Unread.
    #[serde(default)]
    pub local_read: bool,
    pub reason: Reason,
    #[serde(with = "ts")]
    pub updated_at: Timestamp,
    #[serde(default, with = "ts_opt")]
    pub last_read_at: Option<Timestamp>,
    pub subject: Subject,
    pub repository: Repository,
    /// What a look at the pull request or the issue found, once one has
    /// been taken: who opened it, and the state it is in. `None` until
    /// then, and for every other kind of subject.
    #[serde(default)]
    pub subject_snapshot: Option<SubjectSnapshot>,
    /// The run a workflow run notification is about, once a look has
    /// found it. `None` until then, and for every other kind of subject.
    #[serde(default)]
    pub run_snapshot: Option<RunSnapshot>,
    /// When a thread snoozed in Perch comes back to the inbox. Set only on
    /// the threads of Show › Snoozed; the mark itself is kept beside the
    /// Done marks, not in the thread.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "ts_opt")]
    pub snoozed_until: Option<Timestamp>,
}

impl Thread {
    /// The issue or PR number, parsed from the tail of the subject API URL,
    /// which is how a notification is matched to its item.
    pub fn number_hint(&self) -> Option<u64> {
        let url = self.subject.url.as_deref()?;
        url.rsplit('/').next()?.parse().ok()
    }

    /// Whether the thread is the notification about the given pull
    /// request or issue: the one viewing it reads, whichever row it was
    /// opened from, and whose unread state that row shows.
    pub fn is_about(&self, repo_full_name: &str, kind: ItemKind, number: u64) -> bool {
        let same_kind = match kind {
            ItemKind::PullRequest => self.subject.kind == SubjectKind::PullRequest,
            ItemKind::Issue => self.subject.kind == SubjectKind::Issue,
        };
        same_kind
            && self.repository.full_name.eq_ignore_ascii_case(repo_full_name)
            && self.number_hint() == Some(number)
    }

    /// Whether the subject wants looking at: a pull request or an issue
    /// with a number, whose snapshot is missing or was taken before the
    /// thread's latest activity. A pull request's snapshot from before
    /// the look asked for its churn is taken again too.
    pub fn wants_snapshot(&self) -> bool {
        matches!(
            self.subject.kind,
            SubjectKind::PullRequest | SubjectKind::Issue
        ) && self.number_hint().is_some()
            && self.subject_snapshot.as_ref().is_none_or(|snapshot| {
                snapshot.at < self.updated_at
                    || snapshot.participants.is_none()
                    || (self.subject.kind == SubjectKind::PullRequest && snapshot.churn.is_none())
            })
    }

    /// Whether the run wants looking for: a workflow run notification
    /// whose title names one, with no run found yet or one found before
    /// the thread's latest activity.
    pub fn wants_run(&self) -> bool {
        self.subject.kind == SubjectKind::CheckSuite
            && WorkflowRunQuery::from_thread(self).is_some()
            && self
                .run_snapshot
                .as_ref()
                .is_none_or(|run| run.at < self.updated_at)
    }
}

/// What one look at a notification's pull request or issue found: who
/// opened it, the state it was in, and on a pull request its churn and
/// whether a review is still wanted.
///
/// The notifications API says the subject's title and kind and nothing
/// else. This is asked for after the thread arrives, and kept on the
/// thread so its row has a face and a state before the next look.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubjectSnapshot {
    /// Empty when the account that opened it is gone.
    pub author_login: String,
    pub author_avatar_url: String,
    pub state: SubjectState,
    /// A pull request's additions and deletions, as GitHub counts them.
    /// `None` on an issue, and on a snapshot taken before this was asked
    /// for.
    #[serde(default)]
    pub churn: Option<(u32, u32)>,
    /// Whether someone has been asked to review the pull request and has
    /// not yet.
    #[serde(default)]
    pub review_requested: bool,
    /// Who acted on it most recently, most recent first, at most three.
    /// `None` on a snapshot taken before this was asked for.
    #[serde(default)]
    pub participants: Option<Vec<Participant>>,
    /// The thread's `updated_at` when the look was taken. Activity since
    /// can have changed the state, so a thread newer than this is looked
    /// at again.
    #[serde(with = "ts")]
    pub at: Timestamp,
}

/// Someone who acted on a pull request or an issue: opened it, commented
/// on it, reviewed it, or wrote its latest commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Participant {
    pub login: String,
    pub avatar_url: String,
}

/// Where a workflow run notification's run is, once a look has found it.
///
/// A `CheckSuite` notification names its workflow and branch in its title
/// and carries no URL. The run is asked for after the thread arrives and
/// kept on the thread, so its row opens the run itself rather than the
/// repository's Actions page.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub html_url: String,
    /// The thread's `updated_at` when the look was taken. A re-run moves
    /// the thread on, and its run is looked for again.
    #[serde(with = "ts")]
    pub at: Timestamp,
}

/// Why GitHub sent this notification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reason {
    Assign,
    Author,
    Comment,
    CiActivity,
    Invitation,
    Manual,
    Mention,
    ReviewRequested,
    SecurityAlert,
    StateChange,
    Subscribed,
    TeamMention,
    Other(String),
}

impl Reason {
    /// The string GitHub uses on the wire. Serializing this rather than the
    /// variant name keeps a cached thread lossless across a round trip.
    pub fn wire(&self) -> &str {
        match self {
            Reason::Assign => "assign",
            Reason::Author => "author",
            Reason::Comment => "comment",
            Reason::CiActivity => "ci_activity",
            Reason::Invitation => "invitation",
            Reason::Manual => "manual",
            Reason::Mention => "mention",
            Reason::ReviewRequested => "review_requested",
            Reason::SecurityAlert => "security_alert",
            Reason::StateChange => "state_change",
            Reason::Subscribed => "subscribed",
            Reason::TeamMention => "team_mention",
            Reason::Other(s) => s,
        }
    }

    /// The wording GitHub's web UI shows in the right-hand badge.
    pub fn label(&self) -> &str {
        match self {
            Reason::Assign => "assigned",
            Reason::Author => "author",
            Reason::Comment => "commented",
            Reason::CiActivity => "ci activity",
            Reason::Invitation => "invited",
            Reason::Manual => "subscribed",
            Reason::Mention => "mention",
            Reason::ReviewRequested => "review requested",
            Reason::SecurityAlert => "security alert",
            Reason::StateChange => "state change",
            Reason::Subscribed => "subscribed",
            Reason::TeamMention => "team mentioned",
            Reason::Other(s) => s,
        }
    }

}

impl Serialize for Reason {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "assign" => Reason::Assign,
            "author" => Reason::Author,
            "comment" => Reason::Comment,
            "ci_activity" => Reason::CiActivity,
            "invitation" => Reason::Invitation,
            "manual" => Reason::Manual,
            "mention" => Reason::Mention,
            "review_requested" => Reason::ReviewRequested,
            "security_alert" => Reason::SecurityAlert,
            "state_change" => Reason::StateChange,
            "subscribed" => Reason::Subscribed,
            "team_mention" => Reason::TeamMention,
            _ => Reason::Other(s),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub title: String,
    #[serde(rename = "type")]
    pub kind: SubjectKind,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub latest_comment_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    Issue,
    PullRequest,
    Discussion,
    Commit,
    Release,
    CheckSuite,
    RepositoryVulnerabilityAlert,
    Other(String),
}

impl SubjectKind {
    pub fn wire(&self) -> &str {
        match self {
            SubjectKind::Issue => "Issue",
            SubjectKind::PullRequest => "PullRequest",
            SubjectKind::Discussion => "Discussion",
            SubjectKind::Commit => "Commit",
            SubjectKind::Release => "Release",
            SubjectKind::CheckSuite => "CheckSuite",
            SubjectKind::RepositoryVulnerabilityAlert => "RepositoryVulnerabilityAlert",
            SubjectKind::Other(s) => s,
        }
    }
}

impl Serialize for SubjectKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for SubjectKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "Issue" => SubjectKind::Issue,
            "PullRequest" => SubjectKind::PullRequest,
            "Discussion" => SubjectKind::Discussion,
            "Commit" => SubjectKind::Commit,
            "Release" => SubjectKind::Release,
            "CheckSuite" => SubjectKind::CheckSuite,
            "RepositoryVulnerabilityAlert" => SubjectKind::RepositoryVulnerabilityAlert,
            _ => SubjectKind::Other(s),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repository {
    pub full_name: String,
    /// Flattened out of the API's nested `owner` object.
    ///
    /// Serialized back under the same `owner` key as a bare string, and the
    /// deserializer accepts either shape, so a cached thread round-trips
    /// through SQLite without a second wire format.
    #[serde(rename = "owner", default, deserialize_with = "owner_avatar")]
    pub owner_avatar_url: String,
}

/// Whether a face is a GitHub App's, which GitHub draws as a rounded
/// square rather than a circle: a bot's avatar is served from under `/in/`.
pub fn is_app_avatar(url: &str) -> bool {
    url.contains("githubusercontent.com/in/")
}

fn owner_avatar<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape {
        /// What we write to the cache.
        Flat(String),
        /// What GitHub sends.
        Nested {
            #[serde(default)]
            avatar_url: String,
        },
    }
    Ok(match Shape::deserialize(d)? {
        Shape::Flat(url) => url,
        Shape::Nested { avatar_url } => avatar_url,
    })
}

/// The state an issue or pull request is in, as its row and its header
/// draw it. Derived from the item's snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubjectState {
    IssueOpen,
    IssueClosed,
    IssueNotPlanned,
    PrOpen,
    PrDraft,
    PrMerged,
    PrClosed,
}

/// A user as GitHub nests one. Public because a workflow run carries its
/// actor and the run is stored as it came.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPayload {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
    /// `User`, `Organization` or `Bot`, where GitHub says.
    #[serde(default, rename = "type", skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// GitHub emits RFC 3339 with a `Z` suffix, which `jiff::Timestamp` parses.
pub(crate) mod ts {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Timestamp, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Timestamp, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

mod ts_opt {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<Timestamp>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&t.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Timestamp>, D::Error> {
        let s = Option::<String>::deserialize(d)?;
        match s {
            Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_reasons_map_to_variants() {
        let cases = [
            (r#""review_requested""#, Reason::ReviewRequested),
            (r#""team_mention""#, Reason::TeamMention),
            (r#""ci_activity""#, Reason::CiActivity),
            (r#""subscribed""#, Reason::Subscribed),
        ];
        for (json, want) in cases {
            assert_eq!(serde_json::from_str::<Reason>(json).unwrap(), want);
        }
    }

    #[test]
    fn unknown_reason_falls_back_to_other() {
        let r: Reason = serde_json::from_str(r#""brand_new_reason""#).unwrap();
        assert_eq!(r, Reason::Other("brand_new_reason".into()));
        assert_eq!(r.label(), "brand_new_reason");
    }

    #[test]
    fn unknown_subject_kind_falls_back_to_other() {
        let k: SubjectKind = serde_json::from_str(r#""Sponsorship""#).unwrap();
        assert_eq!(k, SubjectKind::Other("Sponsorship".into()));
    }

    #[test]
    fn number_is_parsed_from_subject_url() {
        let t = thread_with_url(Some("https://api.github.com/repos/slatedb/slatedb/issues/2097"));
        assert_eq!(t.number_hint(), Some(2097));
    }

    #[test]
    fn number_hint_is_none_without_a_url() {
        assert_eq!(thread_with_url(None).number_hint(), None);
    }

    #[test]
    fn a_subject_wants_a_look_until_one_is_taken_after_its_latest_activity() {
        let mut t = thread_with_url(Some("https://api.github.com/repos/a/b/issues/9"));
        assert!(t.wants_snapshot());

        // A look taken for this revision of the thread settles it.
        t.subject_snapshot = Some(SubjectSnapshot {
            author_login: "someone".into(),
            author_avatar_url: "https://avatars/2".into(),
            state: SubjectState::IssueOpen,
            churn: None,
            review_requested: false,
            participants: Some(vec![]),
            at: t.updated_at,
        });
        assert!(!t.wants_snapshot());

        // Activity since the look may have closed it.
        t.updated_at = "2026-09-18T18:00:00Z".parse().unwrap();
        assert!(t.wants_snapshot());

        // Only a pull request or an issue has anyone who opened it.
        t.subject.kind = SubjectKind::CheckSuite;
        assert!(!t.wants_snapshot());
        t.subject.kind = SubjectKind::PullRequest;
        t.subject.url = None;
        assert!(!t.wants_snapshot());
    }

    #[test]
    fn a_pull_requests_snapshot_without_its_churn_is_taken_again() {
        let mut t = thread_with_url(Some("https://api.github.com/repos/a/b/pulls/9"));
        t.subject.kind = SubjectKind::PullRequest;
        let mut snapshot = SubjectSnapshot {
            author_login: "someone".into(),
            author_avatar_url: "https://avatars/2".into(),
            state: SubjectState::PrOpen,
            churn: None,
            review_requested: false,
            participants: Some(vec![]),
            at: t.updated_at,
        };
        // Taken by a version that did not ask.
        t.subject_snapshot = Some(snapshot.clone());
        assert!(t.wants_snapshot());
        snapshot.churn = Some((12, 3));
        t.subject_snapshot = Some(snapshot.clone());
        assert!(!t.wants_snapshot());

        // An issue has no churn to wait for.
        t.subject.kind = SubjectKind::Issue;
        snapshot.churn = None;
        t.subject_snapshot = Some(snapshot);
        assert!(!t.wants_snapshot());
    }

    #[test]
    fn a_snapshot_without_its_participants_is_taken_again() {
        let mut t = thread_with_url(Some("https://api.github.com/repos/a/b/issues/9"));
        let mut snapshot = SubjectSnapshot {
            author_login: "someone".into(),
            author_avatar_url: "https://avatars/2".into(),
            state: SubjectState::IssueOpen,
            churn: None,
            review_requested: false,
            participants: None,
            at: t.updated_at,
        };
        // Taken by a version that did not ask.
        t.subject_snapshot = Some(snapshot.clone());
        assert!(t.wants_snapshot());
        // Asked, and nobody there: settled all the same.
        snapshot.participants = Some(vec![]);
        t.subject_snapshot = Some(snapshot);
        assert!(!t.wants_snapshot());
    }

    #[test]
    fn a_snapshot_round_trips_through_the_cache_and_is_absent_from_the_wire() {
        let mut t = thread_with_url(Some("https://api.github.com/repos/a/b/pulls/9"));
        t.subject_snapshot = Some(SubjectSnapshot {
            author_login: "someone".into(),
            author_avatar_url: "https://avatars/2".into(),
            state: SubjectState::PrMerged,
            churn: Some((1192, 36)),
            review_requested: true,
            participants: Some(vec![Participant {
                login: "someone".into(),
                avatar_url: "https://avatars/2".into(),
            }]),
            at: t.updated_at,
        });
        let json = serde_json::to_string(&t).unwrap();
        let back: Thread = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);

        // A snapshot cached before the churn was asked for still reads,
        // with none.
        let mut older: serde_json::Value = serde_json::from_str(&json).unwrap();
        let snapshot = older["subject_snapshot"].as_object_mut().unwrap();
        snapshot.remove("churn");
        snapshot.remove("review_requested");
        snapshot.remove("participants");
        let older: Thread = serde_json::from_value(older).unwrap();
        let snapshot = older.subject_snapshot.unwrap();
        assert_eq!(snapshot.churn, None);
        assert!(!snapshot.review_requested);
        assert_eq!(snapshot.participants, None);

        // GitHub's payload has no such field, and is not refused for it.
        let mut from_wire: serde_json::Value = serde_json::from_str(&json).unwrap();
        from_wire.as_object_mut().unwrap().remove("subject_snapshot");
        let bare: Thread = serde_json::from_value(from_wire).unwrap();
        assert_eq!(bare.subject_snapshot, None);
    }

    #[test]
    fn a_run_wants_looking_for_until_one_is_found_after_the_latest_activity() {
        let mut t = thread_with_url(None);
        t.subject.kind = SubjectKind::CheckSuite;
        t.subject.title = "CI workflow run failed for main branch".into();
        assert!(t.wants_run());

        // A run found for this revision of the thread settles it.
        t.run_snapshot = Some(RunSnapshot {
            html_url: "https://github.com/a/b/actions/runs/1".into(),
            at: t.updated_at,
        });
        assert!(!t.wants_run());

        // A re-run moves the thread on, and is a new attempt to find.
        t.updated_at = "2026-09-18T18:00:00Z".parse().unwrap();
        t.subject.title = "CI workflow run, Attempt #2 failed for main branch".into();
        assert!(t.wants_run());

        // A title that names no run is nothing to look for.
        t.run_snapshot = None;
        t.subject.title = "Something else entirely".into();
        assert!(!t.wants_run());

        // Only a workflow run notification has a run.
        t.subject.title = "CI workflow run failed for main branch".into();
        t.subject.kind = SubjectKind::Issue;
        assert!(!t.wants_run());
    }

    #[test]
    fn a_found_run_round_trips_through_the_cache_and_is_absent_from_the_wire() {
        let mut t = thread_with_url(None);
        t.subject.kind = SubjectKind::CheckSuite;
        t.run_snapshot = Some(RunSnapshot {
            html_url: "https://github.com/a/b/actions/runs/1".into(),
            at: t.updated_at,
        });
        let json = serde_json::to_string(&t).unwrap();
        let back: Thread = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);

        // GitHub's payload has no such field, nor do rows cached before
        // runs were looked for, and neither is refused for it.
        let mut from_wire: serde_json::Value = serde_json::from_str(&json).unwrap();
        from_wire.as_object_mut().unwrap().remove("run_snapshot");
        let bare: Thread = serde_json::from_value(from_wire).unwrap();
        assert_eq!(bare.run_snapshot, None);
    }

    #[test]
    fn a_thread_is_about_its_subject_by_repository_kind_and_number() {
        let issue = thread_with_url(Some("https://api.github.com/repos/a/b/issues/42"));
        assert!(issue.is_about("a/b", ItemKind::Issue, 42));
        assert!(issue.is_about("A/B", ItemKind::Issue, 42), "as GitHub spells it, or not");
        assert!(
            !issue.is_about("a/b", ItemKind::PullRequest, 42),
            "a pull request is not the issue"
        );
        assert!(!issue.is_about("a/b", ItemKind::Issue, 43));
        assert!(!issue.is_about("a/c", ItemKind::Issue, 42));

        let mut pull = thread_with_url(Some("https://api.github.com/repos/a/b/pulls/42"));
        pull.subject.kind = SubjectKind::PullRequest;
        assert!(pull.is_about("a/b", ItemKind::PullRequest, 42));
        assert!(!pull.is_about("a/b", ItemKind::Issue, 42));

        // A subject with no number is about nothing an item can be.
        let mut run = thread_with_url(None);
        run.subject.kind = SubjectKind::CheckSuite;
        assert!(!run.is_about("a/b", ItemKind::PullRequest, 42));
    }

    #[test]
    fn a_workflow_run_notification_names_the_run_to_look_up() {
        let mut t = thread_with_url(None);
        t.subject.kind = SubjectKind::CheckSuite;
        t.subject.title = "Hourly DST workflow run failed for main branch".into();
        assert_eq!(
            WorkflowRunQuery::from_thread(&t),
            Some(WorkflowRunQuery {
                repo_full_name: "a/b".into(),
                workflow: "Hourly DST".into(),
                branch: "main".into(),
                at: t.updated_at,
            })
        );

        // A title that names no run is left alone rather than guessed at.
        t.subject.title = "Something else entirely".into();
        assert_eq!(WorkflowRunQuery::from_thread(&t), None);
    }

    #[test]
    fn full_thread_payload_deserializes() {
        let json = r#"{
            "id": "123",
            "unread": true,
            "reason": "mention",
            "updated_at": "2026-09-17T18:00:00Z",
            "last_read_at": null,
            "subject": {
                "title": "RFC 0035 - incremental compaction",
                "type": "PullRequest",
                "url": "https://api.github.com/repos/slatedb/slatedb/pulls/2091",
                "latest_comment_url": null
            },
            "repository": {
                "full_name": "slatedb/slatedb",
                "owner": { "avatar_url": "https://avatars.githubusercontent.com/u/1" }
            }
        }"#;
        let t: Thread = serde_json::from_str(json).unwrap();
        assert!(!t.local_unread);
        assert_eq!(t.reason, Reason::Mention);
        assert_eq!(t.subject.kind, SubjectKind::PullRequest);
        assert_eq!(t.number_hint(), Some(2091));
        assert_eq!(t.repository.owner_avatar_url, "https://avatars.githubusercontent.com/u/1");

        let mut cached = t;
        cached.local_unread = true;
        let round_tripped: Thread =
            serde_json::from_str(&serde_json::to_string(&cached).unwrap()).unwrap();
        assert!(round_tripped.local_unread);
    }

    #[test]
    fn enums_round_trip_through_their_wire_format() {
        for reason in [
            Reason::ReviewRequested,
            Reason::CiActivity,
            Reason::Subscribed,
            Reason::Other("something_new".into()),
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(serde_json::from_str::<Reason>(&json).unwrap(), reason);
        }

        for kind in [
            SubjectKind::PullRequest,
            SubjectKind::Issue,
            SubjectKind::Other("Sponsorship".into()),
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<SubjectKind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn repository_avatar_round_trips_through_both_shapes() {
        // GitHub's shape.
        let from_api: Repository = serde_json::from_str(
            r#"{"full_name":"a/b","owner":{"avatar_url":"https://avatars/9"}}"#,
        )
        .unwrap();
        assert_eq!(from_api.owner_avatar_url, "https://avatars/9");

        // Our own shape, as written to the cache.
        let json = serde_json::to_string(&from_api).unwrap();
        let round_tripped: Repository = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, from_api);
    }

    #[test]
    fn repository_without_an_owner_is_not_an_error() {
        let r: Repository = serde_json::from_str(r#"{"full_name":"a/b"}"#).unwrap();
        assert_eq!(r.owner_avatar_url, "");
    }

    fn thread_with_url(url: Option<&str>) -> Thread {
        Thread {
            id: "1".into(),
            unread: true,
            local_unread: false,
            local_read: false,
            reason: Reason::Subscribed,
            updated_at: "2026-09-17T18:00:00Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: "t".into(),
                kind: SubjectKind::Issue,
                url: url.map(String::from),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "a/b".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        }
    }
}
