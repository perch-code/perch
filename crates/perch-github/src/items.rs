//! Projects, and the items under them.
//!
//! An item is one record with three optional parts: a GitHub subject, a
//! checkout, and attention. Which parts are present decides what it looks
//! like and what opens when it is clicked. Nothing is stored as "a PR item"
//! or "an issue item": a branch of your own becomes a pull request when
//! GitHub opens one on it, and it is the same row before and after.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::pulls::PullRequest;

/// A place work belongs to: some repositories, and some directories where
/// you already work on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The name, lower-cased.
    pub id: String,
    pub name: String,
    /// `owner/repo`.
    pub repos: Vec<String>,
    /// Absolute paths to checkouts of those repositories.
    pub roots: Vec<PathBuf>,
    /// Not in config: made because an item needed a home.
    pub implicit: bool,
}

impl Project {
    pub fn id_for(name: &str) -> String {
        name.trim().to_lowercase()
    }

    pub fn implicit(name: &str) -> Self {
        Self {
            id: Self::id_for(name),
            name: name.trim().to_string(),
            repos: Vec::new(),
            roots: Vec::new(),
            implicit: true,
        }
    }

    pub fn claims(&self, repo_full_name: &str) -> bool {
        self.repos.iter().any(|r| r.eq_ignore_ascii_case(repo_full_name))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    /// Six hex digits, as GitHub sends it.
    #[serde(default)]
    pub color: String,
}

/// What Perch keeps of an issue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub updated_at: Timestamp,
    pub repo_full_name: String,
}

impl Issue {
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }
}

/// The GitHub thing an item is about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub project_id: String,
    pub repo_full_name: String,
    pub subject: Option<Subject>,
    pub checkout: Option<Checkout>,
    /// The issue or item this was cut from.
    pub from_item_id: Option<String>,
    /// A name of the reader's own, from Rename.
    pub name: Option<String>,
    pub saved: bool,
    pub pinned_at: Option<Timestamp>,
    /// `attention_at` at the moment Done was pressed.
    pub done_at: Option<Timestamp>,
    pub attention_at: Option<Timestamp>,
    /// Why: `review_requested`, `mention`, `comment`, `manual`, `ci_failed`.
    pub attention_reason: Option<String>,
    pub archived_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
}

impl Item {
    /// A fresh item with nothing set but what is given.
    pub fn new(project_id: &str, repo_full_name: &str, now: Timestamp) -> Self {
        Self {
            id: crate::comments::new_id(),
            project_id: project_id.to_string(),
            repo_full_name: repo_full_name.to_string(),
            subject: None,
            checkout: None,
            from_item_id: None,
            name: None,
            saved: false,
            pinned_at: None,
            done_at: None,
            attention_at: None,
            attention_reason: None,
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

    /// Attention newer than the last Done.
    pub fn needs_attention(&self) -> bool {
        match (self.attention_at, self.done_at) {
            (Some(at), Some(done)) => at > done,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    pub fn is_done(&self) -> bool {
        !self.needs_attention() && self.done_at.is_some()
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned_at.is_some()
    }

    /// The subject is closed or merged, or the item is Done.
    pub fn is_closed(&self) -> bool {
        if self.is_done() {
            return true;
        }
        self.subject.as_ref().is_some_and(|s| !s.is_open())
    }

    /// Not archived, not Done, and the subject open or absent.
    pub fn is_active(&self) -> bool {
        !self.is_archived() && !self.is_closed()
    }

    /// The later of attention and the subject's own activity. For a
    /// checkout-only item, when it was made or last opened.
    pub fn last_activity(&self) -> Timestamp {
        let subject = self.subject.as_ref().and_then(Subject::updated_at);
        [self.attention_at, subject, Some(self.created_at)]
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
        Item::new("perch", "criccomini/perch", at("2026-09-18T10:00:00Z"))
    }

    #[test]
    fn attention_past_done_needs_looking_at() {
        let mut i = item();
        assert!(!i.needs_attention());
        i.attention_at = Some(at("2026-09-18T11:00:00Z"));
        assert!(i.needs_attention());
        i.done_at = Some(at("2026-09-18T11:00:00Z"));
        assert!(!i.needs_attention());
        assert!(i.is_done());
        i.attention_at = Some(at("2026-09-18T12:00:00Z"));
        assert!(i.needs_attention(), "newer attention reopens");
        assert!(!i.is_done());
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
    fn closed_means_the_subject_closed_or_done() {
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
            updated_at: at("2026-09-18T10:00:00Z"),
            repo_full_name: "r".into(),
        }));
        assert!(i.is_closed());
        assert!(!i.is_active());
        let mut j = item();
        j.checkout = Some(Checkout { branch: "b".into(), base_ref: "main".into(), head_sha: "h".into(), path: "/x".into(), owned: false, on_disk: true });
        assert!(j.is_active());
        j.attention_at = Some(at("2026-09-18T11:00:00Z"));
        j.done_at = Some(at("2026-09-18T11:00:00Z"));
        assert!(j.is_closed(), "done is closed");
        j.archived_at = Some(at("2026-09-18T12:00:00Z"));
        assert!(j.is_archived());
        assert!(!j.is_active());
    }

    #[test]
    fn a_project_id_is_its_name_lower_cased() {
        assert_eq!(Project::id_for("SlateDB "), "slatedb");
        let p = Project::implicit("perch");
        assert!(p.implicit);
        assert!(p.roots.is_empty());
        assert!(!p.claims("criccomini/perch"));
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
            updated_at: at("2026-09-18T10:00:00Z"),
            repo_full_name: "r".into(),
        });
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"issue\""));
        assert_eq!(serde_json::from_str::<Subject>(&json).unwrap(), s);
    }
}
