//! GitHub's issue search, which answers for pull requests too: what
//! Browse's views list.

use jiff::Timestamp;
use serde::Deserialize;

use crate::items::SubjectKind;
use crate::model::SubjectState;
use crate::pulls::UserRef;

/// GitHub serves at most the first thousand results of a search.
pub const SEARCH_RESULT_CAP: u64 = 1000;

/// One pull request or issue a search found, as a Browse row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub kind: SubjectKind,
    pub repo_full_name: String,
    pub number: u64,
    pub title: String,
    /// `open` or `closed`.
    pub state: String,
    pub state_reason: Option<String>,
    pub draft: bool,
    pub merged: bool,
    pub author_login: String,
    pub author_avatar_url: String,
    pub updated_at: Timestamp,
    pub html_url: String,
}

impl SearchHit {
    pub fn is_open(&self) -> bool {
        self.state == "open"
    }

    /// The state glyph the row draws, as an item's would be.
    pub fn subject_state(&self) -> SubjectState {
        match (self.kind, self.is_open()) {
            (SubjectKind::PullRequest, true) if self.draft => SubjectState::PrDraft,
            (SubjectKind::PullRequest, true) => SubjectState::PrOpen,
            (SubjectKind::PullRequest, false) if self.merged => SubjectState::PrMerged,
            (SubjectKind::PullRequest, false) => SubjectState::PrClosed,
            (SubjectKind::Issue, true) => SubjectState::IssueOpen,
            (SubjectKind::Issue, false) if self.state_reason.as_deref() == Some("not_planned") => {
                SubjectState::IssueNotPlanned
            }
            (SubjectKind::Issue, false) => SubjectState::IssueClosed,
        }
    }
}

/// One page of a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Whether a next page has anything on it.
    pub more: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchPayload {
    #[serde(default)]
    total_count: u64,
    #[serde(default)]
    items: Vec<SearchItemPayload>,
}

#[derive(Debug, Deserialize)]
struct SearchItemPayload {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    user: Option<UserRef>,
    updated_at: Timestamp,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    repository_url: String,
    #[serde(default)]
    pull_request: Option<PullRef>,
}

#[derive(Debug, Deserialize)]
struct PullRef {
    #[serde(default)]
    merged_at: Option<Timestamp>,
}

impl SearchPayload {
    /// The page, and whether one after it, `page` of `per_page`, would
    /// have anything.
    pub(crate) fn into_page(self, page: u32, per_page: u32) -> SearchPage {
        let shown = u64::from(page) * u64::from(per_page);
        let more = shown < self.total_count.min(SEARCH_RESULT_CAP);
        let hits = self
            .items
            .into_iter()
            .filter_map(|item| {
                let repo_full_name = repo_of(&item.repository_url)?;
                let (login, avatar) = item
                    .user
                    .map(|u| (u.login, u.avatar_url))
                    .unwrap_or_default();
                Some(SearchHit {
                    kind: if item.pull_request.is_some() {
                        SubjectKind::PullRequest
                    } else {
                        SubjectKind::Issue
                    },
                    merged: item
                        .pull_request
                        .as_ref()
                        .is_some_and(|pr| pr.merged_at.is_some()),
                    repo_full_name,
                    number: item.number,
                    title: item.title,
                    state: item.state,
                    state_reason: item.state_reason,
                    draft: item.draft.unwrap_or(false),
                    author_login: login,
                    author_avatar_url: avatar,
                    updated_at: item.updated_at,
                    html_url: item.html_url,
                })
            })
            .collect();
        SearchPage { hits, more }
    }
}

/// `owner/repo` from `https://api.github.com/repos/owner/repo`, or an
/// enterprise host's `…/api/v3/repos/owner/repo`.
fn repo_of(url: &str) -> Option<String> {
    let rest = &url[url.rfind("/repos/")? + "/repos/".len()..];
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (!owner.is_empty() && !repo.is_empty() && parts.next().is_none())
        .then(|| format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reads_pull_requests_and_issues_and_knows_when_there_is_more() {
        let body = r#"{"total_count": 51, "incomplete_results": false, "items": [
            {"number": 7, "title": "Fence", "state": "open", "draft": true,
             "user": {"login": "rohan", "avatar_url": "https://avatars/1"},
             "updated_at": "2026-09-20T10:00:00Z", "html_url": "https://github.com/a/b/pull/7",
             "repository_url": "https://api.github.com/repos/a/b",
             "pull_request": {"merged_at": null}},
            {"number": 8, "title": "Merged", "state": "closed",
             "updated_at": "2026-09-19T10:00:00Z",
             "repository_url": "https://git.corp/api/v3/repos/team/app",
             "pull_request": {"merged_at": "2026-09-19T10:00:00Z"}},
            {"number": 9, "title": "Wontfix", "state": "closed", "state_reason": "not_planned",
             "draft": null, "updated_at": "2026-09-18T10:00:00Z",
             "repository_url": "https://api.github.com/repos/a/b"}
        ]}"#;
        let payload: SearchPayload = serde_json::from_str(body).unwrap();
        let page = payload.into_page(1, 50);
        assert!(page.more, "51 results, 50 a page");
        let states: Vec<SubjectState> = page.hits.iter().map(SearchHit::subject_state).collect();
        assert_eq!(
            states,
            [
                SubjectState::PrDraft,
                SubjectState::PrMerged,
                SubjectState::IssueNotPlanned
            ]
        );
        assert_eq!(page.hits[0].repo_full_name, "a/b");
        assert_eq!(page.hits[0].author_login, "rohan");
        assert_eq!(
            page.hits[1].repo_full_name, "team/app",
            "an enterprise host's"
        );
        assert_eq!(page.hits[2].kind, SubjectKind::Issue);

        let payload: SearchPayload = serde_json::from_str(body).unwrap();
        assert!(!payload.into_page(2, 50).more);
    }

    #[test]
    fn github_serves_no_more_than_a_thousand() {
        let payload: SearchPayload =
            serde_json::from_str(r#"{"total_count": 5000, "items": []}"#).unwrap();
        assert!(!payload.into_page(20, 50).more);
    }
}
