//! A GitHub that answers from what a test put in it, and the fixtures
//! the list pass's and the backfill's tests build from. Both run over a
//! `Source`, and one fake serves both so a test of either can say what
//! was asked, in what order.

use std::collections::HashMap;
use std::sync::Mutex;

use jiff::Timestamp;
use perch_github::{
    Allowance, Cache, Discussion, Issue, IssueComment, Item, ItemSubject, Listed, Page, PullRequest,
};

use super::sync::Source;

pub(super) const REPO: &str = "a/b";

pub(super) fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

/// A pull request as the list gives it: no stats, no merger.
pub(super) fn pull(n: u64, updated: &str) -> PullRequest {
    PullRequest {
        number: n,
        title: format!("pull {n}"),
        author_login: format!("u{n}"),
        author_avatar_url: String::new(),
        head_sha: format!("h{n}"),
        head_ref: format!("branch-{n}"),
        base_sha: "m".into(),
        base_ref: "main".into(),
        draft: false,
        merged: false,
        state: "open".into(),
        additions: 0,
        deletions: 0,
        changed_files: 0,
        repo_full_name: REPO.into(),
        head_repo_full_name: Some(REPO.into()),
        updated_at: Some(at(updated)),
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

/// The same pull request as a detail fetch gives it, merged.
pub(super) fn merged(n: u64, updated: &str) -> PullRequest {
    let mut pr = pull(n, updated);
    pr.state = "closed".into();
    pr.merged = true;
    pr.merged_at = Some(at(updated));
    pr.merged_by = Some("rohan".into());
    pr.closed_at = Some(at(updated));
    pr.additions = 12;
    pr.deletions = 3;
    pr.changed_files = 2;
    pr
}

pub(super) fn issue(n: u64, updated: &str) -> Issue {
    Issue {
        number: n,
        title: format!("issue {n}"),
        body: String::new(),
        state: "open".into(),
        state_reason: None,
        author_login: format!("u{n}"),
        author_avatar_url: String::new(),
        labels: Vec::new(),
        assignees: Vec::new(),
        comments: 0,
        created_at: None,
        updated_at: at(updated),
        repo_full_name: REPO.into(),
    }
}

/// The same issue closed, as a closed page lists it.
pub(super) fn closed_issue(n: u64, updated: &str) -> Issue {
    let mut i = issue(n, updated);
    i.state = "closed".into();
    i
}

/// One comment, as either kind of discussion carries it.
pub(super) fn comment(id: u64, body: &str) -> IssueComment {
    IssueComment {
        id,
        user: "rohan".into(),
        user_avatar_url: String::new(),
        body: body.into(),
        created_at: at("2026-09-15T10:00:00Z"),
        updated_at: None,
        html_url: format!("https://github.com/a/b/pull/1#issuecomment-{id}"),
    }
}

/// A discussion with one comment that names the number, so a test can
/// see which one landed where.
pub(super) fn discussion(n: u64) -> Discussion {
    Discussion {
        opened_at: Some(at("2026-09-15T09:00:00Z")),
        body: format!("about {n}"),
        comments: vec![comment(n * 10, &format!("comment on {n}"))],
        ..Default::default()
    }
}

pub(super) fn changed<T>(etag: Option<&str>, items: Vec<T>) -> Listed<T> {
    Listed::Changed { etag: etag.map(str::to_string), items }
}

/// When a list entry was last updated, for a page's `newest`.
pub(super) trait Updated {
    fn updated(&self) -> Option<Timestamp>;
}

impl Updated for PullRequest {
    fn updated(&self) -> Option<Timestamp> {
        self.updated_at
    }
}

impl Updated for Issue {
    fn updated(&self) -> Option<Timestamp> {
        Some(self.updated_at)
    }
}

/// One closed page, with the page the `Link` header would name as last,
/// its newest the newest of its entries. A test of a page that listed
/// pull requests among issues builds the `Page` itself.
pub(super) fn page<T: Updated>(items: Vec<T>, last_page: Option<u32>) -> Page<T> {
    let newest = items.iter().filter_map(Updated::updated).max();
    Page { items, last_page, newest }
}

pub(super) fn down() -> perch_github::Error {
    perch_github::Error::Network("the fake is down".into())
}

/// A GitHub that answers from what the test put in it. A list left
/// `None` fails; a page or a number nobody put in fails, or answers the
/// status the test gave it; every call is written down so a test can say
/// what was asked.
#[derive(Default)]
pub(super) struct Fake {
    pub pulls: Option<Listed<PullRequest>>,
    pub issues: Option<Listed<Issue>>,
    pub pulls_by_number: HashMap<u64, PullRequest>,
    pub issues_by_number: HashMap<u64, Issue>,
    /// A number GitHub refuses, and with what status.
    pub refused: HashMap<u64, u16>,
    /// The closed history, by page number.
    pub closed_pulls: HashMap<u32, Page<PullRequest>>,
    pub closed_issues: HashMap<u32, Page<Issue>>,
    pub discussions: HashMap<u64, Discussion>,
    pub comments: HashMap<u64, Vec<IssueComment>>,
    /// What the last response said about the rate limit. Settable, so a
    /// test's sleeper can play the reset.
    pub allowance: Mutex<Option<Allowance>>,
    pub calls: Mutex<Vec<String>>,
}

impl Fake {
    pub fn note(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn set_allowance(&self, remaining: i64, reset_epoch: i64) {
        *self.allowance.lock().unwrap() = Some(Allowance { remaining, reset_epoch });
    }

    fn refusal(&self, number: u64) -> perch_github::Error {
        match self.refused.get(&number) {
            Some(status) => perch_github::Error::Status { status: *status, body: "{}".into() },
            None => down(),
        }
    }
}

impl Source for Fake {
    async fn list_open_pulls(
        &self,
        _repo: &str,
        etag: Option<&str>,
    ) -> Result<Listed<PullRequest>, perch_github::Error> {
        self.note(format!("pulls etag={}", etag.unwrap_or("-")));
        self.pulls.clone().ok_or_else(down)
    }

    async fn list_open_issues(
        &self,
        _repo: &str,
        etag: Option<&str>,
    ) -> Result<Listed<Issue>, perch_github::Error> {
        self.note(format!("issues etag={}", etag.unwrap_or("-")));
        self.issues.clone().ok_or_else(down)
    }

    async fn list_closed_pulls(&self, _repo: &str, page: u32) -> Result<Page<PullRequest>, perch_github::Error> {
        self.note(format!("closed pulls page={page}"));
        self.closed_pulls.get(&page).cloned().ok_or_else(down)
    }

    async fn list_closed_issues(
        &self,
        _repo: &str,
        page: u32,
        since: Option<Timestamp>,
    ) -> Result<Page<Issue>, perch_github::Error> {
        let since = since.map(|s| s.to_string()).unwrap_or_else(|| "-".into());
        self.note(format!("closed issues page={page} since={since}"));
        self.closed_issues.get(&page).cloned().ok_or_else(down)
    }

    async fn fetch_pull_request(&self, _repo: &str, number: u64) -> Result<PullRequest, perch_github::Error> {
        self.note(format!("pull {number}"));
        self.pulls_by_number.get(&number).cloned().ok_or_else(|| self.refusal(number))
    }

    async fn fetch_issue(&self, _repo: &str, number: u64) -> Result<Issue, perch_github::Error> {
        self.note(format!("issue {number}"));
        self.issues_by_number.get(&number).cloned().ok_or_else(|| self.refusal(number))
    }

    async fn fetch_discussion(&self, _repo: &str, number: u64) -> Result<Discussion, perch_github::Error> {
        self.note(format!("discussion {number}"));
        self.discussions.get(&number).cloned().ok_or_else(|| self.refusal(number))
    }

    async fn fetch_issue_comments(
        &self,
        _repo: &str,
        number: u64,
    ) -> Result<Vec<IssueComment>, perch_github::Error> {
        self.note(format!("comments {number}"));
        self.comments.get(&number).cloned().ok_or_else(|| self.refusal(number))
    }

    fn allowance(&self) -> Option<Allowance> {
        *self.allowance.lock().unwrap()
    }
}

/// A cache in a directory of its own, with the directory kept so it
/// lives as long as the cache does.
pub(super) fn cache() -> (tempfile::TempDir, Mutex<Cache>) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(&dir.path().join("perch.db"), None).unwrap();
    (dir, Mutex::new(cache))
}

pub(super) fn meta(cache: &Mutex<Cache>, key: &str) -> Option<String> {
    cache.lock().unwrap().meta(key).unwrap()
}

pub(super) fn store_pull(cache: &Mutex<Cache>, pr: PullRequest) -> Item {
    let mut item = Item::new(REPO, at("2026-09-10T00:00:00Z"));
    item.subject = Some(ItemSubject::PullRequest(pr));
    cache.lock().unwrap().insert_item(&item).unwrap();
    item
}

pub(super) fn store_issue(cache: &Mutex<Cache>, issue: Issue) -> Item {
    let mut item = Item::new(REPO, at("2026-09-10T00:00:00Z"));
    item.subject = Some(ItemSubject::Issue(issue));
    cache.lock().unwrap().insert_item(&item).unwrap();
    item
}
