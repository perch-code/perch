//! A look at the subjects of many notifications at once.
//!
//! The notifications API names a pull request or an issue and says nothing
//! of who opened it, the state it is in, or what a pull request changes.
//! One GraphQL query asks after every subject in a batch, aliased by
//! repository and number, so a full inbox costs a request or two rather
//! than one a row. The answer is read back onto the threads by those
//! aliases.
//!
//! The same look answers who acted on each subject last, which the rows
//! stack as faces, for items and search hits as for notifications.

use std::collections::HashMap;

use jiff::Timestamp;
use serde::Deserialize;

use crate::model::{Participant, SubjectKind, SubjectSnapshot, SubjectState, Thread};

/// How many faces a row stacks, as GitHub's inbox does.
const PARTICIPANTS: usize = 3;

/// The subjects of one batch of threads: the query that asks after them,
/// and what is needed to read the answer back.
pub(crate) struct SubjectLook {
    /// Each repository's name, in alias order: `r0`, `r1`, ...
    repos: Vec<String>,
    /// The threads about each subject, by repository alias index and
    /// number: their ids and `updated_at`s, which stamp the snapshot.
    /// None about a subject asked after for an item or a search hit.
    threads: HashMap<(usize, u64), Vec<(String, Timestamp)>>,
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
    /// A pull request's counts; an issue has none.
    #[serde(default)]
    additions: Option<u32>,
    #[serde(default)]
    deletions: Option<u32>,
    #[serde(default)]
    review_requests: Option<CountNode>,
    #[serde(default)]
    created_at: Option<Timestamp>,
    #[serde(default)]
    comments: Option<Nodes<ActNode>>,
    /// A pull request's; an issue has none.
    #[serde(default)]
    reviews: Option<Nodes<ActNode>>,
    #[serde(default)]
    commits: Option<Nodes<CommitNode>>,
}

/// A connection's nodes, any of which GitHub may leave null.
#[derive(Debug, Deserialize)]
pub(crate) struct Nodes<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<T>>,
}

/// A comment or a review: who, and when. A review still pending has no
/// `at`, and counts for nothing.
#[derive(Debug, Deserialize)]
pub(crate) struct ActNode {
    #[serde(default)]
    author: Option<ActorNode>,
    #[serde(default)]
    at: Option<Timestamp>,
}

/// A commit, whose author is someone on GitHub only when their email is.
#[derive(Debug, Deserialize)]
pub(crate) struct CommitNode {
    commit: CommitInner,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitInner {
    #[serde(default)]
    at: Option<Timestamp>,
    #[serde(default)]
    author: Option<GitActorNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GitActorNode {
    #[serde(default)]
    user: Option<ActorNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActorNode {
    #[serde(default)]
    login: String,
    #[serde(default)]
    avatar_url: String,
}

/// A connection asked only how many it holds.
#[derive(Debug, Deserialize)]
pub(crate) struct CountNode {
    #[serde(default)]
    total_count: u32,
}

/// The answer's `data`: each repository alias to its subjects by alias,
/// either of which is null when GitHub could not resolve it.
pub(crate) type SubjectLookData = HashMap<String, Option<HashMap<String, Option<SubjectNode>>>>;

const FRAGMENT: &str = r#"
fragment subject on IssueOrPullRequest {
  __typename
  ... on Issue {
    author { login avatar_url: avatarUrl }
    created_at: createdAt
    issue_state: state
    state_reason: stateReason
    comments(last: 10) { nodes { author { login avatar_url: avatarUrl } at: createdAt } }
  }
  ... on PullRequest {
    author { login avatar_url: avatarUrl }
    created_at: createdAt
    pull_state: state
    is_draft: isDraft
    additions
    deletions
    review_requests: reviewRequests { total_count: totalCount }
    comments(last: 10) { nodes { author { login avatar_url: avatarUrl } at: createdAt } }
    reviews(last: 10) { nodes { author { login avatar_url: avatarUrl } at: submittedAt } }
    commits(last: 1) { nodes { commit { at: committedDate author { user { login avatar_url: avatarUrl } } } } }
  }
}
"#;

/// Only the state: the look a sweep sends after many pull requests that
/// were open, to learn which of them no longer are.
const STATE_FRAGMENT: &str = r#"
fragment subject on IssueOrPullRequest {
  __typename
  ... on Issue {
    issue_state: state
    state_reason: stateReason
  }
  ... on PullRequest {
    pull_state: state
    is_draft: isDraft
  }
}
"#;

impl SubjectLook {
    /// The look at the pull requests and the issues among `threads`, or
    /// `None` when there is nothing among them to ask after.
    pub(crate) fn new(threads: &[Thread]) -> Option<Self> {
        let mut look = Self::empty();
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
            if let Some(stamps) = look.add(&thread.repository.full_name, number) {
                stamps.push((thread.id.clone(), thread.updated_at));
            }
        }
        look.or_none()
    }

    /// The look at pull requests and issues named by repository and
    /// number, for rows that are not notifications: items and search
    /// hits. `None` when there is nothing to ask after.
    pub(crate) fn of_subjects<'a>(subjects: impl IntoIterator<Item = (&'a str, u64)>) -> Option<Self> {
        let mut look = Self::empty();
        for (repo, number) in subjects {
            look.add(repo, number);
        }
        look.or_none()
    }

