//! A look at the subjects of many notifications at once.
//!
//! The notifications API names a pull request or an issue and says nothing
//! of who opened it or the state it is in. One GraphQL query asks after
//! every subject in a batch, aliased by repository and number, so a full
//! inbox costs a request or two rather than one a row. The answer is read
//! back onto the threads by those aliases.

use std::collections::HashMap;

use serde::Deserialize;

use crate::model::{SubjectKind, SubjectSnapshot, SubjectState, Thread};

/// The subjects of one batch of threads: the query that asks after them,
/// and what is needed to read the answer back.
pub(crate) struct SubjectLook {
    /// Each repository's name, in alias order: `r0`, `r1`, ...
    repos: Vec<String>,
    /// The threads about each subject, by repository alias index and
    /// number: their ids and `updated_at`s, which stamp the snapshot.
    threads: HashMap<(usize, u64), Vec<(String, jiff::Timestamp)>>,
}

/// What GitHub says of one subject, in the shape the query asks for it.
///
/// `state` is aliased apart for the two types: an `IssueState` and a
/// `PullRequestState` under one name is a query GitHub refuses.
#[derive(Debug, Deserialize)]
pub(crate) struct SubjectNode {
    #[serde(rename = "__typename")]
    typename: String,
    #[serde(default)]
    author: Option<ActorNode>,
    #[serde(default)]
    issue_state: Option<String>,
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    pull_state: Option<String>,
    #[serde(default)]
    is_draft: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActorNode {
    #[serde(default)]
    login: String,
    #[serde(default)]
    avatar_url: String,
}

/// The answer's `data`: each repository alias to its subjects by alias,
/// either of which is null when GitHub could not resolve it.
pub(crate) type SubjectLookData = HashMap<String, Option<HashMap<String, Option<SubjectNode>>>>;

const FRAGMENT: &str = r#"
fragment subject on IssueOrPullRequest {
  __typename
  ... on Issue {
    author { login avatar_url: avatarUrl }
    issue_state: state
    state_reason: stateReason
  }
  ... on PullRequest {
    author { login avatar_url: avatarUrl }
    pull_state: state
    is_draft: isDraft
  }
}
"#;

impl SubjectLook {
    /// The look at the pull requests and the issues among `threads`, or
    /// `None` when there is nothing among them to ask after.
    pub(crate) fn new(threads: &[Thread]) -> Option<Self> {
        let mut repos: Vec<String> = Vec::new();
        let mut subjects: HashMap<(usize, u64), Vec<(String, jiff::Timestamp)>> = HashMap::new();
        for thread in threads {
            if !matches!(
                thread.subject.kind,
                SubjectKind::PullRequest | SubjectKind::Issue
            ) {
                continue;
            }
            let Some(number) = thread.number_hint() else {
                continue;
            };
            let name = &thread.repository.full_name;
            if !name.split_once('/').is_some_and(|(owner, repo)| {
                !owner.is_empty() && !repo.is_empty() && !repo.contains('/')
            }) {
                continue;
            }
            let repo = match repos.iter().position(|r| r == name) {
                Some(i) => i,
                None => {
                    repos.push(name.clone());
                    repos.len() - 1
                }
            };
            subjects
                .entry((repo, number))
                .or_default()
                .push((thread.id.clone(), thread.updated_at));
        }
        if subjects.is_empty() {
            return None;
        }
        Some(Self {
            repos,
            threads: subjects,
        })
    }

    /// The GraphQL document: one aliased `repository` per repository, one
    /// aliased `issueOrPullRequest` per subject under it.
    pub(crate) fn query(&self) -> String {
        let mut numbers: HashMap<usize, Vec<u64>> = HashMap::new();
        for &(repo, number) in self.threads.keys() {
            numbers.entry(repo).or_default().push(number);
        }
        let mut query = String::from("query {\n");
        for (i, name) in self.repos.iter().enumerate() {
            let Some((owner, repo)) = name.split_once('/') else {
                continue;
            };
            let mut numbers = numbers.remove(&i).unwrap_or_default();
            numbers.sort_unstable();
            // A JSON string is a GraphQL string: the same quotes, and no
            // escape the one has that the other lacks.
            query.push_str(&format!(
                "  r{i}: repository(owner: {}, name: {}) {{\n",
                serde_json::to_string(owner).unwrap_or_default(),
                serde_json::to_string(repo).unwrap_or_default(),
            ));
            for number in numbers {
                query.push_str(&format!(
                    "    n{number}: issueOrPullRequest(number: {number}) {{ ...subject }}\n"
                ));
            }
            query.push_str("  }\n");
        }
        query.push_str("}\n");
        query.push_str(FRAGMENT);
        query
    }

