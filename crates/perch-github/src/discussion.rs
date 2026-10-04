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
use crate::timeline::{PullEvent, PullEventKind};

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
    /// What was done to the pull request or issue, as GitHub's timeline
    /// lists it, with the opening the timeline has no line for.
    pub events: Vec<PullEvent>,
    /// What GitHub says of each review thread: resolved or not, outdated or
    /// not. Read by a query of its own, so a discussion whose lists came
    /// may have none, as one an older build kept has none.
    #[serde(default)]
    pub review_threads: Vec<ReviewThreadState>,
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

    /// Mark whether the reader may delete the comment with this node id,
    /// in the conversation or on a line. An empty id names nothing.
    pub fn set_deletable(&mut self, node_id: &str, deletable: bool) {
        if node_id.is_empty() {
            return;
        }
        for c in self.comments.iter_mut().filter(|c| c.node_id == node_id) {
            c.viewer_can_delete = deletable;
        }
        for c in self.review_comments.iter_mut().filter(|c| c.node_id == node_id) {
            c.viewer_can_delete = deletable;
        }
    }

    /// Mark whether the reader may edit the comment with this node id,
    /// in the conversation or on a line. An empty id names nothing.
    pub fn set_updatable(&mut self, node_id: &str, updatable: bool) {
        if node_id.is_empty() {
            return;
        }
        for c in self.comments.iter_mut().filter(|c| c.node_id == node_id) {
            c.viewer_can_update = updatable;
        }
        for c in self.review_comments.iter_mut().filter(|c| c.node_id == node_id) {
            c.viewer_can_update = updatable;
        }
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

/// A review thread as GitHub keeps it: whether someone resolved it, and
/// whether the lines it was left on are gone from the head. Keyed by the
/// thread's first comment, which is the only id the REST list shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThreadState {
    /// The thread's GraphQL id, which it is resolved by.
    pub node_id: String,
    pub root_comment_id: u64,
    pub resolved: bool,
    pub outdated: bool,
    /// Whether GitHub lets the reader resolve the thread and unresolve
    /// it. False on a row an older build kept, which did not ask.
    #[serde(default)]
    pub can_resolve: bool,
}

/// Where a comment was left, which names the mutations that edit and
/// delete it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPlace {
    /// In the conversation: an issue comment.
    Conversation,
    /// On a line of a pull request's diff: a review comment.
    Line,
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
    /// Whether GitHub lets the reader delete the comment: its author
    /// may, and so may anyone with write access to the repository. Read
    /// with the reactions; false until they are, and on a row an older
    /// build kept.
    #[serde(default)]
    pub viewer_can_delete: bool,
    /// Whether GitHub lets the reader edit the comment, which goes as
    /// deleting does: read with the reactions, false until they are.
    #[serde(default)]
    pub viewer_can_update: bool,
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

/// A comment that opens a thread on a line, as the review it is submitted
/// with carries it. `start_line` and `start_side` are sent only when the
/// comment is on more than one line, as GitHub expects, and the sides go
/// as `LEFT` and `RIGHT`, its spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewCommentDraft {
    pub path: String,
    pub line: u64,
    #[serde(serialize_with = "side_upper")]
    pub side: Side,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "optional_side_upper"
    )]
    pub start_side: Option<Side>,
    pub body: String,
}

fn side_upper<S: serde::Serializer>(side: &Side, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match side {
        Side::Left => "LEFT",
        Side::Right => "RIGHT",
    })
}

