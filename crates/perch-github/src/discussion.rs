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
use crate::reactions::Reactions;

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
    /// The pull request's or issue's GraphQL node id, which its own
    /// reactions are read and left by. Empty on a row an older build kept.
    #[serde(default)]
    pub node_id: String,
    /// When the pull request was opened.
    pub opened_at: Option<Timestamp>,
    /// The pull request's description, as written.
    pub body: String,
    /// The reactions on the description.
    #[serde(default)]
    pub reactions: Reactions,
    /// The conversation tab, in time order.
    pub comments: Vec<IssueComment>,
    /// Each review's verdict and summary.
    pub reviews: Vec<Review>,
    /// Comments on lines of the diff, each belonging to a review.
    pub review_comments: Vec<ReviewComment>,
    /// What the pull request did, from its own payload.
    pub events: Vec<PullEvent>,
}

impl Discussion {
    /// The node ids of everything here a reaction can be left on: the
    /// body, each comment, and each review with a summary to react to.
    /// An id an older build did not keep is left out.
    pub fn reactable_ids(&self) -> Vec<String> {
        let reviews = self
            .reviews
            .iter()
            .filter(|r| !r.body.trim().is_empty())
            .map(|r| r.node_id.as_str());
        std::iter::once(self.node_id.as_str())
            .chain(self.comments.iter().map(|c| c.node_id.as_str()))
            .chain(reviews)
            .chain(self.review_comments.iter().map(|c| c.node_id.as_str()))
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Put the reactions read for a node on whatever here has that id.
    /// Whether anything did. An empty id names nothing, not the entries
    /// an older build kept without one.
    pub fn set_reactions(&mut self, node_id: &str, reactions: &Reactions) -> bool {
        if node_id.is_empty() {
            return false;
        }
        let mut set = false;
        if self.node_id == node_id {
            self.reactions = reactions.clone();
            set = true;
        }
        for c in self.comments.iter_mut().filter(|c| c.node_id == node_id) {
            c.reactions = reactions.clone();
            set = true;
        }
        for r in self.reviews.iter_mut().filter(|r| r.node_id == node_id) {
            r.reactions = reactions.clone();
            set = true;
        }
        for c in self.review_comments.iter_mut().filter(|c| c.node_id == node_id) {
            c.reactions = reactions.clone();
            set = true;
        }
        set
    }

    /// Everything here with its reactions, by node id, for the view to
    /// key on.
    pub fn reactions_by_id(&self) -> impl Iterator<Item = (&str, &Reactions)> {
        std::iter::once((self.node_id.as_str(), &self.reactions))
            .chain(self.comments.iter().map(|c| (c.node_id.as_str(), &c.reactions)))
            .chain(self.reviews.iter().map(|r| (r.node_id.as_str(), &r.reactions)))
            .chain(self.review_comments.iter().map(|c| (c.node_id.as_str(), &c.reactions)))
            .filter(|(id, _)| !id.is_empty())
    }
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
    /// The GraphQL node id, which reactions are read and left by. Empty on
    /// a row an older build kept.
    #[serde(default)]
    pub node_id: String,
    pub user: String,
    /// The author's avatar, for the issue view.
    pub user_avatar_url: String,
    pub body: String,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
    pub html_url: String,
    #[serde(default)]
    pub reactions: Reactions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: u64,
    /// The GraphQL node id, which reactions on the summary are read and
    /// left by. Empty on a row an older build kept.
    #[serde(default)]
    pub node_id: String,
    pub user: String,
    pub body: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    pub state: String,
    /// Absent on a pending review.
    pub submitted_at: Option<Timestamp>,
    pub html_url: String,
    /// The reactions on the summary.
    #[serde(default)]
    pub reactions: Reactions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    /// The GraphQL node id, which reactions are read and left by. Empty on
    /// a row an older build kept.
    #[serde(default)]
    pub node_id: String,
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
    #[serde(default)]
    pub reactions: Reactions,
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
    pub node_id: String,
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
            node_id: p.node_id,
            user,
            user_avatar_url,
            body: p.body,
            created_at: p.created_at,
            updated_at: p.updated_at,
            html_url: p.html_url,
            reactions: Reactions::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewPayload {
    pub id: u64,
    #[serde(default)]
    pub node_id: String,
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
            node_id: p.node_id,
            user: login(p.user),
            body: p.body,
            state: p.state,
            submitted_at: p.submitted_at,
            html_url: p.html_url,
            reactions: Reactions::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewCommentPayload {
    pub id: u64,
    #[serde(default)]
    pub node_id: String,
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
            node_id: p.node_id,
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
            reactions: Reactions::default(),
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
    pub node_id: String,
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
        assert_eq!(c.node_id, "", "a payload without one is not an error");
        assert!(c.reactions.is_empty(), "the lists do not say; GraphQL does");
    }

    #[test]
    fn reactions_land_on_whatever_carries_their_node_id() {
        let mut d: Discussion = serde_json::from_str(
            r#"{"node_id":"PR_1","opened_at":null,"body":"","comments":[
                {"id":1,"node_id":"IC_1","user":"a","user_avatar_url":"","body":"q","created_at":"2026-09-15T09:14:00Z","updated_at":null,"html_url":""}],
              "reviews":[
                {"id":9,"node_id":"PRR_9","user":"r","body":"nice","state":"APPROVED","submitted_at":null,"html_url":""},
                {"id":10,"node_id":"PRR_10","user":"r","body":"  ","state":"COMMENTED","submitted_at":null,"html_url":""}],
              "review_comments":[
                {"id":3,"node_id":"PRRC_3","pull_request_review_id":9,"in_reply_to_id":null,"user":"r","body":"x","path":"a.rs","line":1,"original_line":null,"diff_hunk":"","side":"right","created_at":"2026-09-16T11:38:00Z","updated_at":null,"html_url":""},
                {"id":4,"pull_request_review_id":9,"in_reply_to_id":3,"user":"r","body":"y","path":"a.rs","line":1,"original_line":null,"diff_hunk":"","side":"right","created_at":"2026-09-16T11:39:00Z","updated_at":null,"html_url":""}],
              "events":[]}"#,
        )
        .unwrap();
        // The review with nothing to react to, and the reply an older build
        // kept without an id, are not asked about.
        assert_eq!(d.reactable_ids(), vec!["PR_1", "IC_1", "PRR_9", "PRRC_3"]);

        let mut hooray = Reactions::default();
        hooray.toggle(crate::reactions::ReactionKind::Hooray, true);
        assert!(d.set_reactions("PRRC_3", &hooray));
        assert!(d.set_reactions("PR_1", &hooray));
        assert!(!d.set_reactions("PRRC_404", &hooray));
        assert!(!d.set_reactions("", &hooray), "a missing id matches nothing");
        assert_eq!(d.review_comments[0].reactions, hooray);
        assert_eq!(d.reactions, hooray);
        assert!(d.review_comments[1].reactions.is_empty());
        assert_eq!(
            d.reactions_by_id().filter(|(_, r)| !r.is_empty()).map(|(id, _)| id).collect::<Vec<_>>(),
            vec!["PR_1", "PRRC_3"]
        );
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
        let mut thumbs = Reactions::default();
        thumbs.toggle(crate::reactions::ReactionKind::ThumbsUp, true);
        let d = Discussion {
            node_id: "PR_1".into(),
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            reactions: thumbs.clone(),
            comments: vec![IssueComment {
                id: 1,
                node_id: "IC_1".into(),
                user: "chris".into(),
                user_avatar_url: "https://avatars/1".into(),
                body: "q".into(),
                created_at: at("2026-09-15T09:14:00Z"),
                updated_at: None,
                html_url: "https://github.com/a/b/pull/1#issuecomment-1".into(),
                reactions: thumbs,
            }],
            reviews: vec![Review {
                id: 9,
                node_id: "PRR_9".into(),
                user: "chris".into(),
                body: "nice".into(),
                state: "APPROVED".into(),
                submitted_at: Some(at("2026-09-16T11:40:00Z")),
                html_url: String::new(),
                reactions: Reactions::default(),
            }],
            review_comments: vec![ReviewComment {
                id: 3,
                node_id: "PRRC_3".into(),
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
                reactions: Reactions::default(),
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
        assert!(json.contains(r#""kind":"thumbs_up""#));
    }

    #[test]
    fn a_discussion_an_older_build_kept_loads_without_reactions() {
        let d: Discussion = serde_json::from_str(
            r#"{"opened_at":null,"body":"b","comments":[
                {"id":1,"user":"a","user_avatar_url":"","body":"q","created_at":"2026-09-15T09:14:00Z","updated_at":null,"html_url":""}],
              "reviews":[],"review_comments":[],"events":[]}"#,
        )
        .unwrap();
        assert_eq!(d.node_id, "");
        assert!(d.reactions.is_empty());
        assert_eq!(d.comments[0].node_id, "");
        assert!(d.comments[0].reactions.is_empty());
        assert!(d.reactable_ids().is_empty());
    }
}
