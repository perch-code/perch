//! Items, and the repositories they belong to.
//!
//! An item is one record with three optional parts: a GitHub subject, a
//! checkout, and attention. Which parts are present decides what it looks
//! like and what opens when it is clicked. Nothing is stored as "a PR item"
//! or "an issue item": a branch of your own becomes a pull request when
//! GitHub opens one on it, and it is the same row before and after.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::pulls::{PullRequest, UserRef};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    /// Six hex digits, as GitHub sends it.
    #[serde(default)]
    pub color: String,
}

/// What Perch keeps of an issue.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// `open` or `closed`.
    pub state: String,
    /// `completed` or `not_planned`, for a closed issue.
    #[serde(default)]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub comments: u32,
    /// When it was opened. Optional because a snapshot stored before this
    /// field existed has none.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    pub updated_at: Timestamp,
    pub repo_full_name: String,
}

impl Issue {
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }
}

/// The subset of GitHub's issue payload we read.
///
/// The issues endpoint answers for pull requests too, since GitHub numbers
/// them together; such a payload carries a `pull_request` key, and a caller
/// must refuse it rather than file a pull request as an issue.
#[derive(Debug, Deserialize)]
pub(crate) struct IssuePayload {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub user: Option<UserRef>,
    #[serde(default)]
    pub labels: Vec<LabelRef>,
    #[serde(default)]
    pub assignees: Vec<UserRef>,
    #[serde(default)]
    pub comments: u32,
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    pub updated_at: Timestamp,
    #[serde(default)]
    pub pull_request: Option<serde::de::IgnoredAny>,
}

/// A label as GitHub sends it. `color` is nullable there, which `Label`
/// does not allow.
#[derive(Debug, Deserialize)]
pub(crate) struct LabelRef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

impl IssuePayload {
    pub(crate) fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }

    pub(crate) fn into_issue(self, repo_full_name: &str) -> Issue {
        let (author_login, author_avatar_url) = self
            .user
            .map(|u| (u.login, u.avatar_url))
            .unwrap_or_default();
        Issue {
            number: self.number,
            title: self.title,
            body: self.body.unwrap_or_default(),
            state: self.state,
            state_reason: self.state_reason,
            author_login,
            author_avatar_url,
            labels: self
                .labels
                .into_iter()
                .map(|l| Label { name: l.name, color: l.color.unwrap_or_default() })
                .collect(),
            assignees: self.assignees.into_iter().map(|u| u.login).collect(),
            comments: self.comments,
            created_at: self.created_at,
            updated_at: self.updated_at,
            repo_full_name: repo_full_name.to_string(),
        }
    }
}

/// The GitHub thing an item is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "subjects are durable value snapshots and boxing one variant would ripple through the cache API"
)]
pub enum Subject {
    PullRequest(PullRequest),
    Issue(Issue),
}

impl Subject {
    pub fn number(&self) -> u64 {
        match self {
            Subject::PullRequest(pr) => pr.number,
            Subject::Issue(i) => i.number,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Subject::PullRequest(pr) => &pr.title,
            Subject::Issue(i) => &i.title,
        }
    }

    /// `pr` or `issue`: the column value.
    pub fn kind(&self) -> SubjectKind {
        match self {
            Subject::PullRequest(_) => SubjectKind::PullRequest,
            Subject::Issue(_) => SubjectKind::Issue,
        }
    }

    pub fn is_open(&self) -> bool {
        match self {
            Subject::PullRequest(pr) => pr.is_open(),
            Subject::Issue(i) => i.is_open(),
        }
    }

    /// The repository the subject lives in on GitHub. For a pull request
    /// opened from a fork this is the upstream, whichever repository the
    /// item's checkout is of.
    pub fn repo_full_name(&self) -> &str {
        match self {
            Subject::PullRequest(pr) => &pr.repo_full_name,
            Subject::Issue(i) => &i.repo_full_name,
        }
    }

    pub fn updated_at(&self) -> Option<Timestamp> {
        match self {
            Subject::PullRequest(pr) => pr.updated_at,
            Subject::Issue(i) => Some(i.updated_at),
        }
    }
}

/// The two kinds of subject an item can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    PullRequest,
    Issue,
}