fn optional_side_upper<S: serde::Serializer>(
    side: &Option<Side>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match side {
        Some(side) => side_upper(side, serializer),
        None => serializer.serialize_none(),
    }
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
    /// The author's avatar. Empty on a row an older build kept.
    #[serde(default)]
    pub user_avatar_url: String,
    pub body: String,
    pub path: String,
    /// The line on the head side, or on the base side for a removed line.
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    /// The first line of a range, when the comment is on more than one.
    #[serde(default)]
    pub start_line: Option<u64>,
    #[serde(default)]
    pub original_start_line: Option<u64>,
    /// The hunk the comment was left on, as GitHub shows it. What the
    /// nested block shows when the lines are not in the checkout.
    pub diff_hunk: String,
    /// Which file `line` counts in. `Left` is a comment on a removed line.
    pub side: Side,
    #[serde(default)]
    pub start_side: Option<Side>,
    /// The head the comment is on now, and the head it was left on. Empty
    /// on a row an older build kept.
    #[serde(default)]
    pub commit_id: String,
    #[serde(default)]
    pub original_commit_id: String,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
    pub html_url: String,
    #[serde(default)]
    pub reactions: Reactions,
    /// Whether GitHub lets the reader delete the comment: its author
    /// may, and so may anyone with write access to the repository. Read
    /// with the reactions; false until they are, and on a row an older
    /// build kept.
    #[serde(default)]
    pub viewer_can_delete: bool,
    /// Whether GitHub lets the reader edit the comment, which goes as
    /// deleting does: read with the reactions, false until they are.
    #[serde(default)]
    pub viewer_can_update: bool,
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
            viewer_can_delete: false,
            viewer_can_update: false,
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
    pub start_line: Option<u64>,
    #[serde(default)]
    pub original_start_line: Option<u64>,
    #[serde(default)]
    pub diff_hunk: String,
    /// `LEFT` or `RIGHT`; absent on a comment older than the field.
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub start_side: Option<String>,
    #[serde(default)]
    pub commit_id: String,
    #[serde(default)]
    pub original_commit_id: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    #[serde(default)]
    pub html_url: String,
}

impl From<ReviewCommentPayload> for ReviewComment {
    fn from(p: ReviewCommentPayload) -> Self {
        let side = p.side.as_deref().map(parse_side).unwrap_or(Side::Right);
        let start_side = p.start_side.as_deref().map(parse_side);
        let (user, user_avatar_url) = p.user.map(|u| (u.login, u.avatar_url)).unwrap_or_default();
        ReviewComment {
            id: p.id,
            node_id: p.node_id,
            pull_request_review_id: p.pull_request_review_id,
            in_reply_to_id: p.in_reply_to_id,
            user,
            user_avatar_url,
            body: p.body,
            path: p.path,
            line: p.line,
            original_line: p.original_line,
            start_line: p.start_line,
            original_start_line: p.original_start_line,
            diff_hunk: p.diff_hunk,
            side,
            start_side,
            commit_id: p.commit_id,
            original_commit_id: p.original_commit_id,
            created_at: p.created_at,
            updated_at: p.updated_at,
            html_url: p.html_url,
            reactions: Reactions::default(),
            viewer_can_delete: false,
            viewer_can_update: false,
        }
    }
}

/// GitHub nests the author under `user`; only the login is wanted.
fn login(user: Option<UserRef>) -> String {
    user.map(|u| u.login).unwrap_or_default()
}

/// `LEFT` is the base side; anything else, the head.
fn parse_side(s: &str) -> Side {
    if s.eq_ignore_ascii_case("left") {
        Side::Left
    } else {
        Side::Right
    }
}

/// One page of a pull request's review threads, with the first comment of
/// each so the thread can be matched to the REST list's root.
pub(crate) const REVIEW_THREADS_QUERY: &str = r#"
query ReviewThreads($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pull_request: pullRequest(number: $number) {
      review_threads: reviewThreads(first: 100, after: $cursor) {
        page_info: pageInfo {
          has_next_page: hasNextPage
          end_cursor: endCursor
        }
        nodes {
          id
          is_resolved: isResolved
          is_outdated: isOutdated
          viewer_can_resolve: viewerCanResolve
          viewer_can_unresolve: viewerCanUnresolve
          comments(first: 1) {
            nodes {
              database_id: databaseId
            }
          }
        }
      }
    }
  }
}
"#;