    /// The snapshots the answer holds, by the id of each thread they are
    /// for. A subject GitHub left null, or answered in a shape that names
    /// no state, is passed over.
    pub(crate) fn read(&self, data: SubjectLookData) -> Vec<(String, SubjectSnapshot)> {
        let mut found = Vec::new();
        for (alias, subjects) in data {
            let Some(repo) = alias
                .strip_prefix('r')
                .and_then(|i| i.parse::<usize>().ok())
                .filter(|i| *i < self.repos.len())
            else {
                continue;
            };
            let Some(subjects) = subjects else { continue };
            for (alias, node) in subjects {
                let Some(number) = alias.strip_prefix('n').and_then(|n| n.parse::<u64>().ok())
                else {
                    continue;
                };
                let Some(node) = node else { continue };
                let Some(state) = state_of(&node) else {
                    continue;
                };
                let (author_login, author_avatar_url) = node
                    .author
                    .map(|actor| (actor.login, actor.avatar_url))
                    .unwrap_or_default();
                for (id, at) in self.threads.get(&(repo, number)).into_iter().flatten() {
                    found.push((
                        id.clone(),
                        SubjectSnapshot {
                            author_login: author_login.clone(),
                            author_avatar_url: author_avatar_url.clone(),
                            state,
                            at: *at,
                        },
                    ));
                }
            }
        }
        found
    }
}

/// The state a node is in, read the way an item's snapshot reads GitHub's
/// REST shape: a closed issue is not planned only when GitHub says so,
/// and a draft is a state of its own only while the pull request is open.
fn state_of(node: &SubjectNode) -> Option<SubjectState> {
    match node.typename.as_str() {
        "Issue" => match node.issue_state.as_deref()? {
            "OPEN" => Some(SubjectState::IssueOpen),
            "CLOSED" if node.state_reason.as_deref() == Some("NOT_PLANNED") => {
                Some(SubjectState::IssueNotPlanned)
            }
            "CLOSED" => Some(SubjectState::IssueClosed),
            _ => None,
        },
        "PullRequest" => match node.pull_state.as_deref()? {
            "MERGED" => Some(SubjectState::PrMerged),
            "CLOSED" => Some(SubjectState::PrClosed),
            "OPEN" if node.is_draft => Some(SubjectState::PrDraft),
            "OPEN" => Some(SubjectState::PrOpen),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Reason, Repository, Subject};

    fn thread(id: &str, repo: &str, kind: SubjectKind, number: Option<u64>) -> Thread {
        Thread {
            id: id.into(),
            unread: true,
            local_unread: false,
            reason: Reason::Subscribed,
            updated_at: "2026-09-17T18:00:00Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: format!("thread {id}"),
                kind,
                url: number.map(|n| format!("https://api.github.com/repos/{repo}/issues/{n}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: repo.into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
        }
    }

    #[test]
    fn the_query_asks_after_every_pull_request_and_issue_by_repository() {
        let threads = [
            thread("1", "slatedb/slatedb", SubjectKind::PullRequest, Some(2085)),
            thread("2", "slatedb/slatedb", SubjectKind::Issue, Some(2097)),
            thread("3", "criccomini/perch", SubjectKind::Issue, Some(7)),
            // Nothing to ask after: no number, or no one who opened it.
            thread("4", "slatedb/slatedb", SubjectKind::PullRequest, None),
            thread("5", "slatedb/slatedb", SubjectKind::CheckSuite, Some(9)),
            thread("6", "not-a-repository", SubjectKind::Issue, Some(1)),
        ];
        let look = SubjectLook::new(&threads).unwrap();
        let query = look.query();
        assert!(
            query.contains(
                "  r0: repository(owner: \"slatedb\", name: \"slatedb\") {\n    \
                 n2085: issueOrPullRequest(number: 2085) { ...subject }\n    \
                 n2097: issueOrPullRequest(number: 2097) { ...subject }\n  }\n"
            ),
            "{query}"
        );
        assert!(
            query.contains(
                "  r1: repository(owner: \"criccomini\", name: \"perch\") {\n    \
                 n7: issueOrPullRequest(number: 7) { ...subject }\n  }\n"
            ),
            "{query}"
        );
        assert!(!query.contains("number: 9"), "{query}");
        assert!(!query.contains("number: 1)"), "{query}");
        assert!(query.contains("fragment subject on IssueOrPullRequest"));
    }

    #[test]
    fn nothing_to_ask_after_is_no_query() {
        let threads = [
            thread("1", "a/b", SubjectKind::CheckSuite, None),
            thread("2", "a/b", SubjectKind::Release, Some(3)),
        ];
        assert!(SubjectLook::new(&threads).is_none());
        assert!(SubjectLook::new(&[]).is_none());
    }

    #[test]
    fn the_answer_is_read_back_onto_the_threads_by_alias() {
        let threads = [
            thread("1", "slatedb/slatedb", SubjectKind::PullRequest, Some(2085)),
            thread("2", "slatedb/slatedb", SubjectKind::Issue, Some(2097)),
            thread("3", "slatedb/slatedb", SubjectKind::PullRequest, Some(2098)),
            thread("4", "criccomini/perch", SubjectKind::Issue, Some(7)),
            thread("5", "gone/repository", SubjectKind::Issue, Some(1)),
        ];
        let look = SubjectLook::new(&threads).unwrap();
        let data: SubjectLookData = serde_json::from_str(
            r#"{
              "r0": {
                "n2085": {
                  "__typename": "PullRequest",
                  "author": { "login": "someone", "avatar_url": "https://avatars/2" },
                  "pull_state": "MERGED",
                  "is_draft": false
                },
                "n2097": {
                  "__typename": "Issue",
                  "author": null,
                  "issue_state": "CLOSED",
                  "state_reason": "NOT_PLANNED"
                },
                "n2098": null
              },
              "r1": {
                "n7": {
                  "__typename": "PullRequest",
                  "author": { "login": "me", "avatar_url": "https://avatars/1" },
                  "pull_state": "OPEN",
                  "is_draft": true
                }
              },
              "r2": null
            }"#,
        )
        .unwrap();
        let mut found = look.read(data);
        found.sort_by(|a, b| a.0.cmp(&b.0));
        let at = threads[0].updated_at;
        assert_eq!(
            found,
            vec![
                (
                    "1".to_string(),
                    SubjectSnapshot {
                        author_login: "someone".into(),
                        author_avatar_url: "https://avatars/2".into(),
                        state: SubjectState::PrMerged,
                        at,
                    }
                ),
                (
                    "2".to_string(),
                    SubjectSnapshot {
                        author_login: String::new(),
                        author_avatar_url: String::new(),
                        state: SubjectState::IssueNotPlanned,
                        at,
                    }
                ),
                (
                    "4".to_string(),
                    SubjectSnapshot {
                        author_login: "me".into(),
                        author_avatar_url: "https://avatars/1".into(),
                        state: SubjectState::PrDraft,
                        at,
                    }
                ),
            ]
        );
    }

    #[test]
    fn states_read_as_an_items_snapshot_reads_them() {
        let node = |json: &str| serde_json::from_str::<SubjectNode>(json).unwrap();
        assert_eq!(
            state_of(&node(r#"{"__typename":"Issue","issue_state":"OPEN"}"#)),
            Some(SubjectState::IssueOpen)
        );
        assert_eq!(
            state_of(&node(
                r#"{"__typename":"Issue","issue_state":"CLOSED","state_reason":"COMPLETED"}"#
            )),
            Some(SubjectState::IssueClosed)
        );
        assert_eq!(
            state_of(&node(
                r#"{"__typename":"Issue","issue_state":"CLOSED","state_reason":"DUPLICATE"}"#
            )),
            Some(SubjectState::IssueClosed)
        );
        assert_eq!(
            state_of(&node(
                r#"{"__typename":"PullRequest","pull_state":"CLOSED"}"#
            )),
            Some(SubjectState::PrClosed)
        );
        assert_eq!(
            state_of(&node(r#"{"__typename":"PullRequest","pull_state":"OPEN"}"#)),
            Some(SubjectState::PrOpen)
        );
        // A draft that was merged or closed is merged or closed.
        assert_eq!(
            state_of(&node(
                r#"{"__typename":"PullRequest","pull_state":"MERGED","is_draft":true}"#
            )),
            Some(SubjectState::PrMerged)
        );
        assert_eq!(state_of(&node(r#"{"__typename":"Discussion"}"#)), None);
        assert_eq!(
            state_of(&node(
                r#"{"__typename":"Issue","issue_state":"SOMETHING_NEW"}"#
            )),
            None
        );
    }
}
