//! Who can be requested to review a pull request, and who may ask them:
//! GitHub lets anyone with triage access or more, or the pull request's
//! author, request reviewers on an open pull request. The pull request's
//! author cannot be requested to review their own pull request.

use serde::Deserialize;

use crate::mentions::MentionableUser;
use crate::pulls::{RepositoryPermission, Reviewer};

/// The most people GitHub lets be requested to review one pull request.
pub const MAX_REVIEWERS: usize = 15;

/// The requested reviewers GitHub answered a change with, whether any
/// review request (including teams) remains pending, and when the pull
/// request last changed by its clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedReviewers {
    pub requested_reviewers: Vec<Reviewer>,
    pub review_requested: bool,
    pub updated_at: Option<jiff::Timestamp>,
}

/// The reader's login and permission on the repository, the pull
/// request's state, author and GitHub's suggested reviewers, and the
/// repository's assignable users, at most `first` of those.
pub(crate) fn requestable_query() -> &'static str {
    "query($owner: String!, $name: String!, $number: Int!, $first: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    viewer_permission: viewerPermission
    assignable_users: assignableUsers(first: $first) {
      nodes { login name avatar_url: avatarUrl }
    }
    pull_request: pullRequest(number: $number) {
      state
      author { login }
      suggested_reviewers: suggestedReviewers {
        reviewer { login name avatar_url: avatarUrl }
      }
    }
  }
}"
}

#[derive(Debug, Deserialize)]
pub(crate) struct RequestableData {
    #[serde(default)]
    viewer: Option<ActorNode>,
    #[serde(default)]
    repository: Option<RepositoryNode>,
}

#[derive(Debug, Deserialize)]
struct ActorNode {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryNode {
    #[serde(default)]
    viewer_permission: Option<String>,
    #[serde(default)]
    assignable_users: Option<UserConnection>,
    #[serde(default)]
    pull_request: Option<PullRequestNode>,
}

#[derive(Debug, Deserialize)]
struct PullRequestNode {
    #[serde(default)]
    state: String,
    #[serde(default)]
    author: Option<ActorNode>,
    #[serde(default)]
    suggested_reviewers: Option<Vec<Option<SuggestedReviewerNode>>>,
}

#[derive(Debug, Deserialize)]
struct SuggestedReviewerNode {
    #[serde(default)]
    reviewer: Option<UserNode>,
}

#[derive(Debug, Deserialize)]
struct UserConnection {
    #[serde(default)]
    nodes: Vec<Option<UserNode>>,
}

#[derive(Debug, Deserialize)]
struct UserNode {
    #[serde(default)]
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: String,
}

impl RequestableData {
    /// Who may be requested to review when the reader may request them,
    /// none when they may not or the pull request is not open. Triage
    /// access and up, or the pull request's author, may request reviews.
    /// GitHub's suggested reviewers come first, then the repository's
    /// assignable users, each once, excluding the pull request's author.
    pub(crate) fn into_requestable(self) -> Option<Vec<MentionableUser>> {
        let repository = self.repository?;
        let pr = repository.pull_request?;
        if !pr.state.eq_ignore_ascii_case("open") {
            return None;
        }
        let author = pr.author.map(|a| a.login).filter(|login| !login.is_empty());
        let viewer = self
            .viewer
            .map(|v| v.login)
            .filter(|login| !login.is_empty());
        let permission = crate::pulls::repository_permission(
            repository.viewer_permission.as_deref().unwrap_or_default(),
        );
        let has_triage = matches!(
            permission,
            RepositoryPermission::Admin
                | RepositoryPermission::Maintain
                | RepositoryPermission::Write
                | RepositoryPermission::Triage
        );
        let is_author = match (&viewer, &author) {
            (Some(v), Some(a)) => v.eq_ignore_ascii_case(a),
            _ => false,
        };
        if !has_triage && !is_author {
            return None;
        }
        let suggested = pr
            .suggested_reviewers
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|s| s.reviewer);
        let assignable = repository
            .assignable_users
            .into_iter()
            .flat_map(|c| c.nodes)
            .flatten();
        let mut users: Vec<MentionableUser> = Vec::new();
        for n in suggested.chain(assignable) {
            if n.login.is_empty()
                || author
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&n.login))
                || users.iter().any(|u| u.login.eq_ignore_ascii_case(&n.login))
            {
                continue;
            }
            users.push(MentionableUser {
                login: n.login,
                name: n.name.filter(|name| !name.trim().is_empty()),
                avatar_url: n.avatar_url,
            });
        }
        Some(users)
    }
}

/// The fields of GitHub's pull request response that `POST` and `DELETE
/// /pulls/{number}/requested_reviewers` update.
#[derive(Debug, Deserialize)]
pub(crate) struct RequestedReviewersPayload {
    #[serde(default)]
    requested_reviewers: Vec<ReviewerRef>,
    #[serde(default)]
    requested_teams: Vec<serde_json::Value>,
    #[serde(default)]
    updated_at: Option<jiff::Timestamp>,
}