impl SubjectKind {
    pub fn column(self) -> &'static str {
        match self {
            SubjectKind::PullRequest => "pr",
            SubjectKind::Issue => "issue",
        }
    }

    pub fn from_column(s: &str) -> Option<Self> {
        match s {
            "pr" => Some(SubjectKind::PullRequest),
            "issue" => Some(SubjectKind::Issue),
            _ => None,
        }
    }
}

/// A branch checked out somewhere on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Checkout {
    pub branch: String,
    /// What the diff is taken against.
    pub base_ref: String,
    pub head_sha: String,
    pub path: PathBuf,
    /// Under Perch's store, so Perch may remove it. A checkout under one of
    /// the reader's roots is never touched.
    pub owned: bool,
    /// False while archived, or when a root's worktree has gone.
    pub on_disk: bool,
    /// Lines added and removed against the base, for the row. Filled by
    /// the root scan; `None` until then.
    pub churn: Option<(u32, u32)>,
    /// The head at the previous open, which "Since you last looked" diffs
    /// from. Set from `head_sha` when the item is opened, before `head_sha`
    /// is refreshed; `None` until the item has been opened once.
    pub last_seen_head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Item {
    pub id: String,
    /// The repository the item is in, which is what the sidebar sections
    /// over: `owner/repo` on GitHub, `local/<name>` for one with no GitHub
    /// remote. For a checkout under a root this is its `[[repo]]`'s
    /// identity, the repository its GitHub remote names with `upstream`
    /// before `origin`, so a branch in a fork of a repository is that
    /// repository's, and its pull request there attaches to it.
    pub repo_full_name: String,
    pub subject: Option<Subject>,
    pub checkout: Option<Checkout>,
    /// The issue or item this was cut from.
    pub from_item_id: Option<String>,
    /// A name of the reader's own, from Rename.
    pub name: Option<String>,
    pub saved: bool,
    pub pinned_at: Option<Timestamp>,
    pub archived_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
}

impl Item {
    /// A fresh item with nothing set but what is given.
    pub fn new(repo_full_name: &str, now: Timestamp) -> Self {
        Self {
            id: crate::comments::new_id(),
            repo_full_name: repo_full_name.to_string(),
            subject: None,
            checkout: None,
            from_item_id: None,
            name: None,
            saved: false,
            pinned_at: None,
            archived_at: None,
            created_at: now,
            last_opened_at: now,
        }
    }

    /// The name, else the subject's title, else the branch.
    pub fn title(&self) -> &str {
        if let Some(name) = self.name.as_deref().filter(|n| !n.trim().is_empty()) {
            return name;
        }
        if let Some(subject) = &self.subject {
            return subject.title();
        }
        self.checkout.as_ref().map(|c| c.branch.as_str()).unwrap_or("")
    }

    /// The title before any Rename.
    pub fn given_title(&self) -> &str {
        match (&self.subject, &self.checkout) {
            (Some(s), _) => s.title(),
            (None, Some(c)) => &c.branch,
            (None, None) => "",
        }
    }

    pub fn pull_request(&self) -> Option<&PullRequest> {
        match &self.subject {
            Some(Subject::PullRequest(pr)) => Some(pr),
            _ => None,
        }
    }

    pub fn issue(&self) -> Option<&Issue> {
        match &self.subject {
            Some(Subject::Issue(i)) => Some(i),
            _ => None,
        }
    }

    pub fn branch(&self) -> Option<&str> {
        self.checkout.as_ref().map(|c| c.branch.as_str())
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned_at.is_some()
    }

    /// The subject is closed or merged.
    pub fn is_closed(&self) -> bool {
        self.subject.as_ref().is_some_and(|s| !s.is_open())
    }

    /// Not archived, and the subject open or absent.
    pub fn is_active(&self) -> bool {
        !self.is_archived() && !self.is_closed()
    }

    /// The subject's own activity. For a checkout-only item, when it was
    /// made.
    pub fn last_activity(&self) -> Timestamp {
        let subject = self.subject.as_ref().and_then(Subject::updated_at);
        [subject, Some(self.created_at)]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(self.created_at)
    }