    fn empty() -> Self {
        Self {
            repos: Vec::new(),
            threads: HashMap::new(),
        }
    }

    fn or_none(self) -> Option<Self> {
        (!self.threads.is_empty()).then_some(self)
    }

    /// Ask after `number` in `name`, and the threads about it. `None` for
    /// a name that is not `owner/repo`.
    fn add(&mut self, name: &str, number: u64) -> Option<&mut Vec<(String, Timestamp)>> {
        if !name.split_once('/').is_some_and(|(owner, repo)| {
            !owner.is_empty() && !repo.is_empty() && !repo.contains('/')
        }) {
            return None;
        }
        let repo = match self.repos.iter().position(|r| r == name) {
            Some(i) => i,
            None => {
                self.repos.push(name.to_string());
                self.repos.len() - 1
            }
        };
        Some(self.threads.entry((repo, number)).or_default())
    }

    /// The GraphQL document: one aliased `repository` per repository, one
    /// aliased `issueOrPullRequest` per subject under it.
    pub(crate) fn query(&self) -> String {
        self.query_with(FRAGMENT)
    }

    /// The document that asks only for each subject's state.
    pub(crate) fn state_query(&self) -> String {
        self.query_with(STATE_FRAGMENT)
    }

    fn query_with(&self, fragment: &str) -> String {
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
        query.push_str(fragment);
        query
    }

    /// The subjects the answer holds, by repository alias index and
    /// number. A subject GitHub left null is passed over.
    fn nodes(&self, data: SubjectLookData) -> Vec<(usize, u64, SubjectNode)> {
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
                if let Some(node) = node {
                    found.push((repo, number, node));
                }
            }
        }
        found
    }

    /// The snapshots the answer holds, by the id of each thread they are
    /// for. A subject GitHub left null, or answered in a shape that names
    /// no state, is passed over.
    pub(crate) fn read(&self, data: SubjectLookData) -> Vec<(String, SubjectSnapshot)> {
        let mut found = Vec::new();
        for (repo, number, node) in self.nodes(data) {
            let Some(state) = state_of(&node) else {
                continue;
            };
            let participants = participants(&node);
            let (author_login, author_avatar_url) = node
                .author
                .map(|actor| (actor.login, actor.avatar_url))
                .unwrap_or_default();
            let churn = node.additions.zip(node.deletions);
            let review_requested = node.review_requests.is_some_and(|r| r.total_count > 0);
            for (id, at) in self.threads.get(&(repo, number)).into_iter().flatten() {
                found.push((
                    id.clone(),
                    SubjectSnapshot {
                        author_login: author_login.clone(),
                        author_avatar_url: author_avatar_url.clone(),
                        state,
                        churn,
                        review_requested,
                        participants: Some(participants.clone()),
                        at: *at,
                    },
                ));
            }
        }
        found
    }

    /// The state of each subject the answer holds, by repository and
    /// number.
    pub(crate) fn states(&self, data: SubjectLookData) -> Vec<((String, u64), SubjectState)> {
        self.nodes(data)
            .into_iter()
            .filter_map(|(repo, number, node)| {
                Some(((self.repos[repo].clone(), number), state_of(&node)?))
            })
            .collect()
    }

    /// Who acted on each subject the answer holds last, by repository
    /// and number.
    pub(crate) fn participants(&self, data: SubjectLookData) -> Vec<((String, u64), Vec<Participant>)> {
        self.nodes(data)
            .into_iter()
            .map(|(repo, number, node)| ((self.repos[repo].clone(), number), participants(&node)))
            .collect()
    }
}

