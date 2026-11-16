//! Builders the engine's tests and perch-app's share, so the two crates'
//! tests build the same pull request and issue rather than copies that
//! drift apart.

use perch_github::{Issue, PullRequest};

/// An open pull request on slatedb/slatedb, from `feature` into `main`.
pub fn pull_request(number: u64) -> PullRequest {
    PullRequest {
        number,
        title: format!("pull {number}"),
        html_url: format!("https://github.com/slatedb/slatedb/pull/{number}"),
        author_login: "u".into(),
        author_avatar_url: String::new(),
        head_sha: "h".into(),
        head_ref: "feature".into(),
        base_sha: "base".into(),
        base_ref: "main".into(),
        draft: false,
        merged: false,
        state: "open".into(),
        additions: 0,
        deletions: 0,
        changed_files: 0,
        repo_full_name: "slatedb/slatedb".into(),
        head_repo_full_name: Some("slatedb/slatedb".into()),
        updated_at: None,
        review_requested: false,
        checks: None,
        review_decision: None,
        merged_at: None,
        merged_by: None,
        closed_at: None,
        requested_reviewers: Vec::new(),
        milestone: None,
        labels: Vec::new(),
        assignees: Vec::new(),
    }
}

/// Issue 7 on slatedb/slatedb, open, with `body`.
pub fn issue(body: &str) -> Issue {
    serde_json::from_value(serde_json::json!({
        "number": 7,
        "title": "Fence the manifest",
        "body": body,
        "state": "open",
        "updated_at": "2026-09-18T09:59:00Z",
        "repo_full_name": "slatedb/slatedb",
    }))
    .unwrap()
}