#[derive(Debug, Deserialize)]
struct ReviewerRef {
    #[serde(default)]
    login: String,
    #[serde(default)]
    avatar_url: String,
}

impl RequestedReviewersPayload {
    pub(crate) fn into_requested(self) -> RequestedReviewers {
        let requested_reviewers: Vec<Reviewer> = self
            .requested_reviewers
            .into_iter()
            .filter(|u| !u.login.is_empty())
            .map(|u| Reviewer {
                login: u.login,
                avatar_url: u.avatar_url,
            })
            .collect();
        let review_requested = !requested_reviewers.is_empty() || !self.requested_teams.is_empty();
        RequestedReviewers {
            requested_reviewers,
            review_requested,
            updated_at: self.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(permission: &str, viewer: &str, author: &str, state: &str) -> RequestableData {
        serde_json::from_str(&format!(
            r#"{{
                "viewer": {{"login": "{viewer}"}},
                "repository": {{
                    "viewer_permission": "{permission}",
                    "assignable_users": {{"nodes": [
                        {{"login": "criccomini", "name": "Chris", "avatar_url": "https://avatars/1"}},
                        {{"login": "rohan", "name": "  ", "avatar_url": "https://avatars/2"}},
                        null,
                        {{"login": "", "name": null, "avatar_url": ""}}
                    ]}},
                    "pull_request": {{
                        "state": "{state}",
                        "author": {{"login": "{author}"}},
                        "suggested_reviewers": [
                            {{"reviewer": {{"login": "agavra", "name": "Almog", "avatar_url": "https://avatars/3"}}}},
                            {{"reviewer": {{"login": "Criccomini", "name": "Chris", "avatar_url": "https://avatars/1"}}}},
                            {{"reviewer": {{"login": "{author}", "name": "Author", "avatar_url": "https://avatars/0"}}}},
                            {{"reviewer": null}}
                        ]
                    }}
                }}
            }}"#
        ))
        .unwrap()
    }

    #[test]
    fn triage_or_more_may_request_reviewers_on_an_open_pull_request() {
        for permission in ["TRIAGE", "WRITE", "MAINTAIN", "ADMIN"] {
            let users = data(permission, "someone", "cadonna", "OPEN")
                .into_requestable()
                .expect(permission);
            let logins: Vec<&str> = users.iter().map(|u| u.login.as_str()).collect();
            assert_eq!(logins, ["agavra", "Criccomini", "rohan"], "{permission}");
            assert_eq!(users[0].name.as_deref(), Some("Almog"));
            assert_eq!(users[2].name, None, "blank names are dropped");
        }
    }

    #[test]
    fn the_pull_request_author_may_request_reviewers_even_with_read_access() {
        let users = data("READ", "Criccomini", "criccomini", "OPEN")
            .into_requestable()
            .expect("PR author may request reviewers");
        let logins: Vec<&str> = users.iter().map(|u| u.login.as_str()).collect();
        assert_eq!(
            logins,
            ["agavra", "rohan"],
            "author is excluded from candidates"
        );
    }

    #[test]
    fn read_access_non_author_and_closed_pull_requests_cannot_request_reviewers() {
        assert_eq!(
            data("READ", "someone", "cadonna", "OPEN").into_requestable(),
            None
        );
        assert_eq!(
            data("", "someone", "cadonna", "OPEN").into_requestable(),
            None
        );
        assert_eq!(
            data("ADMIN", "cadonna", "cadonna", "CLOSED").into_requestable(),
            None
        );
        assert_eq!(
            data("ADMIN", "cadonna", "cadonna", "MERGED").into_requestable(),
            None
        );
        let missing: RequestableData = serde_json::from_str(r#"{"repository":null}"#).unwrap();
        assert_eq!(missing.into_requestable(), None);
    }

    #[test]
    fn requested_reviewers_payload_tracks_users_and_teams() {
        let payload: RequestedReviewersPayload = serde_json::from_str(
            r#"{
                "requested_reviewers": [
                    {"login": "agavra", "avatar_url": "https://avatars/3"}
                ],
                "requested_teams": [],
                "updated_at": "2026-10-03T10:00:00Z"
            }"#,
        )
        .unwrap();
        let parsed = payload.into_requested();
        assert_eq!(
            parsed.requested_reviewers,
            vec![Reviewer {
                login: "agavra".into(),
                avatar_url: "https://avatars/3".into(),
            }]
        );
        assert!(parsed.review_requested);

        let team_only: RequestedReviewersPayload = serde_json::from_str(
            r#"{
                "requested_reviewers": [],
                "requested_teams": [{"slug": "core"}],
                "updated_at": "2026-10-03T10:00:00Z"
            }"#,
        )
        .unwrap();
        let parsed = team_only.into_requested();
        assert!(parsed.requested_reviewers.is_empty());
        assert!(parsed.review_requested);
    }
}