/// The most pages of review threads read for one pull request.
pub(crate) const REVIEW_THREADS_MAX_PAGES: usize = 10;

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadsData {
    #[serde(default)]
    repository: Option<ReviewThreadsRepositoryPayload>,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadsRepositoryPayload {
    #[serde(default)]
    pull_request: Option<ReviewThreadsPullPayload>,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadsPullPayload {
    #[serde(default)]
    review_threads: Option<ReviewThreadsConnection>,
}

#[derive(Debug, Default, Deserialize)]
struct ReviewThreadsConnection {
    #[serde(default)]
    page_info: ReviewThreadsPageInfo,
    #[serde(default)]
    nodes: Vec<Option<ReviewThreadPayload>>,
}

#[derive(Debug, Default, Deserialize)]
struct ReviewThreadsPageInfo {
    #[serde(default)]
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadPayload {
    #[serde(default)]
    id: String,
    #[serde(default)]
    is_resolved: bool,
    #[serde(default)]
    is_outdated: bool,
    #[serde(default)]
    viewer_can_resolve: bool,
    #[serde(default)]
    viewer_can_unresolve: bool,
    #[serde(default)]
    comments: ReviewThreadCommentsPayload,
}

#[derive(Debug, Default, Deserialize)]
struct ReviewThreadCommentsPayload {
    #[serde(default)]
    nodes: Vec<Option<ReviewThreadCommentPayload>>,
}

#[derive(Debug, Deserialize)]
struct ReviewThreadCommentPayload {
    #[serde(default)]
    database_id: Option<u64>,
}

impl ReviewThreadsData {
    /// The threads on the page, and the cursor of the next page if there
    /// is one. `None` when GitHub answered without the pull request. A
    /// thread whose first comment has no id cannot be matched and is left
    /// out.
    pub(crate) fn into_page(self) -> Option<(Vec<ReviewThreadState>, Option<String>)> {
        let connection = self.repository?.pull_request?.review_threads.unwrap_or_default();
        let threads = connection
            .nodes
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let root_comment_id = t.comments.nodes.into_iter().flatten().next()?.database_id?;
                Some(ReviewThreadState {
                    node_id: t.id,
                    root_comment_id,
                    resolved: t.is_resolved,
                    outdated: t.is_outdated,
                    // GitHub may answer only for the way the thread can
                    // go from where it is; whoever may take it one way
                    // may take it back.
                    can_resolve: t.viewer_can_resolve || t.viewer_can_unresolve,
                })
            })
            .collect();
        let next = connection
            .page_info
            .end_cursor
            .filter(|_| connection.page_info.has_next_page);
        Some((threads, next))
    }
}

/// Resolve a review thread, or unresolve it, and read back whether it is
/// resolved after.
pub(crate) fn resolve_thread_mutation(resolve: bool) -> String {
    let field = if resolve {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    format!(
        "mutation($thread: ID!) {{ {field}(input: {{threadId: $thread}}) {{ thread {{ isResolved }} }} }}"
    )
}

/// The mutation's answer: the thread as it stands after.
#[derive(Debug, Deserialize)]
pub(crate) struct ResolveThreadData {
    #[serde(default, rename = "resolveReviewThread", alias = "unresolveReviewThread")]
    payload: Option<ResolveThreadPayload>,
}

#[derive(Debug, Deserialize)]
struct ResolveThreadPayload {
    #[serde(default)]
    thread: Option<ResolvedThreadPayload>,
}

#[derive(Debug, Deserialize)]
struct ResolvedThreadPayload {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
}

impl ResolveThreadData {
    /// Whether the thread is resolved now. `None` when GitHub answered
    /// without the thread.
    pub(crate) fn into_resolved(self) -> Option<bool> {
        Some(self.payload?.thread?.is_resolved)
    }
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
    pub user: Option<UserRef>,
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
    ///
    /// GitHub's timeline has no line for the opening, so that one is only
    /// ever from here; the other two are here as well so a pull request
    /// whose timeline could not be read still says how it ended.
    pub(crate) fn events(&self) -> Vec<PullEvent> {
        let login = |user: &Option<UserRef>| {
            user.as_ref().map(|u| u.login.clone()).unwrap_or_default()
        };
        let mut events = Vec::new();
        if let Some(at) = self.created_at {
            events.push(PullEvent::new(PullEventKind::Opened, at, login(&self.user)));
        }
        if self.merged {
            if let Some(at) = self.merged_at {
                events.push(PullEvent::new(
                    PullEventKind::Merged,
                    at,
                    login(&self.merged_by),
                ));
            }
        } else if let Some(at) = self.closed_at {
            events.push(PullEvent::new(PullEventKind::Closed, at, String::new()));
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
            r#"{"id":3,"user":{"login":"chris","avatar_url":"https://avatars/1"},"body":"x","path":"src/manifest.rs",
                "line":41,"original_line":40,"start_line":39,"original_start_line":38,
                "side":"LEFT","start_side":"RIGHT","diff_hunk":"@@ -40,3 +40,3 @@",
                "commit_id":"head","original_commit_id":"orig",
                "created_at":"2026-09-16T11:38:00Z"}"#,
        )
        .unwrap();
        let c = ReviewComment::from(p);
        assert_eq!(c.location(), "src/manifest.rs:41");
        assert_eq!(c.user, "chris");
        assert_eq!(c.user_avatar_url, "https://avatars/1");
        assert_eq!(c.side, Side::Left);
        assert_eq!(c.start_side, Some(Side::Right));
        assert_eq!((c.start_line, c.original_start_line), (Some(39), Some(38)));
        assert_eq!((c.commit_id.as_str(), c.original_commit_id.as_str()), ("head", "orig"));
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
        assert_eq!(c.start_side, None);
        assert_eq!(c.diff_hunk, "");
        assert_eq!(c.commit_id, "");
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
        assert_eq!(events, vec![PullEvent::new(PullEventKind::Opened, at("2026-09-14T14:02:00Z"), "")]);
    }

    #[test]
    fn a_merged_pull_request_was_merged_by_someone_and_not_also_closed() {
        let events = summary(
            r#"{"created_at":"2026-09-14T14:02:00Z","user":{"login":"chris"},"merged":true,
                "merged_at":"2026-09-17T15:00:00Z","merged_by":{"login":"rohan"},
                "closed_at":"2026-09-17T15:00:00Z"}"#,
        )
        .events();
        assert_eq!(
            events,
            vec![
                PullEvent::new(PullEventKind::Opened, at("2026-09-14T14:02:00Z"), "chris"),
                PullEvent::new(PullEventKind::Merged, at("2026-09-17T15:00:00Z"), "rohan"),
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
        assert_eq!(events[1], PullEvent::new(PullEventKind::Closed, at("2026-09-17T15:00:00Z"), ""));
    }

    #[test]
    fn a_review_threads_page_reads_each_thread_by_its_first_comment() {
        let data: ReviewThreadsData = serde_json::from_str(
            r#"{"repository":{"pull_request":{"review_threads":{
                "page_info":{"has_next_page":true,"end_cursor":"c2"},
                "nodes":[
                  {"id":"PRRT_1","is_resolved":true,"is_outdated":false,"comments":{"nodes":[{"database_id":3}]}},
                  {"id":"PRRT_2","is_resolved":false,"is_outdated":true,"comments":{"nodes":[]}},
                  null
                ]}}}}"#,
        )
        .unwrap();
        let (threads, next) = data.into_page().unwrap();
        assert_eq!(
            threads,
            vec![ReviewThreadState {
                node_id: "PRRT_1".into(),
                root_comment_id: 3,
                resolved: true,
                outdated: false,
                can_resolve: false,
            }]
        );
        assert_eq!(next.as_deref(), Some("c2"));

        let last: ReviewThreadsData = serde_json::from_str(
            r#"{"repository":{"pull_request":{"review_threads":{
                "page_info":{"has_next_page":false,"end_cursor":"c3"},"nodes":[]}}}}"#,
        )
        .unwrap();
        assert_eq!(last.into_page().unwrap(), (vec![], None));
        let gone: ReviewThreadsData = serde_json::from_str(r#"{"repository":null}"#).unwrap();
        assert!(gone.into_page().is_none());
    }

    /// GitHub says whether the reader may resolve a thread that is open
    /// and whether they may unresolve one that is resolved; either is
    /// leave to do both.
    #[test]
    fn a_thread_the_reader_may_resolve_or_unresolve_can_be_resolved() {
        let data: ReviewThreadsData = serde_json::from_str(
            r#"{"repository":{"pull_request":{"review_threads":{
                "page_info":{"has_next_page":false,"end_cursor":null},
                "nodes":[
                  {"id":"PRRT_1","is_resolved":false,"is_outdated":false,
                   "viewer_can_resolve":true,"viewer_can_unresolve":false,
                   "comments":{"nodes":[{"database_id":1}]}},
                  {"id":"PRRT_2","is_resolved":true,"is_outdated":false,
                   "viewer_can_resolve":false,"viewer_can_unresolve":true,
                   "comments":{"nodes":[{"database_id":2}]}},
                  {"id":"PRRT_3","is_resolved":false,"is_outdated":false,
                   "viewer_can_resolve":false,"viewer_can_unresolve":false,
                   "comments":{"nodes":[{"database_id":3}]}}
                ]}}}}"#,
        )
        .unwrap();
        let (threads, _) = data.into_page().unwrap();
        let can: Vec<bool> = threads.iter().map(|t| t.can_resolve).collect();
        assert_eq!(can, vec![true, true, false]);
        // The names read are the ones the query asks for.
        assert!(REVIEW_THREADS_QUERY.contains("viewer_can_resolve: viewerCanResolve"));
        assert!(REVIEW_THREADS_QUERY.contains("viewer_can_unresolve: viewerCanUnresolve"));

        // A state kept by a build that did not ask reads as not allowed.
        let kept: ReviewThreadState = serde_json::from_str(
            r#"{"node_id":"PRRT_1","root_comment_id":1,"resolved":false,"outdated":false}"#,
        )
        .unwrap();
        assert!(!kept.can_resolve);
    }

    #[test]
    fn resolving_and_unresolving_are_two_mutations_that_answer_with_the_thread() {
        let resolve = resolve_thread_mutation(true);
        assert!(resolve.contains("resolveReviewThread(input: {threadId: $thread})"), "{resolve}");
        assert!(!resolve.contains("unresolve"), "{resolve}");
        assert!(resolve_thread_mutation(false).contains("unresolveReviewThread("));

        let resolved: ResolveThreadData = serde_json::from_str(
            r#"{"resolveReviewThread":{"thread":{"isResolved":true}}}"#,
        )
        .unwrap();
        assert_eq!(resolved.into_resolved(), Some(true));
        let unresolved: ResolveThreadData = serde_json::from_str(
            r#"{"unresolveReviewThread":{"thread":{"isResolved":false}}}"#,
        )
        .unwrap();
        assert_eq!(unresolved.into_resolved(), Some(false));
        let empty: ResolveThreadData =
            serde_json::from_str(r#"{"resolveReviewThread":null}"#).unwrap();
        assert_eq!(empty.into_resolved(), None);
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
                viewer_can_delete: false,
                viewer_can_update: false,
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
                user_avatar_url: "https://avatars/1".into(),
                body: "epoch".into(),
                path: "src/manifest.rs".into(),
                line: Some(41),
                original_line: None,
                start_line: Some(40),
                original_start_line: None,
                diff_hunk: "@@ -40,3 +40,3 @@".into(),
                side: Side::Left,
                start_side: Some(Side::Left),
                commit_id: "abc".into(),
                original_commit_id: "abc".into(),
                created_at: at("2026-09-16T11:38:00Z"),
                updated_at: Some(at("2026-09-16T11:38:00Z")),
                html_url: String::new(),
                reactions: Reactions::default(),
                viewer_can_delete: false,
                viewer_can_update: false,
            }],
            events: vec![
                PullEvent::new(PullEventKind::Opened, at("2026-09-14T14:02:00Z"), "rohan"),
                PullEvent::new(PullEventKind::Merged, at("2026-09-17T15:00:00Z"), ""),
                PullEvent::new(PullEventKind::Ready, at("2026-09-15T15:00:00Z"), ""),
            ],
            review_threads: vec![ReviewThreadState {
                node_id: "PRRT_1".into(),
                root_comment_id: 3,
                resolved: true,
                outdated: false,
                can_resolve: true,
            }],
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
              "reviews":[],"review_comments":[
                {"id":3,"pull_request_review_id":9,"in_reply_to_id":null,"user":"rohan","body":"epoch?",
                 "path":"src/lib.rs","line":41,"original_line":40,"diff_hunk":"@@ -40,3 +40,3 @@",
                 "side":"right","created_at":"2026-09-16T11:38:00Z","updated_at":null,"html_url":""}],
              "events":[]}"#,
        )
        .unwrap();
        assert_eq!(d.node_id, "");
        assert!(d.reactions.is_empty());
        assert_eq!(d.comments[0].node_id, "");
        assert!(d.comments[0].reactions.is_empty());
        assert!(d.reactable_ids().is_empty());
        assert!(d.review_threads.is_empty());
        let rc = &d.review_comments[0];
        assert_eq!((rc.id, rc.line, rc.side), (3, Some(41), Side::Right));
        assert_eq!(rc.user_avatar_url, "");
        assert_eq!((rc.start_line, rc.original_start_line, rc.start_side), (None, None, None));
        assert_eq!((rc.commit_id.as_str(), rc.original_commit_id.as_str()), ("", ""));
    }
}
