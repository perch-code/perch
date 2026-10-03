//! The pull request endpoint.
//!
//! The notifications API gives a subject URL and little else. Opening a pull
//! request needs the commits to check out and the branches to diff, which
//! only this endpoint carries.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::workflow_runs::{Check, CheckKind, CheckRunState, CheckState, Checks};

type CheckRunKey = (String, String, String);

/// A rollup entry with what it is told apart from the others by.
struct ParsedCheck {
    check: Check,
    /// Present only for CheckRuns: GitHub can return retries of the same
    /// workflow job, while distinct legacy status contexts remain rows.
    key: Option<CheckRunKey>,
    /// The workflow run a check run belongs to, when Actions made it.
    suite: Option<SuiteRef>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SuiteRef {
    id: u64,
    done: bool,
}

/// What Perch needs to check out and describe a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// The pull request page. Used for GitHub-hosted operations such as
    /// resolving merge conflicts.
    #[serde(default)]
    pub html_url: String,
    pub author_login: String,
    pub author_avatar_url: String,
    pub head_sha: String,
    /// Branch name on the head side, for the header.
    pub head_ref: String,
    pub base_sha: String,
    /// Branch the pull request targets. Fetched so a merge base can be found.
    pub base_ref: String,
    pub draft: bool,
    pub merged: bool,
    pub state: String,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    /// `owner/repo` of the repository the pull request lives in.
    pub repo_full_name: String,
    /// `owner/repo` of the repository the head branch is on: the same, or
    /// a fork.
    #[serde(default)]
    pub head_repo_full_name: Option<String>,
    /// When GitHub last saw activity on it.
    #[serde(default)]
    pub updated_at: Option<jiff::Timestamp>,
    /// Whether anyone has been asked to review and nobody has yet.
    #[serde(default)]
    pub review_requested: bool,
    /// The state of the head commit's checks, from the checks API.
    #[serde(default)]
    pub checks: Option<CheckStatus>,
    /// Where the reviews stand, from the reviews list.
    #[serde(default)]
    pub review_decision: Option<ReviewDecision>,
    /// When it was merged, and who pressed the button.
    #[serde(default)]
    pub merged_at: Option<Timestamp>,
    #[serde(default)]
    pub merged_by: Option<String>,
    /// When it was closed. Set on a merged pull request too.
    #[serde(default)]
    pub closed_at: Option<Timestamp>,
    /// Who has been asked to review and has not yet. Teams are not here:
    /// they have no login, and `review_requested` counts them.
    #[serde(default)]
    pub requested_reviewers: Vec<Reviewer>,
    /// The milestone's title.
    #[serde(default)]
    pub milestone: Option<String>,
    /// As on an issue, for the item's summary.
    #[serde(default)]
    pub labels: Vec<crate::items::Label>,
    #[serde(default)]
    pub assignees: Vec<String>,
}

/// Someone asked to review, for the item's summary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reviewer {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

/// The CI chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
}

/// The review chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    ReviewRequested,
    ChangesRequested,
    Approved,
}

/// Everything the merge panel needs from one head-fenced GraphQL overview.
/// It is kept separate from [`PullRequest`], whose REST snapshot owns
/// checkout and timeline metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestOverview {
    pub head_sha: String,
    pub mergeable: MergeableState,
    pub merge_state: MergeState,
    pub review_decision: Option<ReviewDecision>,
    pub viewer_can_merge_as_admin: bool,
    pub viewer_permission: Option<RepositoryPermission>,
    pub allowed_merge_methods: MergeMethods,
    pub checks: Checks,
    /// The commit message GitHub's merge box would offer for a squash.
    #[serde(default)]
    pub squash_message: CommitMessage,
    /// The commit message GitHub's merge box would offer for a merge commit.
    #[serde(default)]
    pub merge_message: CommitMessage,
}

impl PullRequestOverview {
    /// The message GitHub would write for `method`; None for a rebase,
    /// which keeps each commit's own.
    pub fn commit_message(&self, method: MergeMethod) -> Option<&CommitMessage> {
        match method {
            MergeMethod::Squash => Some(&self.squash_message),
            MergeMethod::Merge => Some(&self.merge_message),
            MergeMethod::Rebase => None,
        }
    }
}

