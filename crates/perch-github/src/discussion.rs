//! The discussion on a pull request: description, conversation, reviews,
//! and the line comments under them.
//!
//! Three list endpoints, each paginated to the end. The shapes here are the
//! few fields the knowledge archive keeps; everything else GitHub sends is
//! ignored on the way in.

use jiff::Timestamp;
use serde::Deserialize;

use crate::pulls::UserRef;

/// Everything said on one pull request, as GitHub lists it.
#[derive(Debug, Clone, Default, PartialEq)]
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
}

#[derive(Debug, Clone, PartialEq)]
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

/// The wire shape: the author is one nested object, read into two fields.
#[derive(Deserialize)]
struct IssueCommentPayload {
    id: u64,
    #[serde(default)]
    user: Option<UserRef>,
    #[serde(default)]
    body: String,
    created_at: Timestamp,
    #[serde(default)]
    updated_at: Option<Timestamp>,
    #[serde(default)]
    html_url: String,
}

impl<'de> serde::Deserialize<'de> for IssueComment {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let p = IssueCommentPayload::deserialize(d)?;
        let (user, user_avatar_url) = p.user.map(|u| (u.login, u.avatar_url)).unwrap_or_default();
        Ok(IssueComment {
            id: p.id,
            user,
            user_avatar_url,
            body: p.body,
            created_at: p.created_at,
            updated_at: p.updated_at,
            html_url: p.html_url,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Review {
    pub id: u64,
    #[serde(default, deserialize_with = "login")]
    pub user: String,
    #[serde(default)]
    pub body: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    #[serde(default)]
    pub state: String,
    /// Absent on a pending review.
    #[serde(default)]
    pub submitted_at: Option<Timestamp>,
    #[serde(default)]
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    #[serde(default)]
    pub pull_request_review_id: Option<u64>,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
    #[serde(default, deserialize_with = "login")]
    pub user: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub path: String,
    /// The line on the head side, or on the base side for a removed line.
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub original_line: Option<u64>,
    pub created_at: Timestamp,
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    #[serde(default)]
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

/// GitHub nests the author under `user`; only the login is wanted.
fn login<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let user: Option<UserRef> = Option::deserialize(d)?;
    Ok(user.map(|u| u.login).unwrap_or_default())
}

/// The description, from the pull request payload.
#[derive(Debug, Deserialize)]
pub(crate) struct BodyOnly {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_comment_reads_its_location_from_the_head_line() {
        let c: ReviewComment = serde_json::from_str(
            r#"{"id":3,"user":{"login":"chris"},"body":"x","path":"src/manifest.rs",
                "line":41,"original_line":40,"created_at":"2026-09-16T11:38:00Z"}"#,
        )
        .unwrap();
        assert_eq!(c.location(), "src/manifest.rs:41");
        assert_eq!(c.user, "chris");
    }

    #[test]
    fn an_outdated_comment_falls_back_to_the_original_line() {
        let c: ReviewComment = serde_json::from_str(
            r#"{"id":3,"body":"x","path":"a.rs","line":null,"original_line":7,
                "created_at":"2026-09-16T11:38:00Z"}"#,
        )
        .unwrap();
        assert_eq!(c.location(), "a.rs:7");
        assert_eq!(c.user, "", "a deleted account is not an error");
    }

    #[test]
    fn a_pending_review_has_no_submitted_time() {
        let r: Review =
            serde_json::from_str(r#"{"id":9,"state":"PENDING","user":{"login":"r"}}"#).unwrap();
        assert_eq!(r.submitted_at, None);
    }
}
