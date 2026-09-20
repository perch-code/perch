//! The discussion on a pull request: description, conversation, reviews,
//! and the line comments under them.
//!
//! Three list endpoints, each paginated to the end. The shapes here are the
//! few fields the conversation keeps; everything else GitHub sends is
//! ignored on the way in.
//!
//! The domain types are plain data and serialise as themselves: a
//! discussion is stored as one JSON row and must load back whole. GitHub's
//! payloads have their own `*Payload` types, converted on the way in, so
//! the wire shape and the stored shape can drift apart without a migration.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::comments::Side;
use crate::pulls::UserRef;

/// The verdict attached to a submitted pull-request review.
///
/// These names serialize exactly as GitHub's REST endpoint expects. Leaving
/// the event out would create a pending review, which Perch does not expose:
/// its review composer always submits one of these three final decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PullReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl PullReviewEvent {
    /// A summary is optional only for an approval. GitHub rejects empty
    /// comment and change-request reviews with a 422, so catch those before
    /// spending a request or triggering abuse-rate-limit accounting.
    pub fn requires_body(self) -> bool {
        matches!(self, Self::Comment | Self::RequestChanges)
    }
}

/// Everything said on one pull request, as GitHub lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Discussion {
    /// When the pull request was opened.
    pub opened_at: Option<Timestamp>,
    /// The pull request's description, as written.
    pub body: String,
    /// The conversation tab, in time order.
    pub comments: Vec<IssueComment>,
    /// Each review's verdict and summary.
    pub reviews: Vec<Review>,
    /// Comments on lines of the diff, each belonging to a review.
    pub review_comments: Vec<ReviewComment>,
    /// What the pull request did, from its own payload.
    pub events: Vec<PullEvent>,
}

/// One line in the timeline: the pull request was opened, merged, closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullEvent {
    pub kind: PullEventKind,
    pub at: Timestamp,
}

/// What happened. `Reopened`, `Draft` and `Ready` have no source in the
/// pull request payload, which carries only the current state; they exist
/// so the timeline has a shape for them, and none is produced until the
/// timeline endpoint is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullEventKind {
    Opened,
    Merged { by: Option<String> },
    Closed,
    Reopened,
    Draft,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub user: String,
    /// The author's avatar, for the issue view.
    pub user_avatar_url: String,
    pub body: String,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: u64,
    pub user: String,
    pub body: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    pub state: String,
    /// Absent on a pending review.
    pub submitted_at: Option<Timestamp>,
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub pull_request_review_id: Option<u64>,
    pub in_reply_to_id: Option<u64>,
    pub user: String,
    pub body: String,
    pub path: String,
    /// The line on the head side, or on the base side for a removed line.
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    /// The hunk the comment was left on, as GitHub shows it. What the
    /// nested block shows when the lines are not in the checkout.
    pub diff_hunk: String,
    /// Which file `line` counts in. `Left` is a comment on a removed line.
    pub side: Side,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
    pub html_url: String,
}

impl ReviewComment {
    /// `path:line`, with the original line standing in when the comment is
    /// on a line the head no longer has.
    pub fn location(&self) -> String {
        match self.line.or(self.original_line) {
            Some(line) => format!("{}:{line}", self.path),
            None => self.path.clone(),
        }
    }
}

/// The wire shape: the author is one nested object, read into two fields.
#[derive(Debug, Deserialize)]
pub(crate) struct IssueCommentPayload {
    pub id: u64,
    #[serde(default)]
    pub user: Option<UserRef>,
    #[serde(default)]
    pub body: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    #[serde(default)]
    pub html_url: String,
}