/// A merge's commit message: its first line and the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitMessage {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeableState {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeState {
    Behind,
    Blocked,
    Clean,
    Dirty,
    Draft,
    HasHooks,
    Unstable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryPermission {
    Admin,
    Maintain,
    Write,
    Triage,
    Read,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MergeMethods {
    pub merge: bool,
    pub squash: bool,
    pub rebase: bool,
}

impl MergeMethods {
    pub fn allows(self, method: MergeMethod) -> bool {
        match method {
            MergeMethod::Merge => self.merge,
            MergeMethod::Squash => self.squash,
            MergeMethod::Rebase => self.rebase,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeResult {
    #[serde(default)]
    pub sha: Option<String>,
    pub merged: bool,
    #[serde(default)]
    pub message: String,
}

impl ReviewDecision {
    pub fn label(self) -> &'static str {
        match self {
            ReviewDecision::ReviewRequested => "review requested",
            ReviewDecision::ChangesRequested => "changes requested",
            ReviewDecision::Approved => "approved",
        }
    }
}

impl PullRequest {
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }

    /// Merged, closed, draft or open, as the sidebar's glyphs draw it.
    pub fn subject_state(&self) -> crate::SubjectState {
        if self.merged {
            crate::SubjectState::PrMerged
        } else if !self.is_open() {
            crate::SubjectState::PrClosed
        } else if self.draft {
            crate::SubjectState::PrDraft
        } else {
            crate::SubjectState::PrOpen
        }
    }
}

/// The subset of GitHub's payload we read.
#[derive(Debug, Deserialize)]
pub(crate) struct PullPayload {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub changed_files: u32,
    #[serde(default)]
    pub user: Option<UserRef>,
    pub head: GitRef,
    pub base: GitRef,
    #[serde(default)]
    pub updated_at: Option<jiff::Timestamp>,
    #[serde(default)]
    pub requested_reviewers: Vec<UserRef>,
    #[serde(default)]
    pub requested_teams: Vec<serde_json::Value>,
    #[serde(default)]
    pub merged_at: Option<Timestamp>,
    #[serde(default)]
    pub merged_by: Option<UserRef>,
    #[serde(default)]
    pub closed_at: Option<Timestamp>,
    #[serde(default)]
    pub milestone: Option<MilestoneRef>,
    #[serde(default)]
    pub labels: Vec<crate::items::LabelRef>,
    #[serde(default)]
    pub assignees: Vec<UserRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UserRef {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GitRef {
    #[serde(default)]
    pub sha: String,
    #[serde(rename = "ref", default)]
    pub ref_name: String,
    #[serde(default)]
    pub repo: Option<RepoRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RepoRef {
    #[serde(default)]
    pub full_name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MilestoneRef {
    #[serde(default)]
    pub title: String,
}

impl PullPayload {
    pub(crate) fn into_pull_request(self, fallback_repo: &str) -> PullRequest {
        let (author_login, author_avatar_url) = self
            .user
            .map(|u| (u.login, u.avatar_url))
            .unwrap_or_default();

        // The base repo is the one to clone: a pull request from a fork has a
        // different head repo, but `refs/pull/N/head` lives on the base.
        let repo_full_name = self
            .base
            .repo
            .as_ref()
            .map(|r| r.full_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| fallback_repo.to_string());

        let head_repo_full_name = self
            .head
            .repo
            .as_ref()
            .map(|r| r.full_name.clone())
            .filter(|n| !n.is_empty());

        PullRequest {
            number: self.number,
            title: self.title,
            html_url: self.html_url,
            author_login,
            author_avatar_url,
            head_repo_full_name,
            head_sha: self.head.sha,
            head_ref: self.head.ref_name,
            base_sha: self.base.sha,
            base_ref: self.base.ref_name,
            draft: self.draft,
            merged: self.merged,
            state: self.state,
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            repo_full_name,
            updated_at: self.updated_at,
            review_requested: !self.requested_reviewers.is_empty()
                || !self.requested_teams.is_empty(),
            checks: None,
            review_decision: None,
            merged_at: self.merged_at,
            merged_by: self.merged_by.map(|u| u.login),
            closed_at: self.closed_at,
            requested_reviewers: self
                .requested_reviewers
                .into_iter()
                .map(|u| Reviewer {
                    login: u.login,
                    avatar_url: u.avatar_url,
                })
                .collect(),
            milestone: self.milestone.map(|m| m.title),
            labels: self
                .labels
                .into_iter()
                .map(|l| crate::items::Label {
                    name: l.name,
                    color: l.color.unwrap_or_default(),
                })
                .collect(),
            assignees: self.assignees.into_iter().map(|u| u.login).collect(),
        }
    }
}

/// Each page supplies the rollup and every merge-panel decision that GitHub
/// computes from branch protection and repository settings. The client
/// rejects later pages from a different head before assembling one overview.
pub(crate) const PULL_REQUEST_OVERVIEW_QUERY: &str = r#"
query PullRequestOverview($owner: String!, $name: String!, $number: Int!, $contextsCursor: String) {
  repository(owner: $owner, name: $name) {
    merge_commit_allowed: mergeCommitAllowed
    squash_merge_allowed: squashMergeAllowed
    rebase_merge_allowed: rebaseMergeAllowed
    viewer_permission: viewerPermission
    pull_request: pullRequest(number: $number) {
      head_sha: headRefOid
      mergeable
      merge_state: mergeStateStatus
      review_decision: reviewDecision
      viewer_can_merge_as_admin: viewerCanMergeAsAdmin
      squash_title: viewerMergeHeadlineText(mergeType: SQUASH)
      squash_body: viewerMergeBodyText(mergeType: SQUASH)
      merge_title: viewerMergeHeadlineText(mergeType: MERGE)
      merge_body: viewerMergeBodyText(mergeType: MERGE)
      status_check_rollup: statusCheckRollup {
        state
        contexts(first: 100, after: $contextsCursor) {
          page_info: pageInfo {
            has_next_page: hasNextPage
            end_cursor: endCursor
          }
          nodes {
            __typename
            ... on CheckRun {
              name
              status
              conclusion
              details_url: detailsUrl
              started_at: startedAt
              completed_at: completedAt
              check_suite: checkSuite {
                database_id: databaseId
                status
                workflow_run: workflowRun {
                  event
                  workflow { name }
                }
              }
            }
            ... on StatusContext {
              context
              state
              description
              target_url: targetUrl
            }
          }
        }
      }
    }
  }
}
"#;

#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestOverviewQuery {
    #[serde(default)]
    repository: Option<OverviewRepositoryPayload>,
}

impl PullRequestOverviewQuery {
    pub(crate) fn into_page(self) -> Option<PullRequestOverviewPage> {
        self.repository?.into_page()
    }
}

#[derive(Debug, Deserialize)]
struct OverviewRepositoryPayload {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    #[serde(default)]
    viewer_permission: Option<String>,
    #[serde(default)]
    pull_request: Option<OverviewPullPayload>,
}

impl OverviewRepositoryPayload {
    fn into_page(self) -> Option<PullRequestOverviewPage> {
        let pull = self.pull_request?;
        let (check_state, check_contexts, check_page_info) = match pull.status_check_rollup {
            Some(rollup) => (
                Some(rollup.state),
                rollup.contexts.nodes,
                rollup.contexts.page_info,
            ),
            None => (None, Vec::new(), CheckContextPageInfoPayload::default()),
        };
        Some(PullRequestOverviewPage {
            head_sha: pull.head_sha,
            mergeable: mergeable_state(&pull.mergeable),
            merge_state: merge_state(&pull.merge_state),
            review_decision: pull.review_decision.as_deref().and_then(review_decision),
            viewer_can_merge_as_admin: pull.viewer_can_merge_as_admin,
            viewer_permission: self.viewer_permission.as_deref().map(repository_permission),
            allowed_merge_methods: MergeMethods {
                merge: self.merge_commit_allowed,
                squash: self.squash_merge_allowed,
                rebase: self.rebase_merge_allowed,
            },
            squash_message: CommitMessage {
                title: pull.squash_title,
                body: pull.squash_body,
            },
            merge_message: CommitMessage {
                title: pull.merge_title,
                body: pull.merge_body,
            },
            check_state,
            check_contexts,
            check_page_info,
        })
    }
}

/// One page of the status rollup plus the merge metadata returned alongside
/// it. The public overview is assembled only after every context page has
/// been fetched for the same head SHA.
#[derive(Debug)]
pub(crate) struct PullRequestOverviewPage {
    head_sha: String,
    mergeable: MergeableState,
    merge_state: MergeState,
    review_decision: Option<ReviewDecision>,
    viewer_can_merge_as_admin: bool,
    viewer_permission: Option<RepositoryPermission>,
    allowed_merge_methods: MergeMethods,
    squash_message: CommitMessage,
    merge_message: CommitMessage,
    check_state: Option<String>,
    check_contexts: Vec<Option<CheckContextPayload>>,
    check_page_info: CheckContextPageInfoPayload,
}

impl PullRequestOverviewPage {
    fn next_contexts_cursor(&self) -> Result<Option<String>, String> {
        if !self.check_page_info.has_next_page {
            return Ok(None);
        }
        self.check_page_info
            .end_cursor
            .as_deref()
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_owned)
            .map(Some)
            .ok_or_else(|| {
                "GitHub said more check contexts exist but returned no end cursor".into()
            })
    }
}

/// Collects GraphQL connection pages while keeping merge decisions tied to a
/// single pull-request head. Metadata and aggregate state use the latest
/// response, while check rows are combined before retry deduplication.
pub(crate) struct PullRequestOverviewAccumulator {
    expected_head_sha: String,
    latest: PullRequestOverviewPage,
    check_contexts: Vec<Option<CheckContextPayload>>,
    next_contexts_cursor: Option<String>,
}

impl PullRequestOverviewAccumulator {
    pub(crate) fn new(mut page: PullRequestOverviewPage) -> Result<Self, String> {
        let next_contexts_cursor = page.next_contexts_cursor()?;
        let check_contexts = std::mem::take(&mut page.check_contexts);
        Ok(Self {
            expected_head_sha: page.head_sha.clone(),
            latest: page,
            check_contexts,
            next_contexts_cursor,
        })
    }

    pub(crate) fn next_contexts_cursor(&self) -> Option<&str> {
        self.next_contexts_cursor.as_deref()
    }

    pub(crate) fn push(&mut self, mut page: PullRequestOverviewPage) -> Result<(), String> {
        if page.head_sha != self.expected_head_sha {
            return Err(format!(
                "pull request head changed while loading checks (expected {}, found {})",
                self.expected_head_sha, page.head_sha
            ));
        }
        if page.check_state.is_none() {
            return Err("pull request check rollup disappeared while loading its pages".into());
        }
        let next_contexts_cursor = page.next_contexts_cursor()?;
        self.check_contexts.append(&mut page.check_contexts);
        self.latest = page;
        self.next_contexts_cursor = next_contexts_cursor;
        Ok(())
    }

    pub(crate) fn into_overview(self) -> PullRequestOverview {
        PullRequestOverview {
            head_sha: self.latest.head_sha,
            mergeable: self.latest.mergeable,
            merge_state: self.latest.merge_state,
            review_decision: self.latest.review_decision,
            viewer_can_merge_as_admin: self.latest.viewer_can_merge_as_admin,
            viewer_permission: self.latest.viewer_permission,
            allowed_merge_methods: self.latest.allowed_merge_methods,
            checks: checks_from_contexts(self.latest.check_state, self.check_contexts),
            squash_message: self.latest.squash_message,
            merge_message: self.latest.merge_message,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OverviewPullPayload {
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    mergeable: String,
    #[serde(default)]
    merge_state: String,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    viewer_can_merge_as_admin: bool,
    #[serde(default)]
    squash_title: String,
    #[serde(default)]
    squash_body: String,
    #[serde(default)]
    merge_title: String,
    #[serde(default)]
    merge_body: String,
    #[serde(default)]
    status_check_rollup: Option<StatusCheckRollupPayload>,
}

#[derive(Debug, Deserialize)]
struct StatusCheckRollupPayload {
    #[serde(default)]
    state: String,
    #[serde(default)]
    contexts: CheckContextConnectionPayload,
}

#[derive(Debug, Default, Deserialize)]
struct CheckContextConnectionPayload {
    #[serde(default)]
    nodes: Vec<Option<CheckContextPayload>>,
    #[serde(default)]
    page_info: CheckContextPageInfoPayload,
}

#[derive(Debug, Default, Deserialize)]
struct CheckContextPageInfoPayload {
    #[serde(default)]
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
enum CheckContextPayload {
    CheckRun {
        #[serde(default)]
        name: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        conclusion: Option<String>,
        #[serde(default)]
        details_url: Option<String>,
        #[serde(default)]
        started_at: Option<Timestamp>,
        #[serde(default)]
        completed_at: Option<Timestamp>,
        #[serde(default)]
        check_suite: Option<CheckSuitePayload>,
    },
    StatusContext {
        #[serde(default)]
        context: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        target_url: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct CheckSuitePayload {
    #[serde(default)]
    database_id: Option<u64>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    workflow_run: Option<WorkflowRunRefPayload>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunRefPayload {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    workflow: Option<WorkflowRefPayload>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRefPayload {
    #[serde(default)]
    name: String,
}

fn checks_from_contexts(
    aggregate_state: Option<String>,
    contexts: Vec<Option<CheckContextPayload>>,
) -> Checks {
    let Some(aggregate_state) = aggregate_state else {
        return Checks {
            state: CheckState::Unknown,
            url: None,
            runs: Vec::new(),
        };
    };
    let parsed: Vec<ParsedCheck> = contexts
        .into_iter()
        .flatten()
        .filter_map(CheckContextPayload::into_check)
        .collect();
    let current = current_suites(&parsed);
    let current_names: std::collections::HashSet<(u64, &str)> = parsed
        .iter()
        .filter_map(|p| Some((p.suite?.id, p.check.name.as_str())))
        .collect();

    let mut runs = Vec::<Check>::new();
    let mut check_run_keys = std::collections::HashMap::<CheckRunKey, usize>::new();
    let mut superseded = Vec::<Check>::new();
    for ParsedCheck { check, key, suite } in &parsed {
        // A workflow run on the same commit, for the same event, that
        // started later stands for the workflow. While it runs, a job it
        // has not got to yet is waiting, not whatever the older run said;
        // once it is done, it is the older run's word or nothing.
        if let (Some(suite), Some((workflow, event, _))) = (suite, key) {
            let running = current
                .get(&(workflow.as_str(), event.as_str()))
                .filter(|newer| newer.id != suite.id && !newer.done);
            if let Some(newer) = running {
                if !current_names.contains(&(newer.id, check.name.as_str())) {
                    superseded.push(Check {
                        state: CheckRunState::Waiting,
                        url: None,
                        started_at: None,
                        completed_at: None,
                        ..check.clone()
                    });
                }
                continue;
            }
        }
        let check = check.clone();
        if let Some(key) = key.clone() {
            match check_run_keys.get(&key).copied() {
                Some(index) if is_newer_check(&check, &runs[index]) => runs[index] = check,
                Some(_) => {}
                None => {
                    check_run_keys.insert(key, runs.len());
                    runs.push(check);
                }
            }
        } else {
            runs.push(check);
        }
    }
    // An older run may hold the same job more than once, as its attempts.
    for check in superseded {
        let key = (
            check.workflow.clone().unwrap_or_default(),
            check.event.clone().unwrap_or_default(),
            check.name.clone(),
        );
        if let std::collections::hash_map::Entry::Vacant(entry) = check_run_keys.entry(key) {
            entry.insert(runs.len());
            runs.push(check);
        }
    }

    let state = match aggregate_state.as_str() {
        "SUCCESS" => CheckState::Passing,
        "PENDING" | "EXPECTED" => CheckState::Running,
        "FAILURE" | "ERROR" => CheckState::Failing {
            workflow: runs
                .iter()
                .find(|check| check.state.is_failing())
                .and_then(|check| check.workflow.clone().or_else(|| Some(check.name.clone())))
                .unwrap_or_else(|| "checks".into()),
        },
        _ => CheckState::Unknown,
    };
    let preferred = match state {
        CheckState::Failing { .. } => runs.iter().find(|check| check.state.is_failing()),
        CheckState::Running => runs
            .iter()
            .find(|check| check.state.is_pending() && check.url.is_some()),
        _ => None,
    };
    let url = preferred
        .or_else(|| runs.iter().find(|check| check.url.is_some()))
        .and_then(|check| check.url.clone());
    Checks { state, url, runs }
}

/// Per workflow and event, the run that stands for it: one still running
/// before one that is done, else the one whose jobs started last.
fn current_suites(parsed: &[ParsedCheck]) -> std::collections::HashMap<(&str, &str), SuiteRef> {
    type Rank = (bool, Option<Timestamp>, u64);
    let mut latest = std::collections::HashMap::<(&str, &str), (Rank, SuiteRef)>::new();
    for p in parsed {
        let (Some(suite), Some((workflow, event, _))) = (p.suite, &p.key) else {
            continue;
        };
        let rank = (!suite.done, p.check.started_at, suite.id);
        let entry = latest
            .entry((workflow.as_str(), event.as_str()))
            .or_insert((rank, suite));
        if rank > entry.0 {
            *entry = (rank, suite);
        }
    }
    latest.into_iter().map(|(key, (_, suite))| (key, suite)).collect()
}

impl CheckContextPayload {
    fn into_check(self) -> Option<ParsedCheck> {
        match self {
            CheckContextPayload::CheckRun {
                name,
                status,
                conclusion,
                details_url,
                started_at,
                completed_at,
                check_suite,
            } => {
                let (suite_id, suite_done, workflow_run) = match check_suite {
                    Some(suite) => (
                        suite.database_id,
                        suite.status == "COMPLETED",
                        suite.workflow_run,
                    ),
                    None => (None, false, None),
                };
                let suite = suite_id
                    .filter(|_| workflow_run.is_some())
                    .map(|id| SuiteRef { id, done: suite_done });
                let workflow = workflow_run
                    .as_ref()
                    .and_then(|run| run.workflow.as_ref())
                    .map(|workflow| workflow.name.clone())
                    .filter(|name| !name.is_empty());
                let event = workflow_run
                    .and_then(|run| run.event)
                    .filter(|event| !event.is_empty());
                let key = (
                    workflow.clone().unwrap_or_default(),
                    event.clone().unwrap_or_default(),
                    name.clone(),
                );
                Some(ParsedCheck {
                    check: Check {
                        name,
                        workflow,
                        event,
                        state: check_run_state(&status, conclusion.as_deref()),
                        description: None,
                        url: details_url.filter(|url| !url.is_empty()),
                        started_at,
                        completed_at,
                        kind: CheckKind::CheckRun,
                    },
                    key: Some(key),
                    suite,
                })
            }
            CheckContextPayload::StatusContext {
                context,
                state,
                description,
                target_url,
            } => Some(ParsedCheck {
                check: Check {
                    name: context,
                    workflow: None,
                    event: None,
                    state: status_context_state(&state),
                    description,
                    url: target_url.filter(|url| !url.is_empty()),
                    started_at: None,
                    completed_at: None,
                    kind: CheckKind::StatusContext,
                },
                key: None,
                suite: None,
            }),
            CheckContextPayload::Unknown => None,
        }
    }
}

fn is_newer_check(candidate: &Check, current: &Check) -> bool {
    match (candidate.started_at, current.started_at) {
        (Some(candidate), Some(current)) => candidate > current,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Each check by kind and name, in its state: what a rollup and the REST
/// listings of the same commit can be told apart by.
pub(crate) type CheckSighting = std::collections::HashSet<(CheckKind, String, CheckRunState)>;

/// The checks in a rollup, sighted.
pub(crate) fn check_sighting(checks: &Checks) -> CheckSighting {
    checks
        .runs
        .iter()
        .map(|check| (check.kind, check.name.clone(), check.state))
        .collect()
}

/// The checks in a commit's REST check-run and combined-status listings,
/// sighted the way the rollup's are. None when either does not read, or
/// holds more than the page it came in.
pub(crate) fn rest_check_sighting(runs: &str, statuses: &str) -> Option<CheckSighting> {
    #[derive(Deserialize)]
    struct Runs {
        total_count: usize,
        check_runs: Vec<Run>,
    }
    #[derive(Deserialize)]
    struct Run {
        name: String,
        status: String,
        conclusion: Option<String>,
    }
    #[derive(Deserialize)]
    struct Statuses {
        total_count: usize,
        statuses: Vec<Status>,
    }
    #[derive(Deserialize)]
    struct Status {
        context: String,
        state: String,
    }
    let runs: Runs = serde_json::from_str(runs).ok()?;
    let statuses: Statuses = serde_json::from_str(statuses).ok()?;
    if runs.total_count > runs.check_runs.len() || statuses.total_count > statuses.statuses.len() {
        return None;
    }
    // REST spells GraphQL's enums in lower case.
    let runs = runs.check_runs.into_iter().map(|run| {
        let conclusion = run.conclusion.map(|c| c.to_ascii_uppercase());
        let state = check_run_state(&run.status.to_ascii_uppercase(), conclusion.as_deref());
        (CheckKind::CheckRun, run.name, state)
    });
    let statuses = statuses.statuses.into_iter().map(|status| {
        let state = status_context_state(&status.state.to_ascii_uppercase());
        (CheckKind::StatusContext, status.context, state)
    });
    Some(runs.chain(statuses).collect())
}

fn check_run_state(status: &str, conclusion: Option<&str>) -> CheckRunState {
    match conclusion {
        Some("SUCCESS") => CheckRunState::Success,
        Some("FAILURE") => CheckRunState::Failure,
        Some("NEUTRAL") => CheckRunState::Neutral,
        Some("SKIPPED") => CheckRunState::Skipped,
        Some("CANCELLED") => CheckRunState::Cancelled,
        Some("TIMED_OUT") => CheckRunState::TimedOut,
        Some("ACTION_REQUIRED") => CheckRunState::ActionRequired,
        Some("STARTUP_FAILURE") => CheckRunState::StartupFailure,
        Some("STALE") => CheckRunState::Stale,
        Some(_) => CheckRunState::Unknown,
        None if matches!(
            status,
            "QUEUED" | "IN_PROGRESS" | "PENDING" | "REQUESTED" | "WAITING"
        ) =>
        {
            CheckRunState::Pending
        }
        None => CheckRunState::Unknown,
    }
}

fn status_context_state(state: &str) -> CheckRunState {
    match state {
        "SUCCESS" => CheckRunState::Success,
        "FAILURE" | "ERROR" => CheckRunState::Failure,
        "EXPECTED" | "PENDING" => CheckRunState::Pending,
        _ => CheckRunState::Unknown,
    }
}

fn mergeable_state(state: &str) -> MergeableState {
    match state {
        "MERGEABLE" => MergeableState::Mergeable,
        "CONFLICTING" => MergeableState::Conflicting,
        _ => MergeableState::Unknown,
    }
}

fn merge_state(state: &str) -> MergeState {
    match state {
        "BEHIND" => MergeState::Behind,
        "BLOCKED" => MergeState::Blocked,
        "CLEAN" => MergeState::Clean,
        "DIRTY" => MergeState::Dirty,
        "DRAFT" => MergeState::Draft,
        "HAS_HOOKS" => MergeState::HasHooks,
        "UNSTABLE" => MergeState::Unstable,
        _ => MergeState::Unknown,
    }
}

fn review_decision(decision: &str) -> Option<ReviewDecision> {
    match decision {
        "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequested),
        "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
        "APPROVED" => Some(ReviewDecision::Approved),
        _ => None,
    }
}

pub(crate) fn repository_permission(permission: &str) -> RepositoryPermission {
    match permission {
        "ADMIN" => RepositoryPermission::Admin,
        "MAINTAIN" => RepositoryPermission::Maintain,
        "WRITE" => RepositoryPermission::Write,
        "TRIAGE" => RepositoryPermission::Triage,
        "READ" => RepositoryPermission::Read,
        _ => RepositoryPermission::Unknown,
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MergeRequest<'a> {
    pub sha: &'a str,
    pub merge_method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "number": 2091,
        "title": "RFC 0035 - incremental compaction",
        "html_url": "https://github.com/slatedb/slatedb/pull/2091",
        "state": "open",
        "draft": false,
        "merged": false,
        "additions": 412,
        "deletions": 87,
        "changed_files": 9,
        "user": { "login": "criccomini", "avatar_url": "https://avatars/1" },
        "head": {
            "sha": "abc123",
            "ref": "feature/incremental",
            "repo": { "full_name": "someone/slatedb" }
        },
        "base": {
            "sha": "def456",
            "ref": "main",
            "repo": { "full_name": "slatedb/slatedb" }
        }
    }"#;

    #[test]
    fn a_pull_request_payload_maps_to_what_a_checkout_needs() {
        let payload: PullPayload = serde_json::from_str(SAMPLE).unwrap();
        let pr = payload.into_pull_request("fallback/repo");

        assert_eq!(pr.number, 2091);
        assert_eq!(pr.html_url, "https://github.com/slatedb/slatedb/pull/2091");
        assert_eq!(pr.head_sha, "abc123");
        assert_eq!(pr.head_ref, "feature/incremental");
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.author_login, "criccomini");
        assert!(pr.is_open());
    }

    #[test]
    fn a_fork_still_clones_the_base_repository() {
        // refs/pull/N/head lives on the base repo, not the contributor's fork.
        let payload: PullPayload = serde_json::from_str(SAMPLE).unwrap();
        let pr = payload.into_pull_request("fallback/repo");
        assert_eq!(pr.repo_full_name, "slatedb/slatedb");
    }

    #[test]
    fn a_missing_base_repo_falls_back_to_the_notification_repository() {
        let json = SAMPLE.replace(r#", "repo": { "full_name": "slatedb/slatedb" }"#, "");
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("slatedb/slatedb");
        assert_eq!(pr.repo_full_name, "slatedb/slatedb");
    }

    #[test]
    fn a_missing_author_is_not_an_error() {
        let json = SAMPLE.replace(
            r#""user": { "login": "criccomini", "avatar_url": "https://avatars/1" },"#,
            "",
        );
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("a/b");
        assert_eq!(pr.author_login, "");
    }

    #[test]
    fn a_merged_pull_request_is_not_open() {
        let json = SAMPLE
            .replace(r#""state": "open""#, r#""state": "closed""#)
            .replace(r#""merged": false"#, r#""merged": true"#);
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("a/b");
        assert!(!pr.is_open());
        assert!(pr.merged);
    }

    #[test]
    fn the_fields_the_header_shows_are_read_when_present_and_empty_when_not() {
        let bare: PullPayload = serde_json::from_str(SAMPLE).unwrap();
        let pr = bare.into_pull_request("a/b");
        assert_eq!(pr.merged_at, None);
        assert_eq!(pr.merged_by, None);
        assert_eq!(pr.closed_at, None);
        assert!(pr.requested_reviewers.is_empty());
        assert_eq!(pr.milestone, None);

        let json = SAMPLE.replace(
            r#""merged": false,"#,
            r#""merged": true,
            "merged_at": "2026-09-17T15:00:00Z",
            "merged_by": { "login": "rohan", "avatar_url": "https://avatars/2" },
            "closed_at": "2026-09-17T15:00:00Z",
            "requested_reviewers": [ { "login": "chris", "avatar_url": "https://avatars/3" } ],
            "requested_teams": [ { "slug": "storage" } ],
            "milestone": { "title": "0.9", "number": 4 },"#,
        );
        let payload: PullPayload = serde_json::from_str(&json).unwrap();
        let pr = payload.into_pull_request("a/b");
        assert_eq!(pr.merged_at, Some("2026-09-17T15:00:00Z".parse().unwrap()));
        assert_eq!(pr.merged_by.as_deref(), Some("rohan"));
        assert_eq!(pr.closed_at, Some("2026-09-17T15:00:00Z".parse().unwrap()));
        assert_eq!(
            pr.requested_reviewers,
            vec![Reviewer {
                login: "chris".into(),
                avatar_url: "https://avatars/3".into()
            }]
        );
        assert_eq!(pr.milestone.as_deref(), Some("0.9"));
        assert!(pr.review_requested);
    }

    #[test]
    fn a_snapshot_written_before_the_header_fields_still_loads() {
        let payload: PullPayload = serde_json::from_str(SAMPLE).unwrap();
        let pr = payload.into_pull_request("a/b");
        let mut old: serde_json::Value = serde_json::to_value(&pr).unwrap();
        for field in [
            "html_url",
            "merged_at",
            "merged_by",
            "closed_at",
            "requested_reviewers",
            "milestone",
        ] {
            old.as_object_mut().unwrap().remove(field);
        }
        let loaded: PullRequest = serde_json::from_value(old).unwrap();
        assert_eq!(loaded.html_url, "");
        assert_eq!(loaded.merged_at, pr.merged_at);
        assert_eq!(loaded.requested_reviewers, pr.requested_reviewers);
    }

    /// slatedb#2122: two runs of PR Checks on one head, the second
    /// cancelled and then re-run. The rollup holds both; the first run's
    /// passes must not stand in for jobs the re-run has not reached.
    fn rerun_rollup(rerun_status: &str) -> Checks {
        let run = |name: &str, status: &str, conclusion: &str, started: &str, suite: u64| {
            let suite_status = if suite == 2 { rerun_status } else { "COMPLETED" };
            serde_json::json!({
                "__typename": "CheckRun",
                "name": name,
                "status": status,
                "conclusion": if conclusion.is_empty() { None } else { Some(conclusion) },
                "details_url": format!("https://github.com/a/b/actions/runs/{suite}/job/{name}"),
                "started_at": started,
                "check_suite": {
                    "database_id": suite,
                    "status": suite_status,
                    "workflow_run": {
                        "event": "pull_request",
                        "workflow": { "name": "PR Checks" },
                    },
                },
            })
        };
        let contexts = serde_json::json!([
            run("tests", "IN_PROGRESS", "", "2026-10-01T18:54:58Z", 2),
            run("tests", "COMPLETED", "SUCCESS", "2026-10-01T18:23:36Z", 1),
            run("style", "COMPLETED", "SUCCESS", "2026-10-01T18:54:57Z", 2),
            run("style", "COMPLETED", "SUCCESS", "2026-10-01T18:23:40Z", 1),
            run("bindings-go", "COMPLETED", "SUCCESS", "2026-10-01T18:37:43Z", 1),
            {"__typename": "StatusContext", "context": "license/cla", "state": "SUCCESS"},
        ]);
        checks_from_contexts(Some("PENDING".into()), serde_json::from_value(contexts).unwrap())
    }

    #[test]
    fn a_rerun_leaves_the_jobs_it_has_not_reached_waiting() {
        let checks = rerun_rollup("IN_PROGRESS");
        let row = |name: &str| checks.runs.iter().find(|c| c.name == name).unwrap();

        assert_eq!(checks.runs.len(), 4);
        assert_eq!(row("tests").state, CheckRunState::Pending);
        assert_eq!(row("style").started_at, Some("2026-10-01T18:54:57Z".parse().unwrap()));
        let waiting = row("bindings-go");
        assert_eq!(waiting.state, CheckRunState::Waiting);
        assert_eq!((waiting.url.as_deref(), waiting.completed_at), (None, None));
        assert_eq!(row("license/cla").state, CheckRunState::Success);
        assert_eq!(
            checks.url.as_deref(),
            Some("https://github.com/a/b/actions/runs/2/job/tests")
        );
    }

    #[test]
    fn a_finished_rerun_keeps_what_the_older_run_alone_ran() {
        let checks = rerun_rollup("COMPLETED");
        let go = checks.runs.iter().find(|c| c.name == "bindings-go").unwrap();
        assert_eq!(go.state, CheckRunState::Success);
    }

    #[test]
    fn the_rest_listings_agree_with_the_rollup_they_were_read_beside() {
        let rollup = checks_from_contexts(
            Some("PENDING".into()),
            serde_json::from_str(
                r#"[
                    {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},
                    {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":null},
                    {"__typename":"StatusContext","context":"ci/legacy","state":"SUCCESS"}
                ]"#,
            )
            .unwrap(),
        );
        let statuses = r#"{"state":"success","total_count":1,
            "statuses":[{"context":"ci/legacy","state":"success"}]}"#;
        let runs = |lint: &str| {
            format!(
                r#"{{"total_count":2,"check_runs":[
                    {{"name":"test","status":"completed","conclusion":"success"}},
                    {{"name":"lint",{lint}}}]}}"#
            )
        };

        let same = rest_check_sighting(&runs(r#""status":"in_progress","conclusion":null"#), statuses);
        assert_eq!(same, Some(check_sighting(&rollup)));

        let moved = rest_check_sighting(&runs(r#""status":"completed","conclusion":"failure""#), statuses);
        assert_ne!(moved, Some(check_sighting(&rollup)));

        // A listing cut at its page says nothing.
        let cut = r#"{"total_count":101,"check_runs":[]}"#;
        assert_eq!(rest_check_sighting(cut, statuses), None);
    }
}
