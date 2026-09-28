//! The issues and pull requests a `#` can name in a repository, as
//! GitHub's own comment box suggests them: the recently updated ones
//! before anything is typed, the ones whose title has the letters after,
//! and the one with the number when digits are typed.

use serde::Deserialize;

use crate::model::SubjectState;

/// An issue or a pull request a `#` can name, as the suggestion list
/// shows it: the state glyph, the title, and the number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueReference {
    pub number: u64,
    pub title: String,
    pub state: SubjectState,
}

/// The search for `query` in `repo_full_name`: the most recently updated
/// with nothing typed, and the titles that have the words otherwise.
pub(crate) fn search_terms(repo_full_name: &str, query: &str) -> String {
    if query.is_empty() {
        format!("repo:{repo_full_name} sort:updated-desc")
    } else {
        format!("repo:{repo_full_name} in:title {query}")
    }
}

/// The search, and the issue or pull request with the number when the
/// query is one.
pub(crate) fn issue_references_query() -> &'static str {
    "query($owner: String!, $name: String!, $search: String!, $first: Int!, $number: Int!, $byNumber: Boolean!) {
  repository(owner: $owner, name: $name) {
    issueOrPullRequest(number: $number) @include(if: $byNumber) {
      ...Reference
    }
  }
  search(query: $search, type: ISSUE, first: $first) {
    nodes { ...Reference }
  }
}
fragment Reference on IssueOrPullRequest {
  __typename
  ... on Issue { number title state stateReason }
  ... on PullRequest { number title state isDraft }
}"
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueReferencesData {
    #[serde(default)]
    repository: Option<RepositoryNode>,
    #[serde(default)]
    search: Option<SearchNode>,
}

#[derive(Debug, Deserialize)]
struct RepositoryNode {
    #[serde(default, rename = "issueOrPullRequest")]
    issue_or_pull_request: Option<ReferenceNode>,
}

#[derive(Debug, Deserialize)]
struct SearchNode {
    #[serde(default)]
    nodes: Vec<Option<ReferenceNode>>,
}

#[derive(Debug, Deserialize)]
struct ReferenceNode {
    #[serde(default, rename = "__typename")]
    typename: String,
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "stateReason")]
    state_reason: Option<String>,
    #[serde(default, rename = "isDraft")]
    is_draft: bool,
}

impl ReferenceNode {
    fn into_reference(self) -> Option<IssueReference> {
        let state = match (self.typename.as_str(), self.state.as_str()) {
            ("PullRequest", "MERGED") => SubjectState::PrMerged,
            ("PullRequest", "CLOSED") => SubjectState::PrClosed,
            ("PullRequest", _) if self.is_draft => SubjectState::PrDraft,
            ("PullRequest", _) => SubjectState::PrOpen,
            ("Issue", "CLOSED") => match self.state_reason.as_deref() {
                Some("NOT_PLANNED" | "DUPLICATE") => SubjectState::IssueNotPlanned,
                _ => SubjectState::IssueClosed,
            },
            ("Issue", _) => SubjectState::IssueOpen,
            _ => return None,
        };
        Some(IssueReference {
            number: self.number?,
            title: self.title,
            state,
        })
    }
}

impl IssueReferencesData {
    /// The one with the number first, then the search's in GitHub's
    /// order, each once. A node GitHub could not resolve is left out.
    pub(crate) fn into_references(self) -> Vec<IssueReference> {
        let by_number = self.repository.and_then(|r| r.issue_or_pull_request);
        let found = self.search.map(|s| s.nodes).unwrap_or_default();
        let mut out: Vec<IssueReference> = Vec::new();
        for node in by_number.into_iter().chain(found.into_iter().flatten()) {
            if let Some(reference) = node.into_reference() {
                if !out.iter().any(|r| r.number == reference.number) {
                    out.push(reference);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_number_comes_first_then_the_search_each_once() {
        let data: IssueReferencesData = serde_json::from_str(
            r#"{
              "repository": {"issueOrPullRequest":
                {"__typename":"PullRequest","number":12,"title":"Fence","state":"OPEN","isDraft":true}},
              "search": {"nodes": [
                {"__typename":"Issue","number":120,"title":"Leak","state":"CLOSED","stateReason":"NOT_PLANNED"},
                null,
                {"__typename":"PullRequest","number":12,"title":"Fence","state":"OPEN","isDraft":true},
                {"__typename":"PullRequest","number":121,"title":"Merge","state":"MERGED","isDraft":false},
                {"__typename":"Issue","number":122,"title":"Done","state":"CLOSED","stateReason":"COMPLETED"},
                {"__typename":"Discussion","number":7,"title":"Other"}
              ]}
            }"#,
        )
        .unwrap();
        let got: Vec<(u64, SubjectState)> = data
            .into_references()
            .into_iter()
            .map(|r| (r.number, r.state))
            .collect();
        assert_eq!(
            got,
            [
                (12, SubjectState::PrDraft),
                (120, SubjectState::IssueNotPlanned),
                (121, SubjectState::PrMerged),
                (122, SubjectState::IssueClosed),
            ]
        );
    }

    #[test]
    fn a_number_nobody_has_leaves_the_search() {
        let data: IssueReferencesData = serde_json::from_str(
            r#"{"repository":{"issueOrPullRequest":null},
                "search":{"nodes":[{"__typename":"Issue","number":3,"title":"A","state":"OPEN"}]}}"#,
        )
        .unwrap();
        assert_eq!(data.into_references().len(), 1);
    }

    #[test]
    fn nothing_typed_lists_the_recently_updated() {
        assert_eq!(search_terms("a/b", ""), "repo:a/b sort:updated-desc");
        assert_eq!(search_terms("a/b", "rfc"), "repo:a/b in:title rfc");
    }
}