/// Who acted on the subject most recently, most recent first, each once,
/// at most three: the faces GitHub's inbox stacks on a row. Opening it,
/// a comment, a review and its latest commit each count, at the time they
/// happened. A ghost, a pending review, and a commit by an email GitHub
/// does not know count for nothing.
fn participants(node: &SubjectNode) -> Vec<Participant> {
    fn acts(nodes: &Option<Nodes<ActNode>>) -> impl Iterator<Item = (Timestamp, &ActorNode)> {
        nodes
            .iter()
            .flat_map(|nodes| nodes.nodes.iter().flatten())
            .filter_map(|act| Some((act.at?, act.author.as_ref()?)))
    }
    let mut acted: Vec<(Timestamp, &ActorNode)> = Vec::new();
    acted.extend(node.created_at.zip(node.author.as_ref()));
    acted.extend(acts(&node.comments));
    acted.extend(acts(&node.reviews));
    acted.extend(
        node.commits
            .iter()
            .flat_map(|commits| commits.nodes.iter().flatten())
            .filter_map(|c| Some((c.commit.at?, c.commit.author.as_ref()?.user.as_ref()?))),
    );
    acted.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut found: Vec<Participant> = Vec::new();
    for (_, actor) in acted {
        if actor.login.is_empty() || found.iter().any(|p| p.login == actor.login) {
            continue;
        }
        found.push(Participant {
            login: actor.login.clone(),
            avatar_url: actor.avatar_url.clone(),
        });
        if found.len() == PARTICIPANTS {
            break;
        }
    }
    found
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
            local_read: false,
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
            snoozed_until: None,
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
        assert!(
            query.contains("review_requests: reviewRequests { total_count: totalCount }"),
            "{query}"
        );
        assert!(query.contains("comments(last: 10)"), "{query}");
        assert!(query.contains("reviews(last: 10)"), "{query}");
        assert!(query.contains("commits(last: 1)"), "{query}");
    }

    #[test]
    fn a_look_at_states_asks_only_the_state_and_reads_it_back() {
        let look =
            SubjectLook::of_subjects([("a/b", 7), ("a/b", 8), ("c/d", 9), ("c/d", 10)]).unwrap();
        let query = look.state_query();
        assert!(query.contains("n7: issueOrPullRequest(number: 7) { ...subject }"), "{query}");
        assert!(query.contains("pull_state: state"), "{query}");
        assert!(!query.contains("comments("), "{query}");
        assert!(!query.contains("author"), "{query}");
        let data: SubjectLookData = serde_json::from_str(
            r#"{"r0":{"n7":{"__typename":"PullRequest","pull_state":"MERGED","is_draft":false},
                      "n8":null},
                "r1":{"n9":{"__typename":"PullRequest","pull_state":"OPEN","is_draft":true},
                      "n10":{"__typename":"Issue","issue_state":"CLOSED","state_reason":"COMPLETED"}}}"#,
        )
        .unwrap();
        let mut states = look.states(data);
        states.sort_by_key(|((_, number), _)| *number);
        assert_eq!(
            states,
            [
                (("a/b".to_string(), 7), SubjectState::PrMerged),
                (("c/d".to_string(), 9), SubjectState::PrDraft),
                (("c/d".to_string(), 10), SubjectState::IssueClosed),
            ]
        );
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
                  "is_draft": false,
                  "additions": 2799,
                  "deletions": 494,
                  "review_requests": { "total_count": 0 }
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
                  "is_draft": true,
                  "additions": 0,
                  "deletions": 0,
                  "review_requests": { "total_count": 2 }
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
                        churn: Some((2799, 494)),
                        review_requested: false,
                        participants: Some(vec![]),
                        at,
                    }
                ),
                (
                    "2".to_string(),
                    SubjectSnapshot {
                        author_login: String::new(),
                        author_avatar_url: String::new(),
                        state: SubjectState::IssueNotPlanned,
                        churn: None,
                        review_requested: false,
                        participants: Some(vec![]),
                        at,
                    }
                ),
                (
                    "4".to_string(),
                    SubjectSnapshot {
                        author_login: "me".into(),
                        author_avatar_url: "https://avatars/1".into(),
                        state: SubjectState::PrDraft,
                        churn: Some((0, 0)),
                        review_requested: true,
                        participants: Some(vec![]),
                        at,
                    }
                ),
            ]
        );
    }

    /// slatedb/slatedb#2084 as GitHub's inbox drew it: geeknarrator,
    /// who opened it and commented last, then rodesai, then agavra. The
    /// bot commented before agavra did, and falls off the end; a pending
    /// review and a commit by an unknown email count for nothing.
    #[test]
    fn the_participants_are_the_three_who_acted_last() {
        let node: SubjectNode = serde_json::from_str(
            r#"{
              "__typename": "PullRequest",
              "pull_state": "OPEN",
              "author": { "login": "geeknarrator", "avatar_url": "https://avatars/g" },
              "created_at": "2026-09-08T17:57:14Z",
              "comments": { "nodes": [
                { "author": { "login": "geeknarrator", "avatar_url": "https://avatars/g" }, "at": "2026-09-10T14:13:41Z" },
                { "author": { "login": "dosubot", "avatar_url": "https://avatars/in/1" }, "at": "2026-09-10T18:05:19Z" },
                { "author": { "login": "agavra", "avatar_url": "https://avatars/a" }, "at": "2026-09-10T21:19:51Z" },
                { "author": { "login": "rodesai", "avatar_url": "https://avatars/r" }, "at": "2026-09-15T04:51:39Z" },
                { "author": null, "at": "2026-09-27T00:00:00Z" },
                { "author": { "login": "geeknarrator", "avatar_url": "https://avatars/g" }, "at": "2026-09-28T17:03:08Z" }
              ] },
              "reviews": { "nodes": [
                { "author": { "login": "rodesai", "avatar_url": "https://avatars/r" }, "at": "2026-09-09T20:43:36Z" },
                { "author": { "login": "pending", "avatar_url": "https://avatars/p" }, "at": null },
                null
              ] },
              "commits": { "nodes": [
                { "commit": { "at": "2026-09-28T17:03:36Z", "author": { "user": null } } }
              ] }
            }"#,
        )
        .unwrap();
        let logins: Vec<String> = participants(&node).into_iter().map(|p| p.login).collect();
        assert_eq!(logins, ["geeknarrator", "rodesai", "agavra"]);
    }

    /// Who opened it counts, at the time they did, and so does whoever
    /// wrote the latest commit: on a quiet subject that is all there is.
    #[test]
    fn opening_it_and_the_latest_commit_count() {
        let node: SubjectNode = serde_json::from_str(
            r#"{
              "__typename": "PullRequest",
              "author": { "login": "opener", "avatar_url": "" },
              "created_at": "2026-09-08T00:00:00Z",
              "commits": { "nodes": [
                { "commit": { "at": "2026-09-09T00:00:00Z", "author": { "user": { "login": "pusher", "avatar_url": "" } } } }
              ] }
            }"#,
        )
        .unwrap();
        let logins: Vec<String> = participants(&node).into_iter().map(|p| p.login).collect();
        assert_eq!(logins, ["pusher", "opener"]);
    }

    /// Items and search hits are asked after by repository and number,
    /// and answered the same way.
    #[test]
    fn subjects_are_asked_after_by_repository_and_number() {
        let look = SubjectLook::of_subjects([("slatedb/slatedb", 2084), ("not-a-repository", 1)]).unwrap();
        let query = look.query();
        assert!(query.contains("n2084: issueOrPullRequest(number: 2084)"), "{query}");
        assert!(!query.contains("number: 1)"), "{query}");
        let data: SubjectLookData = serde_json::from_str(
            r#"{ "r0": { "n2084": {
              "__typename": "Issue",
              "issue_state": "OPEN",
              "author": { "login": "someone", "avatar_url": "https://avatars/2" },
              "created_at": "2026-09-08T00:00:00Z"
            } } }"#,
        )
        .unwrap();
        assert_eq!(
            look.participants(data),
            vec![(
                ("slatedb/slatedb".to_string(), 2084),
                vec![Participant { login: "someone".into(), avatar_url: "https://avatars/2".into() }]
            )]
        );
        assert!(SubjectLook::of_subjects([("local", 3)]).is_none());
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