impl From<IssueCommentPayload> for IssueComment {
    fn from(p: IssueCommentPayload) -> Self {
        let (user, user_avatar_url) = p.user.map(|u| (u.login, u.avatar_url)).unwrap_or_default();
        IssueComment {
            id: p.id,
            user,
            user_avatar_url,
            body: p.body,
            created_at: p.created_at,
            updated_at: p.updated_at,
            html_url: p.html_url,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewPayload {
    pub id: u64,
    #[serde(default)]
    pub user: Option<UserRef>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub submitted_at: Option<Timestamp>,
    #[serde(default)]
    pub html_url: String,
}

impl From<ReviewPayload> for Review {
    fn from(p: ReviewPayload) -> Self {
        Review {
            id: p.id,
            user: login(p.user),
            body: p.body,
            state: p.state,
            submitted_at: p.submitted_at,
            html_url: p.html_url,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewCommentPayload {
    pub id: u64,
    #[serde(default)]
    pub pull_request_review_id: Option<u64>,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
    #[serde(default)]
    pub user: Option<UserRef>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub original_line: Option<u64>,
    #[serde(default)]
    pub diff_hunk: String,
    /// `LEFT` or `RIGHT`; absent on a comment older than the field.
    #[serde(default)]
    pub side: Option<String>,
    pub created_at: Timestamp,
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    #[serde(default)]
    pub html_url: String,
}

impl From<ReviewCommentPayload> for ReviewComment {
    fn from(p: ReviewCommentPayload) -> Self {
        let side = match p.side.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("left") => Side::Left,
            _ => Side::Right,
        };
        ReviewComment {
            id: p.id,
            pull_request_review_id: p.pull_request_review_id,
            in_reply_to_id: p.in_reply_to_id,
            user: login(p.user),
            body: p.body,
            path: p.path,
            line: p.line,
            original_line: p.original_line,
            diff_hunk: p.diff_hunk,
            side,
            created_at: p.created_at,
            updated_at: p.updated_at,
            html_url: p.html_url,
        }
    }
}

/// GitHub nests the author under `user`; only the login is wanted.
fn login(user: Option<UserRef>) -> String {
    user.map(|u| u.login).unwrap_or_default()
}

/// The part of the pull request payload the discussion reads: the
/// description, and the times the events are made from.
#[derive(Debug, Deserialize)]
pub(crate) struct PullSummary {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merged_at: Option<Timestamp>,
    #[serde(default)]
    pub merged_by: Option<UserRef>,
    #[serde(default)]
    pub closed_at: Option<Timestamp>,
}

impl PullSummary {
    /// The events the payload carries, in time order: opened, then merged
    /// or closed. A merged pull request has a `closed_at` too, and that is
    /// the same moment, so it is not a second event.
    pub(crate) fn events(&self) -> Vec<PullEvent> {
        let mut events = Vec::new();
        if let Some(at) = self.created_at {
            events.push(PullEvent { kind: PullEventKind::Opened, at });
        }
        if self.merged {
            if let Some(at) = self.merged_at {
                let by = self.merged_by.as_ref().map(|u| u.login.clone());
                events.push(PullEvent { kind: PullEventKind::Merged { by }, at });
            }
        } else if let Some(at) = self.closed_at {
            events.push(PullEvent { kind: PullEventKind::Closed, at });
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn a_review_comment_reads_its_location_from_the_head_line() {
        let p: ReviewCommentPayload = serde_json::from_str(
            r#"{"id":3,"user":{"login":"chris"},"body":"x","path":"src/manifest.rs",
                "line":41,"original_line":40,"side":"LEFT","diff_hunk":"@@ -40,3 +40,3 @@",
                "created_at":"2026-09-16T11:38:00Z"}"#,
        )
        .unwrap();
        let c = ReviewComment::from(p);
        assert_eq!(c.location(), "src/manifest.rs:41");
        assert_eq!(c.user, "chris");
        assert_eq!(c.side, Side::Left);
        assert_eq!(c.diff_hunk, "@@ -40,3 +40,3 @@");
    }

    #[test]
    fn an_outdated_comment_falls_back_to_the_original_line() {
        let p: ReviewCommentPayload = serde_json::from_str(
            r#"{"id":3,"body":"x","path":"a.rs","line":null,"original_line":7,
                "created_at":"2026-09-16T11:38:00Z"}"#,
        )
        .unwrap();
        let c = ReviewComment::from(p);
        assert_eq!(c.location(), "a.rs:7");
        assert_eq!(c.user, "", "a deleted account is not an error");
        assert_eq!(c.side, Side::Right, "no side is the head side");
        assert_eq!(c.diff_hunk, "");
    }

    #[test]
    fn a_pending_review_has_no_submitted_time() {
        let p: ReviewPayload =
            serde_json::from_str(r#"{"id":9,"state":"PENDING","user":{"login":"r"}}"#).unwrap();
        let r = Review::from(p);
        assert_eq!(r.submitted_at, None);
        assert_eq!(r.user, "r");
    }

    #[test]
    fn an_issue_comment_reads_its_author_out_of_the_nested_user() {
        let p: IssueCommentPayload = serde_json::from_str(
            r#"{"id":1,"user":{"login":"chris","avatar_url":"https://avatars/1"},"body":"q",
                "created_at":"2026-09-15T09:14:00Z","html_url":"https://github.com/a/b/pull/1#issuecomment-1"}"#,
        )
        .unwrap();
        let c = IssueComment::from(p);
        assert_eq!(c.user, "chris");
        assert_eq!(c.user_avatar_url, "https://avatars/1");
        assert_eq!(c.updated_at, None);
    }

    fn summary(json: &str) -> PullSummary {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn an_open_pull_request_has_only_been_opened() {
        let events = summary(r#"{"created_at":"2026-09-14T14:02:00Z","merged":false}"#).events();
        assert_eq!(events, vec![PullEvent { kind: PullEventKind::Opened, at: at("2026-09-14T14:02:00Z") }]);
    }

    #[test]
    fn a_merged_pull_request_was_merged_by_someone_and_not_also_closed() {
        let events = summary(
            r#"{"created_at":"2026-09-14T14:02:00Z","merged":true,
                "merged_at":"2026-09-17T15:00:00Z","merged_by":{"login":"rohan"},
                "closed_at":"2026-09-17T15:00:00Z"}"#,
        )
        .events();
        assert_eq!(
            events,
            vec![
                PullEvent { kind: PullEventKind::Opened, at: at("2026-09-14T14:02:00Z") },
                PullEvent { kind: PullEventKind::Merged { by: Some("rohan".into()) }, at: at("2026-09-17T15:00:00Z") },
            ]
        );
    }

    #[test]
    fn a_closed_pull_request_was_closed() {
        let events = summary(
            r#"{"created_at":"2026-09-14T14:02:00Z","merged":false,
                "closed_at":"2026-09-17T15:00:00Z","merged_by":null}"#,
        )
        .events();
        assert_eq!(events[1], PullEvent { kind: PullEventKind::Closed, at: at("2026-09-17T15:00:00Z") });
    }

    #[test]
    fn a_discussion_round_trips_through_json() {
        let d = Discussion {
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![IssueComment {
                id: 1,
                user: "chris".into(),
                user_avatar_url: "https://avatars/1".into(),
                body: "q".into(),
                created_at: at("2026-09-15T09:14:00Z"),
                updated_at: None,
                html_url: "https://github.com/a/b/pull/1#issuecomment-1".into(),
            }],
            reviews: vec![Review {
                id: 9,
                user: "chris".into(),
                body: "nice".into(),
                state: "APPROVED".into(),
                submitted_at: Some(at("2026-09-16T11:40:00Z")),
                html_url: String::new(),
            }],
            review_comments: vec![ReviewComment {
                id: 3,
                pull_request_review_id: Some(9),
                in_reply_to_id: None,
                user: "chris".into(),
                body: "epoch".into(),
                path: "src/manifest.rs".into(),
                line: Some(41),
                original_line: None,
                diff_hunk: "@@ -40,3 +40,3 @@".into(),
                side: Side::Left,
                created_at: at("2026-09-16T11:38:00Z"),
                updated_at: Some(at("2026-09-16T11:38:00Z")),
                html_url: String::new(),
            }],
            events: vec![
                PullEvent { kind: PullEventKind::Opened, at: at("2026-09-14T14:02:00Z") },
                PullEvent { kind: PullEventKind::Merged { by: None }, at: at("2026-09-17T15:00:00Z") },
                PullEvent { kind: PullEventKind::Ready, at: at("2026-09-15T15:00:00Z") },
            ],
        };
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Discussion>(&json).unwrap(), d);
        // The stored shape is the domain shape, not GitHub's.
        assert!(json.contains(r#""user":"chris""#));
        assert!(json.contains(r#""side":"left""#));
    }
}
