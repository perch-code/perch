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

use crate::model::SubjectState;
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
    /// The GraphQL node id, which reactions on the body are read and left
    /// by. Empty on a snapshot an older build kept.
    #[serde(default)]
    pub node_id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// The reactions on the body, once read with the comments.
    #[serde(default)]
    pub reactions: crate::reactions::Reactions,
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

    /// Open, closed or not planned, as the sidebar's glyphs draw it.
    pub fn subject_state(&self) -> SubjectState {
        if self.is_open() {
            SubjectState::IssueOpen
        } else if self.state_reason.as_deref() == Some("not_planned") {
            SubjectState::IssueNotPlanned
        } else {
            SubjectState::IssueClosed
        }
    }
}

/// Why an issue is closed, as GitHub's Close issue menu offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IssueCloseReason {
    /// Done, fixed or answered: GitHub's default.
    #[default]
    Completed,
    /// Won't be done.
    NotPlanned,
}

impl IssueCloseReason {
    /// The `state_reason` GitHub's REST API takes and answers with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NotPlanned => "not_planned",
        }
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
    pub node_id: String,
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
            node_id: self.node_id,
            title: self.title,
            body: self.body.unwrap_or_default(),
            reactions: crate::reactions::Reactions::default(),
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

    /// The state as the sidebar's glyphs draw it.
    pub fn subject_state(&self) -> SubjectState {
        match self {
            Subject::PullRequest(pr) => pr.subject_state(),
            Subject::Issue(i) => i.subject_state(),
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
    /// The repository an owned checkout was added from when that is one
    /// of the reader's roots rather than Perch's bare clone: where its
    /// branch lives, and where removing it prunes. `None` for the bare
    /// clone, and for a checkout that is not owned.
    pub source: Option<PathBuf>,
    /// How the checkout's head stood against its pull request's the last
    /// time both commits were here to count: set by the root scan and by
    /// opening the item. Read through `Item::divergence`, which checks it
    /// still describes the two heads the row has now.
    pub divergence: Option<Divergence>,
}

/// Commits on one head and not the other, counted in the checkout's
/// repository for a pair of heads.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Divergence {
    /// The checkout's head the counts were taken at.
    pub head: String,
    /// The pull request's head they were taken against.
    pub pr_head: String,
    /// Commits on the checkout that the pull request doesn't have.
    pub ahead: u32,
    /// Commits on the pull request that the checkout doesn't have.
    pub behind: u32,
}

/// An item whose checkout and open pull request are at different
/// commits. The counts are `None` when the pull request's head isn't in
/// the checkout's repository, or hasn't been counted for these heads yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diverged {
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

impl Diverged {
    /// What the Branch row and the bylines say: `3 unpushed`,
    /// `2 behind PR`, both, or that the pull request's head isn't here.
    pub fn label(&self) -> String {
        match (self.ahead, self.behind) {
            (Some(ahead), Some(behind)) if ahead > 0 && behind > 0 => {
                format!("{ahead} unpushed \u{b7} {behind} behind PR")
            }
            (Some(ahead), Some(_)) if ahead > 0 => format!("{ahead} unpushed"),
            (Some(_), Some(behind)) if behind > 0 => format!("{behind} behind PR"),
            _ => "PR head not fetched".to_string(),
        }
    }
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

    /// Whether the checkout and the open pull request are at different
    /// commits, and by how much when that has been counted. `None` when
    /// they match, when either head is unknown, and on an item with no
    /// checkout or no open pull request.
    pub fn divergence(&self) -> Option<Diverged> {
        let checkout = self.checkout.as_ref()?;
        let pr = self.pull_request().filter(|pr| pr.is_open() && !pr.merged)?;
        if checkout.head_sha.is_empty() || pr.head_sha.is_empty() || checkout.head_sha == pr.head_sha {
            return None;
        }
        let counted = checkout
            .divergence
            .as_ref()
            .filter(|d| d.head == checkout.head_sha && d.pr_head == pr.head_sha);
        Some(Diverged {
            ahead: counted.map(|d| d.ahead),
            behind: counted.map(|d| d.behind),
        })
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
            divergence: None,
            source: None,
        });
        assert_eq!(i.title(), "perch/issue-42");
        i.subject = Some(Subject::Issue(Issue {
            number: 42,
            node_id: String::new(),
            title: "Fence the manifest".into(),
            body: String::new(),
            reactions: Default::default(),
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

    fn pr_with_checkout(checkout_head: &str, pr_head: &str) -> Item {
        let mut i = item();
        i.checkout = Some(Checkout {
            branch: "fix".into(),
            base_ref: "main".into(),
            head_sha: checkout_head.into(),
            path: "/x".into(),
            owned: false,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        let pr: PullRequest = serde_json::from_value(serde_json::json!({
            "number": 7, "title": "t", "html_url": "", "author_login": "a",
            "author_avatar_url": "", "head_sha": pr_head, "head_ref": "fix",
            "base_sha": "", "base_ref": "main", "draft": false, "merged": false,
            "state": "open", "additions": 0, "deletions": 0, "changed_files": 0,
            "repo_full_name": "criccomini/perch"
        }))
        .unwrap();
        i.subject = Some(Subject::PullRequest(pr));
        i
    }

    #[test]
    fn divergence_is_counted_only_for_the_heads_the_row_has_now() {
        assert_eq!(pr_with_checkout("h", "h").divergence(), None);
        let mut i = pr_with_checkout("h2", "h1");
        assert_eq!(i.divergence(), Some(Diverged { ahead: None, behind: None }));
        assert_eq!(i.divergence().unwrap().label(), "PR head not fetched");
        i.checkout.as_mut().unwrap().divergence =
            Some(Divergence { head: "h2".into(), pr_head: "h1".into(), ahead: 3, behind: 0 });
        assert_eq!(i.divergence().unwrap().label(), "3 unpushed");
        // A count for heads the row no longer has is not used.
        i.checkout.as_mut().unwrap().head_sha = "h3".into();
        assert_eq!(i.divergence().unwrap().label(), "PR head not fetched");
        // No open pull request, nothing to diverge from.
        if let Some(Subject::PullRequest(pr)) = &mut i.subject {
            pr.merged = true;
        }
        assert_eq!(i.divergence(), None);
        i.subject = None;
        assert_eq!(i.divergence(), None);
    }

    #[test]
    fn the_divergence_label_names_each_side() {
        let label = |ahead, behind| Diverged { ahead: Some(ahead), behind: Some(behind) }.label();
        assert_eq!(label(3, 0), "3 unpushed");
        assert_eq!(label(0, 2), "2 behind PR");
        assert_eq!(label(3, 2), "3 unpushed \u{b7} 2 behind PR");
    }

    #[test]
    fn closed_means_the_subject_is_closed() {
        let mut i = item();
        i.subject = Some(Subject::Issue(Issue {
            number: 1,
            node_id: String::new(),
            title: "t".into(),
            body: String::new(),
            reactions: Default::default(),
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
        j.checkout = Some(Checkout { branch: "b".into(), base_ref: "main".into(), head_sha: "h".into(), path: "/x".into(), owned: false, on_disk: true, churn: None, last_seen_head: None, source: None, divergence: None });
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
            node_id: String::new(),
            title: "t".into(),
            body: "b".into(),
            reactions: Default::default(),
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