    /// Whether the record is one the cache will take: a subject, a checkout,
    /// or both, never neither.
    pub fn is_valid(&self) -> bool {
        self.subject.is_some() || self.checkout.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn item() -> Item {
        Item::new("criccomini/perch", at("2026-09-18T10:00:00Z"))
    }

    #[test]
    fn the_title_prefers_a_name_then_the_subject_then_the_branch() {
        let mut i = item();
        i.checkout = Some(Checkout {
            branch: "perch/issue-42".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/x".into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        assert_eq!(i.title(), "perch/issue-42");
        i.subject = Some(Subject::Issue(Issue {
            number: 42,
            title: "Fence the manifest".into(),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "a".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: at("2026-09-18T10:00:00Z"),
            repo_full_name: "criccomini/perch".into(),
        }));
        assert_eq!(i.title(), "Fence the manifest");
        i.name = Some("the fence one".into());
        assert_eq!(i.title(), "the fence one");
        assert_eq!(i.given_title(), "Fence the manifest");
        i.name = Some("   ".into());
        assert_eq!(i.title(), "Fence the manifest", "a blank name is no name");
    }

    #[test]
    fn closed_means_the_subject_is_closed() {
        let mut i = item();
        i.subject = Some(Subject::Issue(Issue {
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: "closed".into(),
            state_reason: Some("completed".into()),
            author_login: String::new(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: None,
            updated_at: at("2026-09-18T10:00:00Z"),
            repo_full_name: "r".into(),
        }));
        assert!(i.is_closed());
        assert!(!i.is_active());
        let mut j = item();
        j.checkout = Some(Checkout { branch: "b".into(), base_ref: "main".into(), head_sha: "h".into(), path: "/x".into(), owned: false, on_disk: true, churn: None, last_seen_head: None });
        assert!(j.is_active());
        j.archived_at = Some(at("2026-09-18T12:00:00Z"));
        assert!(j.is_archived());
        assert!(!j.is_active());
    }

    const ISSUE_PAYLOAD: &str = r#"{
        "number": 42,
        "title": "Fence the manifest",
        "body": null,
        "state": "closed",
        "state_reason": "completed",
        "user": { "login": "rohan", "avatar_url": "https://avatars/2" },
        "labels": [
            { "name": "bug", "color": "d73a4a" },
            { "name": "storage", "color": null }
        ],
        "assignees": [ { "login": "chris" }, { "login": "rohan" } ],
        "comments": 3,
        "created_at": "2026-09-14T14:02:00Z",
        "updated_at": "2026-09-16T11:40:00Z"
    }"#;

    #[test]
    fn an_issue_payload_maps_to_what_perch_keeps() {
        let payload: IssuePayload = serde_json::from_str(ISSUE_PAYLOAD).unwrap();
        assert!(!payload.is_pull_request());
        let issue = payload.into_issue("slatedb/slatedb");
        assert_eq!(issue.number, 42);
        assert_eq!(issue.body, "", "a null body is an empty one");
        assert_eq!(issue.state_reason.as_deref(), Some("completed"));
        assert!(!issue.is_open());
        assert_eq!(issue.author_login, "rohan");
        assert_eq!(issue.author_avatar_url, "https://avatars/2");
        assert_eq!(
            issue.labels,
            vec![
                Label { name: "bug".into(), color: "d73a4a".into() },
                Label { name: "storage".into(), color: String::new() },
            ]
        );
        assert_eq!(issue.assignees, vec!["chris", "rohan"]);
        assert_eq!(issue.comments, 3);
        assert_eq!(issue.created_at, Some(at("2026-09-14T14:02:00Z")));
        assert_eq!(issue.repo_full_name, "slatedb/slatedb");
    }

    #[test]
    fn a_pull_request_shows_in_the_issue_payload() {
        let json = ISSUE_PAYLOAD.replace(
            r#""comments": 3,"#,
            r#""comments": 3, "pull_request": { "url": "https://api.github.com/repos/slatedb/slatedb/pulls/42" },"#,
        );
        let payload: IssuePayload = serde_json::from_str(&json).unwrap();
        assert!(payload.is_pull_request());
    }

    #[test]
    fn a_subject_serialises_with_its_kind() {
        let s = Subject::Issue(Issue {
            number: 7,
            title: "t".into(),
            body: "b".into(),
            state: "open".into(),
            state_reason: None,
            author_login: "a".into(),
            author_avatar_url: String::new(),
            labels: vec![Label { name: "bug".into(), color: "d73a4a".into() }],
            assignees: vec!["chris".into()],
            comments: 2,
            created_at: None,
            updated_at: at("2026-09-18T10:00:00Z"),
            repo_full_name: "r".into(),
        });
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"issue\""));
        assert_eq!(serde_json::from_str::<Subject>(&json).unwrap(), s);
    }
}
