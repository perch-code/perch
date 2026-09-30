//! Reading a file at a commit and committing a change to it on a branch,
//! through GraphQL, as github.com commits an applied suggestion.

use base64::Engine;
use serde::Deserialize;

/// A file's text at an expression such as `<sha>:<path>`.
pub(crate) const FILE_TEXT_QUERY: &str = "query($owner: String!, $name: String!, $expression: String!) { repository(owner: $owner, name: $name) { object(expression: $expression) { ... on Blob { text isBinary isTruncated } } } }";

/// One commit on a branch, refused by GitHub when the branch has moved
/// from the head it was written against.
pub(crate) const COMMIT_ON_BRANCH_MUTATION: &str = "mutation($input: CreateCommitOnBranchInput!) { createCommitOnBranch(input: $input) { commit { oid } } }";

/// Why a file has no text to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    Missing,
    Binary,
    TooLarge,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileTextData {
    repository: Option<FileTextRepository>,
}

#[derive(Debug, Deserialize)]
struct FileTextRepository {
    object: Option<Blob>,
}

#[derive(Debug, Deserialize)]
struct Blob {
    text: Option<String>,
    #[serde(default, rename = "isBinary")]
    is_binary: Option<bool>,
    #[serde(default, rename = "isTruncated")]
    is_truncated: bool,
}

impl FileTextData {
    /// The file's text, or why there is none. An object that is not a
    /// blob, such as a directory, comes back with none of the fields.
    pub(crate) fn into_text(self) -> Result<String, Unreadable> {
        let blob = self
            .repository
            .and_then(|r| r.object)
            .ok_or(Unreadable::Missing)?;
        if blob.is_binary == Some(true) {
            return Err(Unreadable::Binary);
        }
        if blob.is_truncated {
            return Err(Unreadable::TooLarge);
        }
        blob.text.ok_or(Unreadable::Missing)
    }
}

/// The variables for a commit of `contents` to `path` on `branch` of
/// `repo_full_name`, over `expected_head`.
pub(crate) fn commit_variables(
    repo_full_name: &str,
    branch: &str,
    expected_head: &str,
    path: &str,
    contents: &str,
    headline: &str,
    body: &str,
) -> serde_json::Value {
    let mut message = serde_json::json!({ "headline": headline });
    if !body.trim().is_empty() {
        message["body"] = body.into();
    }
    serde_json::json!({
        "input": {
            "branch": { "repositoryNameWithOwner": repo_full_name, "branchName": branch },
            "message": message,
            "expectedHeadOid": expected_head,
            "fileChanges": {
                "additions": [{
                    "path": path,
                    "contents": base64::engine::general_purpose::STANDARD.encode(contents),
                }],
            },
        },
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitOnBranchData {
    #[serde(rename = "createCommitOnBranch")]
    payload: Option<CommitOnBranchPayload>,
}

#[derive(Debug, Deserialize)]
struct CommitOnBranchPayload {
    commit: Option<CommitOid>,
}

#[derive(Debug, Deserialize)]
struct CommitOid {
    oid: String,
}

impl CommitOnBranchData {
    /// The new commit's sha. `None` when GitHub answered without one.
    pub(crate) fn into_oid(self) -> Option<String> {
        Some(self.payload?.commit?.oid)
    }
}
