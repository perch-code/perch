//! The pull request endpoint.
//!
//! The notifications API gives a subject URL and little else. Opening a pull
//! request needs the commits to check out and the branches to diff, which
//! only this endpoint carries.

use serde::{Deserialize, Serialize};

/// What Perch needs to check out and describe a pull request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author_login: String,
    pub author_avatar_url: String,
    pub head_sha: String,
    /// Branch name on the head side, for the header.
    pub head_ref: String,
    pub base_sha: String,
    /// Branch the pull request targets. Fetched so a merge base can be found.
    pub base_ref: String,
    pub draft: bool,
    pub merged: bool,
    pub state: String,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    /// `owner/repo` of the repository the pull request lives in.
    pub repo_full_name: String,
    /// `owner/repo` of the repository the head branch is on: the same, or
    /// a fork.
    #[serde(default)]
    pub head_repo_full_name: Option<String>,
    /// When GitHub last saw activity on it.
    #[serde(default)]
    pub updated_at: Option<jiff::Timestamp>,
    /// Whether anyone has been asked to review and nobody has yet.
    #[serde(default)]
    pub review_requested: bool,
    /// The state of the head commit's checks, from the checks API.
    #[serde(default)]
    pub checks: Option<CheckStatus>,
    /// Where the reviews stand, from the reviews list.
    #[serde(default)]
    pub review_decision: Option<ReviewDecision>,
}

/// The CI chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
}

/// The review chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    ReviewRequested,
    ChangesRequested,
    Approved,
}

impl ReviewDecision {
    pub fn label(self) -> &'static str {
        match self {
            ReviewDecision::ReviewRequested => "review requested",
            ReviewDecision::ChangesRequested => "changes requested",
            ReviewDecision::Approved => "approved",
        }
    }
}

impl PullRequest {
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }
}

/// The subset of GitHub's payload we read.
#[derive(Debug, Deserialize)]
pub(crate) struct PullPayload {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub changed_files: u32,
    #[serde(default)]
    pub user: Option<UserRef>,
    pub head: GitRef,
    pub base: GitRef,
    #[serde(default)]
    pub updated_at: Option<jiff::Timestamp>,
    #[serde(default)]
    pub requested_reviewers: Vec<UserRef>,
    #[serde(default)]
    pub requested_teams: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UserRef {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GitRef {
    #[serde(default)]
    pub sha: String,
    #[serde(rename = "ref", default)]
    pub ref_name: String,
    #[serde(default)]
    pub repo: Option<RepoRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RepoRef {
    #[serde(default)]
    pub full_name: String,
}

impl PullPayload {
    pub(crate) fn into_pull_request(self, fallback_repo: &str) -> PullRequest {
        let (author_login, author_avatar_url) = self
            .user
            .map(|u| (u.login, u.avatar_url))
            .unwrap_or_default();

        // The base repo is the one to clone: a pull request from a fork has a
        // different head repo, but `refs/pull/N/head` lives on the base.
        let repo_full_name = self
            .base
            .repo
            .as_ref()
            .map(|r| r.full_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| fallback_repo.to_string());

        let head_repo_full_name = self
            .head
            .repo
            .as_ref()
            .map(|r| r.full_name.clone())
            .filter(|n| !n.is_empty());

        PullRequest {
            number: self.number,
            title: self.title,
            author_login,
            author_avatar_url,
            head_repo_full_name,
            head_sha: self.head.sha,
            head_ref: self.head.ref_name,
            base_sha: self.base.sha,
            base_ref: self.base.ref_name,
            draft: self.draft,
            merged: self.merged,
            state: self.state,
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            repo_full_name,
            updated_at: self.updated_at,
            review_requested: !self.requested_reviewers.is_empty() || !self.requested_teams.is_empty(),
            checks: None,
            review_decision: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "number": 2091,
        "title": "RFC 0035 - incremental compaction",
        "state": "open",
        "draft": false,
        "merged": false,
        "additions": 412,
        "deletions": 87,
        "changed_files": 9,
        "user": { "login": "criccomini", "avatar_url": "https://avatars/1" },
        "head": {
            "sha": "abc123",
            "ref": "feature/incremental",
            "repo": { "full_name": "someone/slatedb" }
        },
        "base": {
            "sha": "def456",
            "ref": "main",
            "repo": { "full_name": "slatedb/slatedb" }
        }
    }"#;

    #[test]
    fn a_pull_request_payload_maps_to_what_a_checkout_needs() {
        let payload: PullPayload = serde_json::from_str(SAMPLE).unwrap();
        let pr = payload.into_pull_request("fallback/repo");

        assert_eq!(pr.number, 2091);
        assert_eq!(pr.head_sha, "abc123");
        assert_eq!(pr.head_ref, "feature/incremental");
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.author_login, "criccomini");
        assert!(pr.is_open());
    }

    #[test]
    fn a_fork_still_clones_the_base_repository() {
        // refs/pull/N/head lives on the base repo, not the contributor's fork.
        let payload: PullPayload = serde_json::from_str(SAMPLE).unwrap();
        let pr = payload.into_pull_request("fallback/repo");
        assert_eq!(pr.repo_full_name, "slatedb/slatedb");
    }

    #[test]
    fn a_missing_base_repo_falls_back_to_the_notification_repository() {
        let json = SAMPLE.replace(r#", "repo": { "full_name": "slatedb/slatedb" }"#, "");
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("slatedb/slatedb");
        assert_eq!(pr.repo_full_name, "slatedb/slatedb");
    }

    #[test]
    fn a_missing_author_is_not_an_error() {
        let json = SAMPLE.replace(r#""user": { "login": "criccomini", "avatar_url": "https://avatars/1" },"#, "");
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("a/b");
        assert_eq!(pr.author_login, "");
    }

    #[test]
    fn a_merged_pull_request_is_not_open() {
        let json = SAMPLE.replace(r#""state": "open""#, r#""state": "closed""#)
            .replace(r#""merged": false"#, r#""merged": true"#);
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("a/b");
        assert!(!pr.is_open());
        assert!(pr.merged);
    }
}
