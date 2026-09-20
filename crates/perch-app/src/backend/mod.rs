//! The background half of the app.
//!
//! Owns the tokio runtime, the GitHub client, and the SQLite cache. The UI
//! never touches any of them: it sends `Command`s and receives `Event`s.
//!
//! Commands travel on a tokio channel, which can be sent to from the UI
//! thread without a runtime context. Events travel back on a std channel,
//! because that is what floem's `update_signal_from_channel` consumes.

use std::path::Path;
use std::sync::mpsc::Sender as EventSender;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use perch_git::{Preparer, RepoId, ReviewSnapshotStore, Store};
use perch_github::{
    resolve_token, Anchor, Author, Cache, CacheError, CacheOpen, CommentMessage, CommentThread,
    Config, GitHub, Item, ItemCheckout, ItemKind, ItemSubject, MergeMethod, MergeState,
    MergeableState, NewRunEvent, PullRequest, PullRequestOverview, RepositoryPermission,
    RevisionContext, RunEvent as StoredRunEvent, RunEventRecord, RunHistoryFilter, RunLabels,
    RunOwner, RunPurpose, RunRecord, RunStatus, RunSummary, SubjectKind, TerminalReason, Thread,
    ThreadOrigin,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Semaphore;

pub(crate) mod reviews;
pub(crate) mod runs;

/// How many avatar downloads may be in flight at once.
const AVATAR_CONCURRENCY: usize = 6;
/// Conditional notification polls are cheap, but GitHub's validator does not
/// reliably change when inbox membership does. Periodically fetch the whole
/// representation so a stale cached list heals without a restart or click.
const NOTIFICATION_RECONCILE_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Settings the UI keeps between launches. Sent back at startup. Show
/// whole is not here: its key is per item, `review.folded.<item_id>`, so
/// it is read when the item opens and carried on `ViewedReady`.
pub const UI_META_KEYS: [&str; 6] = [
    "item_filters",
    "review.layout",
    "agents.open",
    "agents.width",
    "viewer.login",
    "viewer.avatar_url",
];

#[derive(Debug, Clone)]
pub enum Command {
    Refresh,
    /// Open an item's view: the review for a pull request or a branch, the
    /// issue view for an issue. Prepares the checkout when there is one to
    /// prepare.
    OpenItem {
        id: String,
        navigation_id: u64,
    },
    /// Open an agent's owner for reading. Archived items use their cached
    /// conversation without restoring a checkout or clearing Archive.
    ViewAgentOwner {
        id: String,
        navigation_id: u64,
    },
    /// Open a reviewer child's exact retained group. The backend resolves
    /// attempt ownership so Agents navigation does not depend on whichever
    /// review dashboard happens to be loaded in the UI.
    ViewReviewAttemptOwner {
        item_id: String,
        attempt_id: String,
        request_id: u64,
    },
    /// Find or make the item for a pull request or issue named by number,
    /// and open it. Every sidebar row opens by id; this is for the launch
    /// hooks and the review header's "Closes #N" chip, which name a subject
    /// that may have no row yet.
    OpenSubject {
        repo_full_name: String,
        kind: ItemKind,
        number: u64,
        navigation_id: u64,
    },
    /// Mark a GitHub notification read and open its pull request or issue.
    /// Other subject kinds are opened by the UI in the browser instead.
    OpenNotification {
        thread: Thread,
        navigation_id: Option<u64>,
    },
    /// Keep a notification visually unread in Perch. GitHub's public API
    /// has no per-thread mark-unread operation.
    MarkNotificationUnread {
        thread_id: String,
    },
    /// Remove a notification from GitHub's inbox.
    DoneNotification {
        thread_id: String,
    },
    /// Ignore future notifications for the thread, then remove this one
    /// from GitHub's inbox.
    UnsubscribeNotification {
        thread_id: String,
    },
    /// Merge the exact pull request head the reader confirmed. GitHub
    /// performs the merge; `bypass_requirements` records the explicit UI
    /// consent needed before an authorized rules bypass is attempted.
    MergePullRequest {
        attempt_id: u64,
        item_id: String,
        repo_full_name: String,
        number: u64,
        expected_head_sha: String,
        method: MergeMethod,
        bypass_requirements: bool,
    },
    /// Read, diff, and highlight one file of the open checkout, over the
    /// scope that is set.
    OpenFile {
        path: String,
        navigation_id: u64,
    },
    /// What the diff is over: the branch, a commit, the uncommitted work.
    /// The changed list is recomputed and every open file reloads.
    SetScope {
        item_id: String,
        scope: perch_git::Scope,
        request_id: u64,
        navigation_id: u64,
    },
    /// Look at the checkout again: its head, its dirt and its commits, and
    /// the changed list if the head moved. Sent as the scope menu is about
    /// to open, and on every poll, so terminal work shows up.
    RefreshScope {
        item_id: String,
        navigation_id: u64,
        scope_revision: u64,
    },
    /// Check or uncheck a file's content as viewed.
    SetViewed {
        item_id: String,
        path: String,
        blob: String,
        viewed: bool,
    },
    /// Remember that head lines `first` to `last` of a file's content were
    /// opened out of a fold, and forget every reveal overlapping a range:
    /// a divider's Reset. Both keyed as Viewed is.
    Reveal {
        item_id: String,
        path: String,
        blob: String,
        first: u32,
        last: u32,
    },
    ResetReveals {
        item_id: String,
        path: String,
        blob: String,
        first: u32,
        last: u32,
    },
    /// Ask the language server about a position. `token` lets the UI drop an
    /// answer that arrives after the pointer has moved on.
    Hover {
        token: u64,
        path: String,
        line: u32,
        character: u32,
    },
    Definition {
        token: u64,
        path: String,
        line: u32,
        character: u32,
    },
    References {
        token: u64,
        path: String,
        line: u32,
        character: u32,
    },
    CloseReview {
        navigation_id: u64,
    },

    /// Leave Active. An owned checkout's directory goes too, refused when
    /// there is uncommitted work in it; a checkout under a root is left
    /// alone and only the row is hidden.
    ArchiveItem {
        id: String,
        suspended_navigation: SuspendedNavigation,
    },
    /// Back to Active, and an owned checkout back on disk.
    RestoreItem {
        id: String,
        navigation_id: u64,
    },
    /// Remove the row and its threads. An owned checkout's directory and
    /// branch go too; anything under a root is not touched.
    DeleteItem {
        id: String,
    },
    /// Keep or remove a local bookmark for an item.
    SetSaved {
        id: String,
        saved: bool,
    },
    PinItem {
        id: String,
        pinned: bool,
    },
    RenameItem {
        id: String,
        name: Option<String>,
    },
    /// Cut `perch/issue-<n>` from the default branch and make a
    /// checkout-only item pointing back at the issue.
    StartWork {
        issue_item_id: String,
        navigation_id: u64,
    },
    /// Cut a branch from the item's head into a new checkout-only item.
    ForkItem {
        id: String,
        navigation_id: u64,
    },
    /// Remember a setting across launches.
    SetMeta {
        key: String,
        value: String,
    },

    /// Ask the agent a question in an item's checkout. `thread_id` of `None`
    /// starts a thread; the anchor is ignored on a follow-up.
    Ask {
        item_id: String,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        question: String,
        backend: perch_agent::Backend,
        model: String,
        effort: perch_agent::Effort,
    },
    CancelRun {
        run_id: String,
    },
    RetryRun {
        run_id: String,
    },
    /// Load native-review capabilities, preferences, local bases, and the
    /// retained history for one worktree.
    LoadReviewDashboard {
        item_id: String,
    },
    StartReview {
        request: crate::review::model::ReviewLaunchRequest,
    },
    SaveReviewPreference {
        choice: crate::review::model::ReviewPreferenceChoice,
    },
    StopReviewGroup {
        group_id: String,
    },
    RetryReviewAttempt {
        attempt_id: String,
    },
    OpenReviewSnapshot {
        group_id: String,
        request_id: u64,
    },
    DismissReviewFinding {
        finding_id: String,
        reason: String,
    },
    ReopenReviewFinding {
        finding_id: String,
    },
    LinkReviewDuplicate {
        finding_id: String,
        canonical_finding_id: String,
    },
    UnlinkReviewDuplicate {
        finding_id: String,
    },
    LoadAgentRuns {
        history_limit: usize,
    },
    LoadAgentRunDetail {
        run_id: String,
    },
    LoadAgentRunEvents {
        run_id: String,
        before_sequence: u64,
    },
    DeleteCommentThread {
        id: String,
    },
}

#[derive(Debug, Clone)]
pub enum Event {
    Avatar {
        url: String,
        bytes: Vec<u8>,
    },
    /// Declared repository identities and display names for filters and
    /// item context. Sent once at startup; the file is read once.
    Repos(Vec<perch_github::DeclaredRepo>),
    /// Every item, active and archived. Whole, for the same reason.
    Items(Vec<Item>),
    /// Perch's visible notification list, in GitHub response order. Read
    /// threads remain; revisions marked Done in Perch stay hidden until newer
    /// activity arrives.
    Notifications(Vec<Thread>),
    /// A remembered setting, sent once at startup for each key the UI
    /// asked to keep.
    Meta {
        key: String,
        value: String,
    },
    /// An issue is open: its snapshot, its comments, and the item.
    IssueReady {
        navigation_id: u64,
        scope_gen: u64,
        item: Arc<Item>,
        issue: Arc<perch_github::Issue>,
        comments: Arc<Vec<perch_github::IssueComment>>,
        /// True when the fetch failed and this is the snapshot alone.
        offline: bool,
    },
    /// Something worth a line in the status bar and nothing more.
    Notice(String),
    /// What config.toml says about the agent, for the pickers.
    AgentConfig(perch_github::AgentConfig),
    /// Every thread on an item. Whole, like `Items`.
    CommentThreads {
        item_id: String,
        threads: Vec<CommentThread>,
        /// Present when the threads were projected for one immutable
        /// historical review request. The UI rejects a projection from an
        /// older request even when it belongs to the same open item.
        review_group_id: Option<String>,
        review_request_id: Option<u64>,
    },
    /// One step of a run, applied in place to the run it names.
    AgentStep {
        run_id: String,
        thread_id: String,
        event: perch_agent::RunEvent,
    },
    /// App-wide run summaries. Every active run is included; terminal
    /// history is the requested newest-first prefix.
    AgentRuns {
        runs: Vec<RunSummary>,
        has_more: bool,
    },
    /// One durable state change, sent after it has committed.
    AgentRunUpdated {
        run: RunRecord,
    },
    /// The selected run and its ordered persisted activity.
    AgentRunDetail {
        run: RunRecord,
        events: Vec<RunEventRecord>,
        has_more: bool,
    },
    /// One older, bounded page for a detail the reader already opened.
    AgentRunEvents {
        run_id: String,
        events: Vec<RunEventRecord>,
        has_more: bool,
    },
    /// The owning item is ready and the exact review group has been
    /// resolved. This precedes HistoricalReviewReady on the same event
    /// channel so the UI can fence and select the intended snapshot.
    AgentReviewNavigationReady {
        item_id: String,
        attempt_id: String,
        request_id: u64,
        /// True when resolving the attempt first installed a different
        /// owner's live view, which becomes this history load's rollback.
        owner_changed: bool,
        summary: crate::review::model::ReviewGroupSummary,
    },
    AgentReviewNavigationFailed {
        item_id: String,
        attempt_id: String,
        request_id: u64,
        message: String,
    },
    /// A navigation resolved to an item that an earlier archive fence owns.
    NavigationCancelled {
        item_id: String,
        navigation_id: u64,
    },
    /// Archive committed and names the exact pending navigation it fenced.
    ItemArchived {
        item_id: String,
        cancelled_navigation_id: Option<u64>,
    },
    ReviewDashboard {
        item_id: String,
        capabilities: Vec<perch_agent::ReviewCapability>,
        preferences: Vec<perch_github::ReviewPreference>,
        base_refs: Vec<String>,
        groups: Vec<crate::review::model::ReviewGroupSummary>,
        findings: Vec<crate::review::model::ReviewFindingSummary>,
    },
    ReviewGroupUpdated {
        item_id: String,
        summary: crate::review::model::ReviewGroupSummary,
    },
    ReviewFindingUpdated {
        item_id: String,
        summary: crate::review::model::ReviewFindingSummary,
    },
    ReviewLaunchFailed {
        item_id: String,
        message: String,
    },
    /// A retained review's immutable file context. The item itself remains
    /// the current item; only Files switches to this read-only snapshot.
    HistoricalReviewReady {
        item_id: String,
        group_id: String,
        request_id: u64,
        scope_gen: u64,
        files: Arc<Vec<perch_git::ChangedFile>>,
        tree: Arc<Vec<perch_git::TreeEntry>>,
        findings: Arc<Vec<crate::review::model::ReviewFindingSummary>>,
    },
    /// The selected historical load failed. The matching optimistic UI
    /// selection is rolled back without disturbing a newer selection.
    HistoricalReviewFailed {
        item_id: String,
        group_id: String,
        request_id: u64,
        message: String,
    },
    Status(Status),

    /// A step of preparing a checkout.
    ReviewProgress {
        navigation_id: u64,
        message: String,
        percent: Option<u8>,
    },
    ReviewReady {
        navigation_id: u64,
        scope_gen: u64,
        item: Arc<Item>,
        /// `None` for a branch of the reader's own with no pull request.
        pull_request: Option<Arc<PullRequest>>,
        files: Arc<Vec<perch_git::ChangedFile>>,
        tree: Arc<Vec<perch_git::TreeEntry>>,
        /// The head the pull request moved to while the worktree had
        /// uncommitted changes. The files shown are the older ones on disk.
        stale_head: Option<String>,
        /// True when the pull request could not be fetched and the review
        /// is showing the checkout as it was last left.
        offline: bool,
    },
    ReviewFailed {
        navigation_id: u64,
        message: String,
    },
    FileLoaded {
        navigation_id: u64,
        scope_gen: u64,
        item_id: String,
        path: String,
        file: Arc<crate::review::model::LoadedFile>,
    },
    FileFailed {
        navigation_id: u64,
        scope_gen: u64,
        item_id: String,
        path: String,
        message: String,
    },

    /// Everything said on the pull request: from the cache first, then
    /// from GitHub.
    DiscussionReady {
        item_id: String,
        discussion: Arc<perch_github::Discussion>,
    },
    /// An issue's comments, fetched again after a notification about it.
    IssueComments {
        item_id: String,
        comments: Arc<Vec<perch_github::IssueComment>>,
    },
    /// The workflow runs' verdict for the item's head.
    ChecksReady {
        item_id: String,
        repo_full_name: String,
        number: u64,
        head_sha: String,
        status_generation: u64,
        checks: Arc<perch_github::Checks>,
    },
    PullRequestOverviewLoading {
        item_id: String,
        repo_full_name: String,
        number: u64,
        head_sha: String,
        status_generation: u64,
    },
    /// Live status for the pull request's exact head. Cached checks may
    /// precede this, but merge controls only use this fresh answer.
    PullRequestOverviewReady {
        item_id: String,
        repo_full_name: String,
        number: u64,
        status_generation: u64,
        overview: Arc<perch_github::PullRequestOverview>,
    },
    PullRequestOverviewFailed {
        item_id: String,
        repo_full_name: String,
        number: u64,
        head_sha: String,
        status_generation: u64,
        message: String,
    },
    /// A fresh pull request and its durable item after a remote mutation or
    /// notification refresh.
    PullRequestUpdated {
        item_id: String,
        item: Arc<Item>,
        pull_request: Arc<PullRequest>,
        merge_attempt_id: Option<u64>,
    },
    PullRequestMergeStarted {
        item_id: String,
        repo_full_name: String,
        number: u64,
        expected_head_sha: String,
        attempt_id: u64,
    },
    PullRequestMergeFailed {
        item_id: String,
        repo_full_name: String,
        number: u64,
        expected_head_sha: String,
        attempt_id: u64,
        message: String,
    },
    /// The commits over the base and whether the checkout has work in it,
    /// for the scope menu, and the head as it was at the previous open,
    /// which "Since you last looked" ranges from.
    CommitsReady {
        navigation_id: u64,
        scope_gen: u64,
        item_id: String,
        commits: Arc<Vec<perch_git::Commit>>,
        dirt: perch_git::Dirt,
        since: Option<String>,
    },
    /// The changed list under a scope, after `SetScope`.
    ScopeReady {
        navigation_id: u64,
        scope_gen: u64,
        item_id: String,
        scope: perch_git::Scope,
        files: Arc<Vec<perch_git::ChangedFile>>,
    },
    /// What the reader marked on the item's files, whole: every (path,
    /// blob) viewed, and, when the item opens, every run of head lines
    /// revealed out of a fold by (path, blob) and whether the item's files
    /// are folded to their changes, which they are unless the reader asked
    /// for them whole. Sent again after a viewed write with the viewed set
    /// alone: the reveals and the fold are the window's own, applied as
    /// they are clicked, and an answer to an earlier write would put them
    /// back as they were.
    ViewedReady {
        item_id: String,
        viewed: Arc<std::collections::HashSet<(String, String)>>,
        reveals: Option<Arc<perch_github::StoredReveals>>,
        folded: Option<bool>,
    },

    LspStatus {
        navigation_id: u64,
        status: perch_lsp::Status,
    },
    LspHover {
        token: u64,
        text: String,
    },
    /// A definition inside the worktree, or `None` when it lands outside it.
    LspDefinition {
        token: u64,
        path: Option<String>,
        line: u32,
    },
    /// Every use of a symbol inside the worktree, with the line's text, and
    /// how many uses lie outside it.
    LspReferences {
        token: u64,
        references: Vec<crate::review::model::Reference>,
        elsewhere: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No usable token. The app shows setup instructions instead of a list.
    NoToken {
        detail: String,
    },
    Refreshing,
    Idle {
        at: Timestamp,
        source: String,
    },
    /// Network or API trouble. Any cached list stays on screen.
    Error {
        message: String,
    },
    RateLimited {
        message: String,
    },
}

/// Handle the UI keeps. Dropping it ends the worker.
pub struct Backend {
    commands: UnboundedSender<Command>,
    navigation_requests: Arc<NavigationRequests>,
    scope_requests: Arc<ScopeRequests>,
    merge_attempts: Arc<MergeAttempts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NavigationRequest {
    id: u64,
    target: NavigationTarget,
    historical_pending: bool,
    historical_rollback: Option<Box<NavigationRequest>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NavigationTarget {
    Current,
    Item(String),
    Closed,
    Group {
        item_id: String,
        group_id: String,
    },
    ReviewAttempt {
        item_id: String,
        attempt_id: String,
    },
    SuspendedItem(String),
}

/// A same-item navigation held aside while Archive validates the checkout.
/// If Archive is refused, the exact request is restored so the UI and backend
/// keep the same correlation id.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct SuspendedNavigation {
    fence_id: u64,
    item_id: String,
    previous: Option<NavigationRequest>,
}

/// The latest view navigation is registered synchronously by the UI handle,
/// before its command reaches the async worker. Current opens, close, Agents
/// owner jumps, and historical snapshots all share this one sequence so task
/// scheduling cannot reverse the reader's click order.
#[derive(Debug, Default)]
struct NavigationRequests {
    next_id: std::sync::atomic::AtomicU64,
    latest: std::sync::Mutex<Option<NavigationRequest>>,
    archive_fences: std::sync::Mutex<std::collections::HashMap<String, ArchiveFence>>,
}

#[derive(Debug, Clone, Copy)]
struct ArchiveFence {
    id: u64,
    committed: bool,
    previous_id: Option<u64>,
}

impl NavigationRequests {
    fn begin_current(&self) -> u64 {
        self.begin_target(NavigationTarget::Current)
    }

    fn begin_item(&self, item_id: &str) -> u64 {
        self.begin_target(NavigationTarget::Item(item_id.to_string()))
    }

    fn begin_closed(&self) -> u64 {
        self.begin_target(NavigationTarget::Closed)
    }

    fn begin_group(&self, item_id: &str, group_id: &str) -> u64 {
        self.begin_historical_target(NavigationTarget::Group {
            item_id: item_id.to_string(),
            group_id: group_id.to_string(),
        })
    }

    fn begin_attempt(&self, item_id: &str, attempt_id: &str) -> u64 {
        self.begin_historical_target(NavigationTarget::ReviewAttempt {
            item_id: item_id.to_string(),
            attempt_id: attempt_id.to_string(),
        })
    }

    fn begin_target(&self, target: NavigationTarget) -> u64 {
        let Ok(mut latest) = self.latest.lock() else {
            return 0;
        };
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        *latest = Some(NavigationRequest {
            id,
            target,
            historical_pending: false,
            historical_rollback: None,
        });
        id
    }

    fn begin_historical_target(&self, target: NavigationTarget) -> u64 {
        let Ok(mut latest) = self.latest.lock() else {
            return 0;
        };
        let historical_rollback = latest.as_ref().and_then(|request| {
            if request.historical_pending {
                request.historical_rollback.clone()
            } else {
                Some(Box::new(request.clone()))
            }
        });
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        *latest = Some(NavigationRequest {
            id,
            target,
            historical_pending: true,
            historical_rollback,
        });
        id
    }

    fn current_id(&self) -> Option<u64> {
        self.latest
            .lock()
            .ok()
            .and_then(|latest| latest.as_ref().map(|request| request.id))
    }

    fn is_current(&self, request_id: u64) -> bool {
        self.latest.lock().is_ok_and(|latest| {
            latest
                .as_ref()
                .is_some_and(|request| request.id == request_id)
        })
    }

    fn is_current_group(&self, group_id: &str, request_id: u64) -> bool {
        self.latest.lock().is_ok_and(|latest| {
            latest.as_ref().is_some_and(|request| {
                request.id == request_id
                    && matches!(
                        &request.target,
                        NavigationTarget::Group { group_id: current, .. } if current == group_id
                    )
            })
        })
    }

    fn is_current_attempt(&self, attempt_id: &str, request_id: u64) -> bool {
        self.latest.lock().is_ok_and(|latest| {
            latest.as_ref().is_some_and(|request| {
                request.id == request_id
                    && matches!(
                        &request.target,
                        NavigationTarget::ReviewAttempt { attempt_id: current, .. }
                            if current == attempt_id
                    )
            })
        })
    }

    /// Resolve the provisional attempt target while holding the same lock
    /// used by newer UI selections. A stale worker can never mint a newer
    /// historical request after a later click.
    fn bind_attempt_to_group(&self, attempt_id: &str, group_id: &str, request_id: u64) -> bool {
        let Ok(mut latest) = self.latest.lock() else {
            return false;
        };
        let Some(request) = latest.as_mut() else {
            return false;
        };
        if request.id != request_id
            || !matches!(
                &request.target,
                NavigationTarget::ReviewAttempt { attempt_id: current, .. }
                    if current == attempt_id
            )
        {
            return false;
        }
        let item_id = match &request.target {
            NavigationTarget::ReviewAttempt { item_id, .. } => item_id.clone(),
            _ => return false,
        };
        request.target = NavigationTarget::Group {
            item_id,
            group_id: group_id.to_string(),
        };
        true
    }

    /// An attempt navigation may first have to open a different owner. Once
    /// that live owner is installed, a later snapshot failure must retain it
    /// instead of restoring the item that preceded the whole navigation.
    fn checkpoint_attempt_owner(&self, item_id: &str, attempt_id: &str, request_id: u64) -> bool {
        let Ok(mut latest) = self.latest.lock() else {
            return false;
        };
        let Some(request) = latest.as_mut() else {
            return false;
        };
        if request.id != request_id
            || !matches!(
                &request.target,
                NavigationTarget::ReviewAttempt {
                    item_id: current_item,
                    attempt_id: current_attempt,
                } if current_item == item_id && current_attempt == attempt_id
            )
        {
            return false;
        }
        request.historical_rollback = Some(Box::new(NavigationRequest {
            id: request_id,
            target: NavigationTarget::Item(item_id.to_string()),
            historical_pending: false,
            historical_rollback: None,
        }));
        true
    }

    fn suspend_item(&self, item_id: &str) -> SuspendedNavigation {
        let fence_id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        let mut fences = self.archive_fences.lock().ok();
        if let Some(fences) = fences.as_mut() {
            fences.insert(
                item_id.to_string(),
                ArchiveFence {
                    id: fence_id,
                    committed: false,
                    previous_id: None,
                },
            );
        }
        let mut previous = None;
        if let Ok(latest) = self.latest.lock() {
            let targets_item = latest.as_ref().is_some_and(|request| {
                request.id < fence_id
                    && match &request.target {
                        NavigationTarget::Item(current)
                        | NavigationTarget::SuspendedItem(current) => current == item_id,
                        NavigationTarget::Group {
                            item_id: current, ..
                        }
                        | NavigationTarget::ReviewAttempt {
                            item_id: current, ..
                        } => current == item_id,
                        NavigationTarget::Current | NavigationTarget::Closed => false,
                    }
            });
            if targets_item {
                previous = latest.clone();
            }
        }
        if let (Some(fences), Some(previous)) = (fences.as_mut(), previous.as_ref()) {
            if let Some(fence) = fences.get_mut(item_id) {
                fence.previous_id = Some(previous.id);
            }
        }
        SuspendedNavigation {
            fence_id,
            item_id: item_id.to_string(),
            previous,
        }
    }

    fn restore_suspended(&self, suspended: &SuspendedNavigation) -> bool {
        let restored_fence = if let Ok(mut fences) = self.archive_fences.lock() {
            if fences
                .get(&suspended.item_id)
                .is_some_and(|fence| fence.id == suspended.fence_id)
            {
                fences.remove(&suspended.item_id);
                true
            } else {
                false
            }
        } else {
            false
        };
        let Some(previous) = suspended.previous.as_ref() else {
            return false;
        };
        let Ok(mut latest) = self.latest.lock() else {
            return false;
        };
        let current_fence = latest.as_ref().is_some_and(|request| {
            request.id == suspended.fence_id
                && matches!(&request.target, NavigationTarget::SuspendedItem(_))
        });
        if current_fence {
            *latest = Some(previous.clone());
        }
        restored_fence
            && (current_fence
                || latest
                    .as_ref()
                    .is_some_and(|request| request.id == previous.id))
    }

    fn is_current_item(&self, item_id: &str, request_id: u64) -> bool {
        let Ok(fences) = self.archive_fences.lock() else {
            return false;
        };
        let fence = fences.get(item_id).copied();
        let Ok(latest) = self.latest.lock() else {
            return false;
        };
        if fence.is_some_and(|fence| {
            !fence.committed && fence.previous_id == Some(request_id)
        }) && latest
            .as_ref()
            .is_some_and(|request| request.id == request_id)
        {
            return true;
        }
        latest.as_ref().is_some_and(|request| {
            request.id == request_id
                && !fence.is_some_and(|fence| {
                    request_id <= fence.id
                        && (fence.committed
                            || !matches!(&request.target, NavigationTarget::Current))
                })
        })
    }

    fn archive_fence_is_current(&self, suspended: &SuspendedNavigation) -> bool {
        let Ok(fences) = self.archive_fences.lock() else {
            return false;
        };
        if !fences
            .get(&suspended.item_id)
            .is_some_and(|fence| fence.id == suspended.fence_id)
        {
            return false;
        }
        let Ok(latest) = self.latest.lock() else {
            return false;
        };
        !latest.as_ref().is_some_and(|request| {
            request.id > suspended.fence_id
                && match &request.target {
                    NavigationTarget::Item(item_id)
                    | NavigationTarget::SuspendedItem(item_id) => item_id == &suspended.item_id,
                    NavigationTarget::Group { item_id, .. }
                    | NavigationTarget::ReviewAttempt { item_id, .. } => {
                        item_id == &suspended.item_id
                    }
                    NavigationTarget::Current | NavigationTarget::Closed => false,
                }
        })
    }

    fn commit_archive_fence(&self, suspended: &SuspendedNavigation) -> bool {
        let Ok(mut fences) = self.archive_fences.lock() else {
            return false;
        };
        let Some(fence) = fences.get_mut(&suspended.item_id) else {
            return false;
        };
        if fence.id != suspended.fence_id {
            return false;
        }
        fence.committed = true;
        if let Some(previous) = suspended.previous.as_ref() {
            let Ok(mut latest) = self.latest.lock() else {
                return false;
            };
            if latest
                .as_ref()
                .is_some_and(|request| request.id == previous.id)
            {
                *latest = Some(NavigationRequest {
                    id: suspended.fence_id,
                    target: NavigationTarget::SuspendedItem(suspended.item_id.clone()),
                    historical_pending: false,
                    historical_rollback: None,
                });
            }
        }
        true
    }

    fn is_committed_archive_fence(&self, item_id: &str, request_id: u64) -> bool {
        let Ok(fences) = self.archive_fences.lock() else {
            return false;
        };
        let blocked = fences
            .get(item_id)
            .is_some_and(|fence| fence.committed && request_id <= fence.id);
        blocked && self.is_current(request_id)
    }

    /// Commit a resolved item while atomically checking both navigation
    /// order and the latest synchronous archive fence. Subject opens begin
    /// before their durable item id is known, so a successful commit binds
    /// them to that item for later same-item lifecycle actions.
    fn with_current_item<T>(
        &self,
        item_id: &str,
        request_id: u64,
        commit: impl FnOnce() -> T,
    ) -> Option<T> {
        let fences = self.archive_fences.lock().ok()?;
        let fence = fences.get(item_id).copied();
        let mut latest = self.latest.lock().ok()?;
        if fence.is_some_and(|fence| {
            !fence.committed && fence.previous_id == Some(request_id)
        }) && latest
            .as_ref()
            .is_some_and(|request| request.id == request_id)
        {
            return Some(commit());
        }
        let request = latest
            .as_mut()
            .filter(|request| request.id == request_id)?;
        if fence.is_some_and(|fence| {
            request_id <= fence.id
                && (fence.committed || !matches!(&request.target, NavigationTarget::Current))
        }) {
            return None;
        }
        let result = commit();
        if matches!(
            &request.target,
            NavigationTarget::Current | NavigationTarget::Item(_)
        ) {
            request.target = NavigationTarget::Item(item_id.to_string());
        }
        drop(fences);
        Some(result)
    }

    fn with_current<T>(&self, request_id: u64, commit: impl FnOnce() -> T) -> Option<T> {
        let latest = self.latest.lock().ok()?;
        latest
            .as_ref()
            .is_some_and(|request| request.id == request_id)
            .then(commit)
    }

    /// Execute a historical commit while holding the same short lock used to
    /// register every newer navigation. Whichever wins the lock defines the
    /// final view.
    fn with_current_group<T>(
        &self,
        group_id: &str,
        request_id: u64,
        commit: impl FnOnce() -> T,
    ) -> Option<T> {
        let latest = self.latest.lock().ok()?;
        latest
            .as_ref()
            .is_some_and(|request| {
                request.id == request_id
                    && matches!(
                        &request.target,
                        NavigationTarget::Group { group_id: current, .. } if current == group_id
                    )
            })
            .then(commit)
    }

    /// Install a historical slot and make it the stable rollback point while
    /// holding the navigation lock. A newer history click therefore observes
    /// either the prior stable view or this fully installed one.
    fn commit_current_group<T>(
        &self,
        group_id: &str,
        request_id: u64,
        commit: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let mut latest = self.latest.lock().ok()?;
        let current = latest.as_ref().is_some_and(|request| {
            request.id == request_id
                && matches!(
                    &request.target,
                    NavigationTarget::Group { group_id: current, .. } if current == group_id
                )
        });
        if !current {
            return None;
        }
        let committed = commit()?;
        if let Some(request) = latest.as_mut() {
            request.historical_pending = false;
            request.historical_rollback = None;
        }
        Some(committed)
    }

    /// Restore the last stable navigation after the current historical load
    /// fails. Returning false means a newer navigation already owns the view.
    fn rollback_current_group(&self, group_id: &str, request_id: u64) -> bool {
        let Ok(mut latest) = self.latest.lock() else {
            return false;
        };
        let Some(request) = latest.as_ref() else {
            return false;
        };
        if request.id != request_id
            || !request.historical_pending
            || !matches!(
                &request.target,
                NavigationTarget::Group { group_id: current, .. } if current == group_id
            )
        {
            return false;
        }
        *latest = request
            .historical_rollback
            .as_ref()
            .map(|previous| (**previous).clone());
        true
    }

    fn rollback_current_attempt(&self, attempt_id: &str, request_id: u64) -> bool {
        let Ok(mut latest) = self.latest.lock() else {
            return false;
        };
        let Some(request) = latest.as_ref() else {
            return false;
        };
        if request.id != request_id
            || !request.historical_pending
            || !matches!(
                &request.target,
                NavigationTarget::ReviewAttempt { attempt_id: current, .. }
                    if current == attempt_id
            )
        {
            return false;
        }
        *latest = request
            .historical_rollback
            .as_ref()
            .map(|previous| (**previous).clone());
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopeRequest {
    id: u64,
    item_id: String,
}

/// Scope file reads run concurrently. Register the selection synchronously
/// so a slower earlier Git query cannot overwrite a later scope or publish
/// finding anchors projected for the wrong pair of sides.
#[derive(Debug, Default)]
struct ScopeRequests {
    state: std::sync::Mutex<ScopeRequestState>,
}

#[derive(Debug, Default)]
struct ScopeRequestState {
    revision: u64,
    latest: Option<ScopeRequest>,
    pending: bool,
}

impl ScopeRequests {
    fn begin(&self, item_id: &str) -> u64 {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        state.revision = state.revision.saturating_add(1);
        let id = state.revision;
        state.latest = Some(ScopeRequest {
            id,
            item_id: item_id.to_string(),
        });
        state.pending = true;
        id
    }

    fn stable_revision(&self) -> Option<u64> {
        self.state
            .lock()
            .ok()
            .and_then(|state| (!state.pending).then_some(state.revision))
    }

    fn is_current(&self, item_id: &str, request_id: u64) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .latest
                .as_ref()
                .is_some_and(|request| request.id == request_id && request.item_id == item_id)
        })
    }

    fn with_current<T>(
        &self,
        item_id: &str,
        request_id: u64,
        commit: impl FnOnce() -> T,
    ) -> Option<T> {
        let state = self.state.lock().ok()?;
        state
            .latest
            .as_ref()
            .is_some_and(|request| request.id == request_id && request.item_id == item_id)
            .then(commit)
    }

    fn with_revision<T>(&self, revision: u64, commit: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock().ok()?;
        (state.revision == revision && !state.pending).then(commit)
    }

    fn settle(&self, item_id: &str, request_id: u64) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .latest
                .as_ref()
                .is_some_and(|request| request.id == request_id && request.item_id == item_id)
            {
                state.pending = false;
            }
        }
    }

    fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.revision = state.revision.saturating_add(1);
            state.latest = None;
            state.pending = false;
        }
    }

    fn clear_item(&self, item_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .latest
                .as_ref()
                .is_some_and(|request| request.item_id == item_id)
            {
                state.revision = state.revision.saturating_add(1);
                state.latest = None;
                state.pending = false;
            }
        }
    }
}

/// Monotonic generations for live pull-request status reads. Generations are
/// per item because unrelated reviews may refresh concurrently.
#[derive(Debug, Default)]
struct PullRequestStatusRequests {
    latest: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl PullRequestStatusRequests {
    fn begin(&self, item_id: &str) -> u64 {
        let Ok(mut latest) = self.latest.lock() else {
            return 0;
        };
        let generation = latest
            .get(item_id)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        latest.insert(item_id.to_string(), generation);
        generation
    }

    fn is_current(&self, item_id: &str, generation: u64) -> bool {
        self.latest
            .lock()
            .ok()
            .and_then(|latest| latest.get(item_id).copied())
            == Some(generation)
    }
}

/// Merge attempts are registered synchronously by the UI handle. Keeping the
/// in-flight item here both assigns an ordering identity and suppresses a
/// second REST mutation while the first is unresolved.
#[derive(Debug, Default)]
struct MergeAttempts {
    next_id: std::sync::atomic::AtomicU64,
    in_flight: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl MergeAttempts {
    fn begin(&self, item_id: &str) -> Option<u64> {
        let mut in_flight = self.in_flight.lock().ok()?;
        if in_flight.contains_key(item_id) {
            return None;
        }
        let attempt_id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        in_flight.insert(item_id.to_string(), attempt_id);
        Some(attempt_id)
    }

    fn finish(&self, item_id: &str, attempt_id: u64) {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            if in_flight.get(item_id).copied() == Some(attempt_id) {
                in_flight.remove(item_id);
            }
        }
    }
}

impl Backend {
    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }

    /// Open a pull request or an issue by number, making its item if it has
    /// none yet. The launch hooks and the review header's "Closes #N" chip
    /// come through here; a sidebar row has an id and uses `open_item`.
    pub fn open_subject(&self, repo_full_name: &str, kind: ItemKind, number: u64) -> u64 {
        let navigation_id = self.navigation_requests.begin_current();
        self.scope_requests.clear();
        let _ = self.commands.send(Command::OpenSubject {
            repo_full_name: repo_full_name.to_string(),
            kind,
            number,
            navigation_id,
        });
        navigation_id
    }

    pub fn open_notification(&self, thread: &Thread) -> Option<u64> {
        let opens_inside = matches!(
            thread.subject.kind,
            SubjectKind::PullRequest | SubjectKind::Issue
        ) && thread.number_hint().is_some();
        let navigation_id = opens_inside.then(|| self.navigation_requests.begin_current());
        if navigation_id.is_some() {
            self.scope_requests.clear();
        }
        let _ = self.commands.send(Command::OpenNotification {
            thread: thread.clone(),
            navigation_id,
        });
        navigation_id
    }

    pub fn done_notification(&self, thread_id: &str) {
        let _ = self.commands.send(Command::DoneNotification {
            thread_id: thread_id.to_string(),
        });
    }

    pub fn mark_notification_unread(&self, thread_id: &str) {
        let _ = self.commands.send(Command::MarkNotificationUnread {
            thread_id: thread_id.to_string(),
        });
    }

    pub fn unsubscribe_notification(&self, thread_id: &str) {
        let _ = self.commands.send(Command::UnsubscribeNotification {
            thread_id: thread_id.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn merge_pull_request(
        &self,
        item_id: &str,
        repo_full_name: &str,
        number: u64,
        expected_head_sha: &str,
        method: MergeMethod,
        bypass_requirements: bool,
    ) {
        let Some(attempt_id) = self.merge_attempts.begin(item_id) else {
            return;
        };
        if self.commands.send(Command::MergePullRequest {
            attempt_id,
            item_id: item_id.to_string(),
            repo_full_name: repo_full_name.to_string(),
            number,
            expected_head_sha: expected_head_sha.to_string(),
            method,
            bypass_requirements,
        }).is_err() {
            self.merge_attempts.finish(item_id, attempt_id);
        }
    }

    pub fn open_item(&self, id: &str) -> u64 {
        let navigation_id = self.navigation_requests.begin_item(id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::OpenItem {
            id: id.to_string(),
            navigation_id,
        });
        navigation_id
    }

    pub fn view_agent_owner(&self, id: &str) -> u64 {
        let navigation_id = self.navigation_requests.begin_item(id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::ViewAgentOwner {
            id: id.to_string(),
            navigation_id,
        });
        navigation_id
    }

    pub fn view_review_attempt_owner(&self, item_id: &str, attempt_id: &str) -> u64 {
        let request_id = self.navigation_requests.begin_attempt(item_id, attempt_id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::ViewReviewAttemptOwner {
            item_id: item_id.to_string(),
            attempt_id: attempt_id.to_string(),
            request_id,
        });
        request_id
    }

    pub fn open_file(&self, path: &str) {
        let navigation_id = self.navigation_requests.current_id().unwrap_or(0);
        let _ = self.commands.send(Command::OpenFile {
            path: path.to_string(),
            navigation_id,
        });
    }

    pub fn set_scope(&self, item_id: &str, scope: perch_git::Scope) {
        let navigation_id = self.navigation_requests.current_id().unwrap_or(0);
        let request_id = self.scope_requests.begin(item_id);
        let _ = self.commands.send(Command::SetScope {
            item_id: item_id.to_string(),
            scope,
            request_id,
            navigation_id,
        });
    }

    pub fn refresh_scope(&self, item_id: &str) {
        let navigation_id = self.navigation_requests.current_id().unwrap_or(0);
        if let Some(scope_revision) = self.scope_requests.stable_revision() {
            let _ = self.commands.send(Command::RefreshScope {
                item_id: item_id.to_string(),
                navigation_id,
                scope_revision,
            });
        }
    }

    pub fn set_viewed(&self, item_id: &str, path: &str, blob: &str, viewed: bool) {
        let _ = self.commands.send(Command::SetViewed {
            item_id: item_id.to_string(),
            path: path.to_string(),
            blob: blob.to_string(),
            viewed,
        });
    }

    pub fn reveal(&self, item_id: &str, path: &str, blob: &str, first: u32, last: u32) {
        let _ = self.commands.send(Command::Reveal {
            item_id: item_id.to_string(),
            path: path.to_string(),
            blob: blob.to_string(),
            first,
            last,
        });
    }

    pub fn reset_reveals(&self, item_id: &str, path: &str, blob: &str, first: u32, last: u32) {
        let _ = self.commands.send(Command::ResetReveals {
            item_id: item_id.to_string(),
            path: path.to_string(),
            blob: blob.to_string(),
            first,
            last,
        });
    }

    pub fn references(&self, token: u64, path: &str, line: u32, character: u32) {
        let _ = self.commands.send(Command::References {
            token,
            path: path.to_string(),
            line,
            character,
        });
    }

    pub fn close_review(&self) -> u64 {
        let navigation_id = self.navigation_requests.begin_closed();
        self.scope_requests.clear();
        let _ = self.commands.send(Command::CloseReview { navigation_id });
        navigation_id
    }

    pub fn archive_item(&self, id: &str) -> u64 {
        let suspended_navigation = self.navigation_requests.suspend_item(id);
        self.scope_requests.clear_item(id);
        let fence_id = suspended_navigation.fence_id;
        if self
            .commands
            .send(Command::ArchiveItem {
                id: id.to_string(),
                suspended_navigation: suspended_navigation.clone(),
            })
            .is_err()
        {
            self.navigation_requests
                .restore_suspended(&suspended_navigation);
        }
        fence_id
    }

    pub fn restore_item(&self, id: &str) -> u64 {
        let navigation_id = self.navigation_requests.begin_item(id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::RestoreItem {
            id: id.to_string(),
            navigation_id,
        });
        navigation_id
    }

    pub fn delete_item(&self, id: &str) {
        self.scope_requests.clear_item(id);
        let _ = self
            .commands
            .send(Command::DeleteItem { id: id.to_string() });
    }

    pub fn mark_saved(&self, id: &str, saved: bool) {
        let _ = self.commands.send(Command::SetSaved {
            id: id.to_string(),
            saved,
        });
    }

    pub fn pin_item(&self, id: &str, pinned: bool) {
        let _ = self.commands.send(Command::PinItem {
            id: id.to_string(),
            pinned,
        });
    }

    pub fn rename_item(&self, id: &str, name: Option<String>) {
        let _ = self.commands.send(Command::RenameItem {
            id: id.to_string(),
            name,
        });
    }

    pub fn start_work(&self, issue_item_id: &str) -> u64 {
        let navigation_id = self.navigation_requests.begin_item(issue_item_id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::StartWork {
            issue_item_id: issue_item_id.to_string(),
            navigation_id,
        });
        navigation_id
    }

    pub fn fork_item(&self, id: &str) -> u64 {
        let navigation_id = self.navigation_requests.begin_item(id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::ForkItem {
            id: id.to_string(),
            navigation_id,
        });
        navigation_id
    }

    pub fn set_meta(&self, key: &str, value: &str) {
        let _ = self.commands.send(Command::SetMeta {
            key: key.to_string(),
            value: value.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ask(
        &self,
        item_id: &str,
        thread_id: Option<String>,
        anchor: Option<Anchor>,
        question: &str,
        backend: perch_agent::Backend,
        model: &str,
        effort: perch_agent::Effort,
    ) {
        let _ = self.commands.send(Command::Ask {
            item_id: item_id.to_string(),
            thread_id,
            anchor,
            question: question.to_string(),
            backend,
            model: model.to_string(),
            effort,
        });
    }

    pub fn cancel_run(&self, run_id: &str) {
        let _ = self.commands.send(Command::CancelRun {
            run_id: run_id.to_string(),
        });
    }

    pub fn retry_run(&self, run_id: &str) {
        let _ = self.commands.send(Command::RetryRun {
            run_id: run_id.to_string(),
        });
    }

    pub fn load_review_dashboard(&self, item_id: &str) {
        let _ = self.commands.send(Command::LoadReviewDashboard {
            item_id: item_id.to_string(),
        });
    }

    pub fn start_review(&self, request: crate::review::model::ReviewLaunchRequest) {
        let _ = self.commands.send(Command::StartReview { request });
    }

    pub fn save_review_preference(&self, choice: crate::review::model::ReviewPreferenceChoice) {
        let _ = self.commands.send(Command::SaveReviewPreference { choice });
    }

    pub fn stop_review_group(&self, group_id: &str) {
        let _ = self.commands.send(Command::StopReviewGroup {
            group_id: group_id.to_string(),
        });
    }

    pub fn retry_review_attempt(&self, attempt_id: &str) {
        let _ = self.commands.send(Command::RetryReviewAttempt {
            attempt_id: attempt_id.to_string(),
        });
    }

    pub fn open_review_snapshot(&self, item_id: &str, group_id: &str) -> u64 {
        let request_id = self.navigation_requests.begin_group(item_id, group_id);
        self.scope_requests.clear();
        let _ = self.commands.send(Command::OpenReviewSnapshot {
            group_id: group_id.to_string(),
            request_id,
        });
        request_id
    }

    pub fn dismiss_review_finding(&self, finding_id: &str, reason: &str) {
        let _ = self.commands.send(Command::DismissReviewFinding {
            finding_id: finding_id.to_string(),
            reason: reason.to_string(),
        });
    }

    pub fn reopen_review_finding(&self, finding_id: &str) {
        let _ = self.commands.send(Command::ReopenReviewFinding {
            finding_id: finding_id.to_string(),
        });
    }

    pub fn link_review_duplicate(&self, finding_id: &str, canonical_finding_id: &str) {
        let _ = self.commands.send(Command::LinkReviewDuplicate {
            finding_id: finding_id.to_string(),
            canonical_finding_id: canonical_finding_id.to_string(),
        });
    }

    pub fn unlink_review_duplicate(&self, finding_id: &str) {
        let _ = self.commands.send(Command::UnlinkReviewDuplicate {
            finding_id: finding_id.to_string(),
        });
    }

    pub fn load_agent_runs(&self, history_limit: usize) {
        let _ = self.commands.send(Command::LoadAgentRuns { history_limit });
    }

    pub fn load_agent_run_detail(&self, run_id: &str) {
        let _ = self.commands.send(Command::LoadAgentRunDetail {
            run_id: run_id.to_string(),
        });
    }

    pub fn load_agent_run_events(&self, run_id: &str, before_sequence: u64) {
        let _ = self.commands.send(Command::LoadAgentRunEvents {
            run_id: run_id.to_string(),
            before_sequence,
        });
    }

    pub fn delete_comment_thread(&self, id: &str) {
        let _ = self
            .commands
            .send(Command::DeleteCommentThread { id: id.to_string() });
    }

    pub fn hover(&self, token: u64, path: &str, line: u32, character: u32) {
        let _ = self.commands.send(Command::Hover {
            token,
            path: path.to_string(),
            line,
            character,
        });
    }

    pub fn definition(&self, token: u64, path: &str, line: u32, character: u32) {
        let _ = self.commands.send(Command::Definition {
            token,
            path: path.to_string(),
            line,
            character,
        });
    }
}

impl Backend {
    /// A handle with nobody behind it: every command is dropped. For
    /// headless tests, which must not open the reader's database or talk
    /// to GitHub.
    #[cfg(test)]
    pub fn detached() -> Self {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        Backend {
            commands,
            navigation_requests: Arc::default(),
            scope_requests: Arc::default(),
            merge_attempts: Arc::default(),
        }
    }
}

/// Start the worker thread. Returns the command handle and the event receiver.
pub fn spawn() -> (Backend, std::sync::mpsc::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel();
    let navigation_requests = Arc::<NavigationRequests>::default();
    let worker_navigation_requests = navigation_requests.clone();
    let scope_requests = Arc::<ScopeRequests>::default();
    let worker_scope_requests = scope_requests.clone();
    let merge_attempts = Arc::<MergeAttempts>::default();
    let worker_merge_attempts = merge_attempts.clone();

    std::thread::Builder::new()
        .name("perch-backend".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = evt_tx.send(Event::Status(Status::Error {
                        message: format!("could not start the network runtime: {e}"),
                    }));
                    return;
                }
            };
            runtime.block_on(run(
                cmd_rx,
                evt_tx,
                worker_navigation_requests,
                worker_scope_requests,
                worker_merge_attempts,
            ));
        })
        .expect("spawn backend thread");

    (
        Backend {
            commands: cmd_tx,
            navigation_requests,
            scope_requests,
            merge_attempts,
        },
        evt_rx,
    )
}

async fn run(
    mut commands: UnboundedReceiver<Command>,
    events: EventSender<Event>,
    navigation_requests: Arc<NavigationRequests>,
    scope_requests: Arc<ScopeRequests>,
    merge_attempts: Arc<MergeAttempts>,
) {
    tracing::info!("backend started");
    let mut config = Config::load();
    // A `[[repo]]` with roots and no `github` is whatever its first root
    // is, which takes a look at the directory before anything reads the
    // declared list. No token is needed: the scan reads the root's own
    // config and fetches nothing.
    crate::repos::resolve(&mut config, &perch_git::cmd::Git::new(None)).await;
    tracing::info!(poll = config.poll_interval_secs, repos = ?config.declared(), "config loaded");

    // Which agents are installed, asked alongside the GitHub setup rather
    // than before it: Codex's catalog can take a few seconds on a cold
    // cache, and the sidebar should not wait for a picker.
    let agents = {
        let mut agent = config.agent.clone();
        let events = events.clone();
        tokio::spawn(async move {
            for problem in crate::ask::discover(&mut agent).await {
                tracing::warn!(%problem, "agent discovery");
            }
            tracing::info!(claude = %agent.binary, codex = ?agent.codex.models, "agents found");
            if agent.models.is_empty() && agent.codex.models.is_empty() {
                let _ = events.send(Event::Notice(
                    "No coding agent found. Install Claude Code or Codex, or set agent.binary in config.toml.".into(),
                ));
            }
            let _ = events.send(Event::AgentConfig(agent.clone()));
            agent
        })
    };

    // Local state and agent history do not depend on GitHub credentials.
    // Reconcile them before token setup so a launch without a token cannot
    // leave crashed work looking active indefinitely.
    let (cache, reset) = open_cache_for_app(&events);
    tracing::info!(cache = cache.is_some(), reset, "cache opened");
    for warning in &config.warnings {
        let _ = events.send(Event::Notice(warning.clone()));
    }
    let _ = events.send(Event::Repos(config.declared_repos()));
    let mut initial_notifications = Vec::new();
    if let Some(cache) = cache.as_ref() {
        if let Ok(guard) = cache.lock() {
            match guard.interrupt_active_runs(
                perch_github::TerminalReason::AppRestart,
                "Perch quit before the agent finished",
            ) {
                Ok(n) if n > 0 => tracing::info!(n, "marked interrupted runs"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "could not reconcile runs"),
            }
            send_items(&guard, &events);
            match guard.load_threads() {
                Ok(threads) => {
                    initial_notifications = threads.clone();
                    let _ = events.send(Event::Notifications(threads));
                }
                Err(e) => tracing::warn!(error = %e, "could not load cached notifications"),
            }
            for key in UI_META_KEYS {
                if let Ok(Some(value)) = guard.meta(key) {
                    let _ = events.send(Event::Meta {
                        key: key.to_string(),
                        value,
                    });
                }
            }
        }
    }
    send_agent_runs_from_cache(&cache, &events, 50);

    // Checkout retirement is local bookkeeping. Finish an archive that was
    // interrupted by a crash before entering the command loop.
    let data_root =
        perch_github::config::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let local_preparer = Preparer::new(Store::new(data_root.clone()), None);
    retire_archived_checkouts_from_cache(&cache, &local_preparer, &events).await;

    // GitHub is optional. The cache, local checkouts and agent history stay
    // useful without a token, so only polling and remote actions are gated
    // on a client below.
    let token = resolve_token(&config);
    let source = token
        .as_ref()
        .map(|token| token.source.describe().to_string())
        .unwrap_or_default();
    let github = match token.as_ref() {
        Some(token) => match GitHub::new(&token.value) {
            Ok(github) => {
                tracing::info!("github client ready");
                Some(Arc::new(github))
            }
            Err(e) => {
                let _ = events.send(Event::Status(Status::Error {
                    message: format!("could not build the GitHub client: {e}"),
                }));
                None
            }
        },
        None => {
            tracing::warn!("no token resolved");
            let _ = events.send(Event::Status(Status::NoToken {
                detail: "Set GITHUB_TOKEN, or run `gh auth login` in a terminal.".into(),
            }));
            None
        }
    };

    let avatars = Arc::new(Semaphore::new(AVATAR_CONCURRENCY));
    // The review side of the app. Long operations run as their own tasks so a
    // clone never stalls the notification poll, and the active checkout is
    // shared so a file open knows where to read from.
    let preparer = Arc::new(Preparer::new(
        Store::new(data_root.clone()),
        token.as_ref().map(|token| token.value.clone()),
    ));
    let checkout: Arc<std::sync::Mutex<Option<ReviewContext>>> =
        Arc::new(std::sync::Mutex::new(None));
    let language_server: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // The runs spawn what discovery found, so the shared config carries
    // its answer. A discovery that panicked leaves the file's own words.
    if let Ok(agent) = agents.await {
        config.agent = agent;
    }
    let (claude_review, codex_review) = tokio::join!(
        perch_agent::detect_review_capability(
            perch_agent::Backend::ClaudeCode,
            Path::new(&config.agent.binary),
        ),
        perch_agent::detect_review_capability(
            perch_agent::Backend::Codex,
            Path::new(&config.agent.codex.binary),
        ),
    );
    let review_capabilities = Arc::new(vec![claude_review, codex_review]);
    let review_snapshots = Arc::new(ReviewSnapshotStore::new(data_root.clone()));
    let knowledge = match perch_knowledge::Knowledge::open(data_root.join("knowledge")) {
        Ok(knowledge) => Some(Arc::new(knowledge)),
        Err(error) => {
            tracing::warn!(%error, "review knowledge export is unavailable");
            None
        }
    };
    let config = Arc::new(config);
    let run_manager = runs::RunManager::new(
        config.agent.max_concurrent_runs,
        cache.clone(),
        events.clone(),
    );
    let ctx = Ctx {
        github: github.clone(),
        preparer: preparer.clone(),
        cache: cache.clone(),
        config: config.clone(),
        slot: checkout.clone(),
        language_server: language_server.clone(),
        events: events.clone(),
        runs: run_manager,
        review_capabilities,
        review_snapshots,
        navigation_requests,
        scope_requests,
        status_requests: Arc::default(),
        merge_attempts,
        review_navigation: Arc::new(tokio::sync::Mutex::new(())),
        review_admissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        review_reconcile: Arc::new(tokio::sync::Mutex::new(())),
        knowledge,
        item_lifecycle: Arc::new(tokio::sync::Mutex::new(())),
        avatars,
        seen_notifications: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        refreshing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        notifications: Arc::new(std::sync::Mutex::new(initial_notifications)),
    };
    send_agent_runs(&ctx, 50);
    reviews::recover(&ctx).await;

    // Who the reader is on GitHub, for their own byline: once per launch,
    // kept in meta so the next launch has it before the network does.
    if ctx.github.is_some() {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let Some(github) = ctx.github.as_ref() else {
                return;
            };
            match github.viewer().await {
                Ok(user) if !user.login.is_empty() => {
                    for (key, value) in [
                        ("viewer.login", &user.login),
                        ("viewer.avatar_url", &user.avatar_url),
                    ] {
                        ctx.with_cache(|c| c.set_meta(key, value));
                        let _ = ctx.events.send(Event::Meta {
                            key: key.into(),
                            value: value.clone(),
                        });
                    }
                    ctx.want_avatars(std::iter::once(user.avatar_url));
                }
                Ok(_) => {}
                Err(e) => tracing::info!(error = %e, "could not ask who the token belongs to"),
            }
        });
    }

    // Bring the table and the disk into agreement before any command can
    // make a directory. Inline, not spawned: a checkout made by a launch
    // hook while this ran would have no row yet, and would be taken for a
    // stray and removed from under it. It is a directory listing and at
    // most a prune, so the first poll is not kept waiting.
    reconcile_checkouts(&ctx, reset).await;
    {
        // Then put away whatever nobody has opened in a fortnight. Archived,
        // not deleted: the branch and the row stay, so restoring is a
        // checkout. Only owned checkouts are touched, so this can run
        // beside whatever the reader does next.
        let ctx = ctx.clone();
        tokio::spawn(async move { sweep_items(&ctx).await });
    }

    // Scan roots before entering the command loop so local work is available
    // immediately. Pull-request lookups for those branches run separately.
    if scan_roots(&ctx).await {
        ctx.send_items();
    }
    if ctx.github.is_some() {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if attach_pull_requests(&ctx).await {
                ctx.send_items();
            }
        });
    }

    // A refresh happens immediately, then on whatever cadence the server
    // asks for. Other commands do not restart that clock, and a manual
    // refresh never polls earlier than GitHub's X-Poll-Interval permits.
    let mut next_auto_poll = tokio::time::Instant::now();
    let mut next_allowed_poll = next_auto_poll;
    let mut next_notification_reconcile = next_auto_poll;
    let mut pending_notification_reconcile = false;

    loop {
        let (command, automatic) = tokio::select! {
            _ = tokio::time::sleep_until(next_auto_poll), if ctx.github.is_some() => {
                (Command::Refresh, true)
            },
            cmd = commands.recv() => match cmd {
                Some(cmd) => (cmd, false),
                // The UI is gone.
                None => {
                    ctx.runs.shutdown().await;
                    // Deferred archive waiters are detached tasks. Complete
                    // their local work synchronously before this runtime can
                    // disappear underneath them.
                    retire_archived_checkouts(&ctx).await;
                    return;
                },
            },
        };

        if !matches!(&command, Command::Refresh | Command::Hover { .. }) {
            tracing::debug!(command = ?command, "command");
        }
        match command {
            Command::Refresh => {
                let now = tokio::time::Instant::now();
                if !automatic {
                    pending_notification_reconcile = true;
                }
                if !automatic && now < next_allowed_poll {
                    // Remember the click. GitHub forbids polling before its
                    // advertised minimum, so wake at that boundary instead
                    // of silently dropping the manual refresh.
                    queue_manual_refresh(&mut next_auto_poll, next_allowed_poll);
                    continue;
                }
                let mode = if pending_notification_reconcile
                    || now >= next_notification_reconcile
                {
                    NotificationPollMode::Reconcile
                } else {
                    NotificationPollMode::Conditional
                };
                let poll_after = refresh(&ctx, &source, mode).await;
                let now = tokio::time::Instant::now();
                next_auto_poll = now + poll_after.automatic;
                next_allowed_poll = now + poll_after.manual;
                if mode == NotificationPollMode::Reconcile {
                    if poll_after.completed {
                        pending_notification_reconcile = false;
                        next_notification_reconcile = now + NOTIFICATION_RECONCILE_INTERVAL;
                    } else {
                        pending_notification_reconcile = true;
                    }
                }
                let open = ctx
                    .slot
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|c| c.item_id.clone()));
                if let Some(item_id) = open {
                    let ctx = ctx.clone();
                    if let (Some(navigation_id), Some(scope_revision)) = (
                        ctx.navigation_requests.current_id(),
                        ctx.scope_requests.stable_revision(),
                    ) {
                        tokio::spawn(async move {
                            refresh_scope(&ctx, &item_id, navigation_id, scope_revision).await
                        });
                    }
                }
                let ctx = ctx.clone();
                tokio::spawn(async move { refresh_items(&ctx).await });
            }
            Command::OpenSubject {
                repo_full_name,
                kind,
                number,
                navigation_id,
            } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_subject_navigation(&ctx, repo_full_name, kind, number, navigation_id)
                        .await
                    {
                        start_language_server(
                            ctx.slot,
                            ctx.language_server,
                            ctx.events,
                            ctx.navigation_requests,
                            navigation_id,
                        )
                        .await;
                    }
                });
            }
            Command::OpenNotification {
                thread,
                navigation_id,
            } => {
                ctx.clear_notice();
                if ctx.prepare_notification_open(&thread) {
                    if let Some(github) = ctx.github.as_ref() {
                        match github.mark_thread_read(&thread.id).await {
                            Ok(()) => {
                                let id = thread.id.clone();
                                ctx.edit_notifications(|threads| {
                                    if let Some(thread) = threads.iter_mut().find(|t| t.id == id) {
                                        thread.unread = false;
                                        thread.local_unread = false;
                                        thread.last_read_at = Some(Timestamp::now());
                                    }
                                });
                            }
                            Err(e) => {
                                defer_polls_for_error(
                                    &e,
                                    &mut next_auto_poll,
                                    &mut next_allowed_poll,
                                );
                                tracing::warn!(thread = %thread.id, error = %e, "could not mark notification read");
                                ctx.notice(format!("Could not mark the notification read: {e}"));
                            }
                        }
                    } else {
                        ctx.notice("A GitHub token is required to mark notifications read");
                    }
                }
                let kind = match &thread.subject.kind {
                    SubjectKind::PullRequest => Some(ItemKind::PullRequest),
                    SubjectKind::Issue => Some(ItemKind::Issue),
                    _ => None,
                };
                if let (Some(navigation_id), Some(kind), Some(number)) =
                    (navigation_id, kind, thread.number_hint())
                {
                    let ctx = ctx.clone();
                    let repo = thread.repository.full_name;
                    tokio::spawn(async move {
                        if open_subject_navigation(&ctx, repo, kind, number, navigation_id).await {
                            start_language_server(
                                ctx.slot,
                                ctx.language_server,
                                ctx.events,
                                ctx.navigation_requests,
                                navigation_id,
                            )
                            .await;
                        }
                    });
                }
            }
            Command::MarkNotificationUnread { thread_id } => {
                ctx.mark_notification_unread(&thread_id);
            }
            Command::DoneNotification { thread_id } => {
                ctx.clear_notice();
                if let Some(github) = ctx.github.as_ref() {
                    match github.mark_thread_done(&thread_id).await {
                        Ok(()) => ctx.dismiss_notification(&thread_id),
                        Err(e) => {
                            defer_polls_for_error(&e, &mut next_auto_poll, &mut next_allowed_poll);
                            tracing::warn!(thread = %thread_id, error = %e, "could not mark notification done");
                            ctx.notice(format!("Could not mark the notification done: {e}"));
                        }
                    }
                } else {
                    ctx.notice("A GitHub token is required to change notifications");
                }
            }
            Command::UnsubscribeNotification { thread_id } => {
                ctx.clear_notice();
                if let Some(github) = ctx.github.as_ref() {
                    let result = match github.ignore_thread(&thread_id).await {
                        Ok(()) => github.mark_thread_done(&thread_id).await,
                        Err(e) => Err(e),
                    };
                    match result {
                        Ok(()) => ctx.dismiss_notification(&thread_id),
                        Err(e) => {
                            defer_polls_for_error(&e, &mut next_auto_poll, &mut next_allowed_poll);
                            tracing::warn!(thread = %thread_id, error = %e, "could not unsubscribe from notification");
                            ctx.notice(format!("Could not unsubscribe from the notification: {e}"));
                        }
                    }
                } else {
                    ctx.notice("A GitHub token is required to change notifications");
                }
            }
            Command::MergePullRequest {
                attempt_id,
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                method,
                bypass_requirements,
            } => {
                ctx.clear_notice();
                let _ = ctx.events.send(Event::PullRequestMergeStarted {
                    item_id: item_id.clone(),
                    repo_full_name: repo_full_name.clone(),
                    number,
                    expected_head_sha: expected_head_sha.clone(),
                    attempt_id,
                });
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    merge_open_pull_request(
                        &ctx,
                        attempt_id,
                        &item_id,
                        &repo_full_name,
                        number,
                        &expected_head_sha,
                        method,
                        bypass_requirements,
                    )
                    .await;
                    ctx.merge_attempts.finish(&item_id, attempt_id);
                });
            }
            Command::OpenItem { id, navigation_id }
            | Command::RestoreItem { id, navigation_id } => {
                // Restoring is opening: the archived flag clears on the way
                // through and an owned checkout is put back by `prepare`.
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if open_item(&ctx, &id, navigation_id).await {
                        start_language_server(
                            ctx.slot,
                            ctx.language_server,
                            ctx.events,
                            ctx.navigation_requests,
                            navigation_id,
                        )
                        .await;
                    }
                });
            }
            Command::ViewAgentOwner { id, navigation_id } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let _navigation = ctx.review_navigation.lock().await;
                    if !ctx.navigation_requests.is_current(navigation_id) {
                        return;
                    }
                    let Some(item) = ctx.item(&id) else {
                        ctx.notice("That item is gone");
                        return;
                    };
                    if item.is_archived() {
                        let _lifecycle = ctx.item_lifecycle.lock().await;
                        if !ctx.navigation_requests.is_current(navigation_id) {
                            return;
                        }
                        show_retained_review_owner(&ctx, item, navigation_id);
                    } else if open_item(&ctx, &id, navigation_id).await {
                        start_language_server(
                            ctx.slot,
                            ctx.language_server,
                            ctx.events,
                            ctx.navigation_requests,
                            navigation_id,
                        )
                        .await;
                    }
                });
            }
            Command::ViewReviewAttemptOwner {
                item_id,
                attempt_id,
                request_id,
            } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    view_review_attempt_owner(&ctx, &item_id, &attempt_id, request_id).await;
                });
            }
            Command::ArchiveItem {
                id,
                suspended_navigation,
            } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) =
                        archive_item(&ctx, &id, Some(&suspended_navigation)).await
                    {
                        tracing::warn!(id, %message, "could not archive item");
                        let _ = ctx.events.send(Event::Notice(message));
                    }
                });
            }
            Command::DeleteItem { id } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(message) = delete_item(&ctx, &id).await {
                        tracing::warn!(id, %message, "could not delete item");
                        let _ = ctx.events.send(Event::Notice(message));
                    }
                });
            }
            Command::SetSaved { id, saved } => {
                edit_item(&ctx, &id, |item| {
                    item.saved = saved;
                });
            }
            Command::PinItem { id, pinned } => {
                edit_item(&ctx, &id, |item| {
                    item.pinned_at = pinned.then(Timestamp::now);
                });
            }
            Command::RenameItem { id, name } => {
                edit_item(&ctx, &id, |item| {
                    item.name = name.clone().filter(|n| !n.trim().is_empty());
                });
            }
            Command::StartWork {
                issue_item_id,
                navigation_id,
            } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    match start_work(&ctx, &issue_item_id, navigation_id).await {
                        Ok(id) => {
                            if open_item(&ctx, &id, navigation_id).await {
                                start_language_server(
                                    ctx.slot,
                                    ctx.language_server,
                                    ctx.events,
                                    ctx.navigation_requests,
                                    navigation_id,
                                )
                                .await;
                            }
                        }
                        Err(message) => {
                            ctx.send_navigation_event(
                                navigation_id,
                                Event::ReviewFailed {
                                    navigation_id,
                                    message,
                                },
                            );
                        }
                    }
                });
            }
            Command::ForkItem { id, navigation_id } => {
                ctx.clear_notice();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    match fork_item(&ctx, &id).await {
                        Ok(id) => {
                            if open_item(&ctx, &id, navigation_id).await {
                                start_language_server(
                                    ctx.slot,
                                    ctx.language_server,
                                    ctx.events,
                                    ctx.navigation_requests,
                                    navigation_id,
                                )
                                .await;
                            }
                        }
                        Err(message) => {
                            ctx.send_navigation_event(
                                navigation_id,
                                Event::ReviewFailed {
                                    navigation_id,
                                    message,
                                },
                            );
                        }
                    }
                });
            }
            Command::SetMeta { key, value } => {
                ctx.with_cache(|c| c.set_meta(&key, &value));
            }
            Command::Ask {
                item_id,
                thread_id,
                anchor,
                question,
                backend,
                model,
                effort,
            } => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    ask(
                        &ctx, item_id, thread_id, anchor, question, backend, model, effort,
                    )
                    .await;
                });
            }
            Command::CancelRun { run_id } => {
                ctx.runs.cancel(&run_id);
            }
            Command::RetryRun { run_id } => {
                let ctx = ctx.clone();
                tokio::spawn(async move { retry_run(&ctx, &run_id).await });
            }
            Command::LoadReviewDashboard { item_id } => {
                let ctx = ctx.clone();
                tokio::spawn(async move { reviews::load_dashboard(&ctx, &item_id).await });
            }
            Command::StartReview { request } => {
                let ctx = ctx.clone();
                tokio::spawn(async move { reviews::start(&ctx, request).await });
            }
            Command::SaveReviewPreference { choice } => {
                reviews::save_preference(&ctx, choice);
            }
            Command::StopReviewGroup { group_id } => {
                reviews::stop_group(&ctx, &group_id);
            }
            Command::RetryReviewAttempt { attempt_id } => {
                let ctx = ctx.clone();
                tokio::spawn(async move { reviews::retry_attempt(&ctx, &attempt_id).await });
            }
            Command::OpenReviewSnapshot {
                group_id,
                request_id,
            } => {
                let ctx = ctx.clone();
                tokio::spawn(
                    async move { reviews::open_snapshot(&ctx, &group_id, request_id).await },
                );
            }
            Command::DismissReviewFinding { finding_id, reason } => {
                reviews::dismiss_finding(&ctx, &finding_id, &reason);
            }
            Command::ReopenReviewFinding { finding_id } => {
                reviews::reopen_finding(&ctx, &finding_id);
            }
            Command::LinkReviewDuplicate {
                finding_id,
                canonical_finding_id,
            } => {
                reviews::link_duplicate(&ctx, &finding_id, &canonical_finding_id);
            }
            Command::UnlinkReviewDuplicate { finding_id } => {
                reviews::unlink_duplicate(&ctx, &finding_id);
            }
            Command::LoadAgentRuns { history_limit } => {
                send_agent_runs(&ctx, history_limit);
            }
            Command::LoadAgentRunDetail { run_id } => {
                send_agent_run_detail(&ctx, &run_id);
            }
            Command::LoadAgentRunEvents {
                run_id,
                before_sequence,
            } => {
                send_agent_run_events(&ctx, &run_id, before_sequence);
            }
            Command::DeleteCommentThread { id } => {
                let ctx = ctx.clone();
                tokio::spawn(async move { delete_comment_thread(&ctx, &id).await });
            }
            Command::OpenFile {
                path,
                navigation_id,
            } => {
                let preparer = preparer.clone();
                let checkout = checkout.clone();
                let events = events.clone();
                let language_server = language_server.clone();
                let navigation_requests = ctx.navigation_requests.clone();
                tokio::spawn(async move {
                    if open_file(
                        preparer,
                        checkout.clone(),
                        events,
                        navigation_requests.clone(),
                        path.clone(),
                        navigation_id,
                    )
                    .await
                    {
                        announce_file(
                            checkout,
                            language_server,
                            navigation_requests,
                            path,
                            navigation_id,
                        )
                        .await;
                    }
                });
            }
            Command::Hover {
                token,
                path,
                line,
                character,
            } => {
                let checkout = checkout.clone();
                let language_server = language_server.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let Some((client, full)) = resolve(&checkout, &language_server, &path).await
                    else {
                        return;
                    };
                    if let Some(hover) = client.hover(&full, line, character).await {
                        let text = render_hover(&hover);
                        if !text.trim().is_empty() {
                            let _ = events.send(Event::LspHover { token, text });
                        }
                    }
                });
            }
            Command::SetScope {
                item_id,
                scope,
                request_id,
                navigation_id,
            } => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    set_scope(&ctx, &item_id, scope, request_id, navigation_id).await
                });
            }
            Command::RefreshScope {
                item_id,
                navigation_id,
                scope_revision,
            } => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    refresh_scope(&ctx, &item_id, navigation_id, scope_revision).await
                });
            }
            Command::SetViewed {
                item_id,
                path,
                blob,
                viewed,
            } => {
                if let Some(Err(e)) =
                    ctx.with_cache(|c| c.set_viewed(&item_id, &path, &blob, viewed))
                {
                    tracing::warn!(error = %e, "could not record viewed");
                }
                send_viewed(&ctx, &item_id, false);
            }
            // The window applied these as they were clicked; nothing is
            // sent back.
            Command::Reveal {
                item_id,
                path,
                blob,
                first,
                last,
            } => {
                if let Some(Err(e)) =
                    ctx.with_cache(|c| c.set_reveal(&item_id, &path, &blob, first, last))
                {
                    tracing::warn!(error = %e, "could not record the reveal");
                }
            }
            Command::ResetReveals {
                item_id,
                path,
                blob,
                first,
                last,
            } => {
                if let Some(Err(e)) =
                    ctx.with_cache(|c| c.remove_reveals(&item_id, &path, &blob, first, last))
                {
                    tracing::warn!(error = %e, "could not forget the reveals");
                }
            }
            Command::References {
                token,
                path,
                line,
                character,
            } => {
                let checkout = checkout.clone();
                let language_server = language_server.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let Some((client, full)) = resolve(&checkout, &language_server, &path).await
                    else {
                        return;
                    };
                    let worktree = checkout
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()));
                    let Some(worktree) = worktree else { return };
                    let locations = client.references(&full, line, character).await;
                    let (references, elsewhere) = read_references(&worktree, &locations).await;
                    let _ = events.send(Event::LspReferences {
                        token,
                        references,
                        elsewhere,
                    });
                });
            }
            Command::Definition {
                token,
                path,
                line,
                character,
            } => {
                let checkout = checkout.clone();
                let language_server = language_server.clone();
                let events = events.clone();
                tokio::spawn(async move {
                    let Some((client, full)) = resolve(&checkout, &language_server, &path).await
                    else {
                        return;
                    };
                    let worktree = checkout
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()));
                    let Some(worktree) = worktree else { return };

                    if let Some(location) = client.definition(&full, line, character).await.first()
                    {
                        // A definition inside the worktree opens as a tab; one
                        // in a dependency has no path the rail can show.
                        let target = perch_lsp::client::uri_to_path(&location.uri.to_string());
                        let relative = target
                            .and_then(|p| p.strip_prefix(&worktree).ok().map(|r| r.to_path_buf()))
                            .map(|r| r.to_string_lossy().into_owned());
                        let _ = events.send(Event::LspDefinition {
                            token,
                            path: relative,
                            line: location.range.start.line,
                        });
                    }
                });
            }
            Command::CloseReview { navigation_id } => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let _lifecycle = ctx.item_lifecycle.lock().await;
                    let closed = ctx
                        .navigation_requests
                        .with_current(navigation_id, || {
                            if let Ok(mut guard) = ctx.slot.lock() {
                                *guard = None;
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if closed {
                        let mut guard = ctx.language_server.lock().await;
                        if let Some(client) = guard.take() {
                            tokio::spawn(async move { client.shutdown().await });
                        }
                    }
                });
            }
        }
    }
}

/// How long an owned checkout stays on disk without being opened before
/// the sweep archives its item.
const CHECKOUT_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Everything an item operation needs, so a spawned task takes one clone
/// rather than six.
#[derive(Clone)]
struct Ctx {
    /// Present when remote polling and GitHub mutations are available.
    /// Local items, cached conversations and agent runs do not require it.
    github: Option<Arc<GitHub>>,
    preparer: Arc<Preparer>,
    cache: SharedCache,
    config: Arc<Config>,
    slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    language_server: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    events: EventSender<Event>,
    /// Durable admission, the shared execution limit, and live children.
    runs: runs::RunManager,
    /// Installed native-review contracts, captured once after CLI discovery.
    review_capabilities: Arc<Vec<perch_agent::ReviewCapability>>,
    /// Perch-owned immutable source/object storage.
    review_snapshots: Arc<ReviewSnapshotStore>,
    /// Latest view navigation, shared with the synchronous UI command handle
    /// so stale work is fenced before the worker receives the newer command.
    navigation_requests: Arc<NavigationRequests>,
    /// Latest requested diff scope, registered synchronously so concurrent
    /// Git reads cannot publish out of selection order.
    scope_requests: Arc<ScopeRequests>,
    /// Latest live checks/mergeability read for each item.
    status_requests: Arc<PullRequestStatusRequests>,
    /// Monotonic merge identities and the one in-flight mutation per item.
    merge_attempts: Arc<MergeAttempts>,
    /// Serialize reviewer-child owner opens. A newer click registers
    /// synchronously and makes the older worker stale; serialization then
    /// guarantees the newest accepted owner is the final backend slot.
    review_navigation: Arc<tokio::sync::Mutex<()>>,
    /// Admission starts before a snapshot has a database row. Fence those
    /// captures in memory so a double click cannot create two costly copies.
    review_admissions: Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>,
    /// Reconciliation reloads durable findings under this short lock. The
    /// database transaction remains the ultimate replay fence.
    review_reconcile: Arc<tokio::sync::Mutex<()>>,
    /// Plain Markdown export is best-effort and never gates durable review.
    knowledge: Option<Arc<perch_knowledge::Knowledge>>,
    /// Serializes checkout archive/restore/delete so deferred agent cleanup
    /// cannot remove a worktree while it is being restored or deleted.
    item_lifecycle: Arc<tokio::sync::Mutex<()>>,
    /// The avatar download limit, shared by everything that wants a face.
    avatars: Arc<Semaphore>,
    /// Notifications already acted on, by id and time, so one GitHub keeps
    /// returning does not refresh an open item again on every poll.
    seen_notifications: Arc<std::sync::Mutex<std::collections::HashSet<(String, Timestamp)>>>,
    /// Items whose GitHub side is being fetched again right now.
    refreshing: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// GitHub's inbox as the last successful poll saw it. This is distinct
    /// from durable items and remains usable when the SQLite cache cannot
    /// be opened.
    notifications: Arc<std::sync::Mutex<Vec<Thread>>>,
}

impl Ctx {
    /// Have these faces on hand, from disk or the network.
    fn want_avatars(&self, urls: impl IntoIterator<Item = String>) {
        let mut seen = std::collections::HashSet::new();
        for url in urls {
            if url.is_empty() || !seen.insert(url.clone()) {
                continue;
            }
            spawn_avatar(
                self.github.clone(),
                self.cache.clone(),
                self.events.clone(),
                self.avatars.clone(),
                url,
            );
        }
    }

    fn send_threads(&self, item_id: &str) {
        let view = self.slot.lock().ok().and_then(|slot| {
            slot.as_ref()
                .filter(|open| open.item_id == item_id)
                .map(|open| {
                    (
                        open.scope.clone(),
                        open.checkout.merge_base.clone(),
                        open.checkout.head_sha.clone(),
                        open.review_snapshot_id.clone(),
                        open.review_group_id.clone(),
                        open.review_request_id,
                    )
                })
        });
        let review_fence = view
            .as_ref()
            .and_then(
                |(_, _, _, _, group_id, request_id)| match (group_id, request_id) {
                    (Some(group_id), Some(request_id)) => Some((group_id.clone(), *request_id)),
                    _ => None,
                },
            );
        let threads = self.with_cache(|cache| -> Result<Vec<CommentThread>, CacheError> {
            let mut threads = cache.load_comment_threads(item_id)?;
            let Some((scope, merge_base, head, exact_snapshot_id, historical_group_id, _)) =
                view.as_ref()
            else {
                return Ok(threads);
            };
            let findings = cache.list_findings(item_id, None)?;
            let finding_threads = findings
                .iter()
                .map(|finding| finding.thread_id.clone())
                .collect::<std::collections::HashSet<_>>();

            // Durable review anchors describe a full committed branch
            // snapshot. Commit and working-tree scopes show a different
            // pair of Git sides, so their numeric lines are never safe
            // inline there. Keep the thread available in Conversation as
            // outdated/original evidence until Branch is selected again.
            if *scope != perch_git::Scope::Branch {
                Self::mark_review_threads_outdated(&mut threads, &finding_threads);
                return Ok(threads);
            }

            let target_snapshot = match exact_snapshot_id.as_deref() {
                Some(snapshot_id) => cache.load_review_snapshot(snapshot_id)?,
                None => cache
                    .list_review_snapshots(item_id)?
                    .into_iter()
                    .find(|snapshot| {
                        snapshot.merge_base_sha == *merge_base && snapshot.head_sha == *head
                    }),
            };

            // A live checkout which has moved away from every retained
            // snapshot has no safe numeric projection. Keep the durable
            // original anchor only as an outdated/original-code affordance;
            // the inline renderer deliberately does not place outdated
            // anchors on the displayed file.
            let Some(target_snapshot) = target_snapshot else {
                Self::mark_review_threads_outdated(&mut threads, &finding_threads);
                return Ok(threads);
            };

            let historical = match historical_group_id.as_deref() {
                Some(group_id) => cache.load_review_group(group_id)?,
                None => None,
            };
            let historical_attempts = match historical.as_ref() {
                Some(group) => cache
                    .list_reviewer_attempts(&group.id)?
                    .into_iter()
                    .map(|attempt| attempt.id)
                    .collect::<std::collections::HashSet<_>>(),
                None => std::collections::HashSet::new(),
            };
            let mut historical_findings = std::collections::HashSet::new();

            // Finding-thread anchors are immutable evidence. Project only the
            // in-memory copy sent to the current view, using an observation or
            // recorded mapping which explicitly targets the displayed
            // snapshot. The durable original anchor remains untouched.
            for finding in findings {
                let observations = cache.list_finding_observations(&finding.id)?;
                let eligible = observations
                    .iter()
                    .filter(|observation| {
                        historical.is_none()
                            || historical_attempts.contains(&observation.attempt_id)
                    })
                    .collect::<Vec<_>>();
                if historical.is_some() && eligible.is_empty() {
                    continue;
                }
                if historical.is_some() {
                    historical_findings.insert(finding.thread_id.clone());
                }
                let observation_projection = eligible.into_iter().rev().find_map(|observation| {
                    (observation.snapshot_id == target_snapshot.id
                        && matches!(
                            observation.kind,
                            perch_github::ObservationKind::Reported
                                | perch_github::ObservationKind::VerifiedPresent
                        ))
                    .then(|| observation.anchor.clone())
                    .flatten()
                });
                let projected = match observation_projection {
                    Some(anchor) => Some(anchor),
                    None => {
                        // Mapping rows are newest first. An explicit newest
                        // Unmapped decision must shadow an older Mapped row;
                        // otherwise a force-push can revive a stale line.
                        cache
                            .list_anchor_mappings(&finding.id, Some(&target_snapshot.id))?
                            .into_iter()
                            .find(|mapping| {
                                historical.is_none()
                                    || mapping.created_by_attempt_id.as_ref().is_some_and(
                                        |attempt_id| historical_attempts.contains(attempt_id),
                                    )
                            })
                            .and_then(|mapping| {
                                (mapping.status == perch_github::AnchorMappingStatus::Mapped)
                                    .then_some(mapping.target_anchor)
                                    .flatten()
                            })
                    }
                };
                let Some(thread) = threads
                    .iter_mut()
                    .find(|thread| thread.id == finding.thread_id)
                else {
                    continue;
                };
                thread.outdated = true;
                let Some(projected) = projected else { continue };
                thread.anchor = Some(perch_github::Anchor {
                    path: projected.path,
                    start_line: projected.start_line,
                    line: projected.end_line,
                    side: projected.side,
                    commit_sha: projected.commit_sha,
                });
                thread.outdated = false;
            }

            if let Some(group) = historical {
                let cutoff = group.ended_at.unwrap_or_else(Timestamp::now);
                threads.retain(|thread| {
                    if finding_threads.contains(&thread.id) {
                        historical_findings.contains(&thread.id)
                    } else {
                        thread.created_at <= cutoff
                    }
                });
                for thread in &mut threads {
                    for message in &mut thread.messages {
                        if message.created_at > cutoff {
                            message.body = if message.body.trim().is_empty() {
                                format!("[After Review #{} · later discussion]", group.number)
                            } else {
                                format!(
                                    "[After Review #{} · later discussion]\n\n{}",
                                    group.number, message.body
                                )
                            };
                        }
                    }
                }
            }
            Ok(threads)
        });
        match threads {
            Some(Ok(threads)) => {
                let _ = self.events.send(Event::CommentThreads {
                    item_id: item_id.to_string(),
                    threads,
                    review_group_id: review_fence.as_ref().map(|(group_id, _)| group_id.clone()),
                    review_request_id: review_fence.as_ref().map(|(_, request_id)| *request_id),
                });
            }
            Some(Err(e)) => tracing::warn!(error = %e, "could not load threads"),
            None => {}
        }
    }

    fn mark_review_threads_outdated(
        threads: &mut [CommentThread],
        finding_threads: &std::collections::HashSet<String>,
    ) {
        for thread in threads {
            if finding_threads.contains(&thread.id) {
                thread.outdated = true;
            }
        }
    }

    /// Run one closure against the cache. The guard never crosses an await.
    fn with_cache<T>(&self, f: impl FnOnce(&Cache) -> T) -> Option<T> {
        let cache = self.cache.as_ref()?;
        let guard = cache.lock().ok()?;
        Some(f(&guard))
    }

    fn item(&self, id: &str) -> Option<Item> {
        self.with_cache(|c| c.load_item(id).ok().flatten())
            .flatten()
    }

    fn send_items(&self) {
        self.with_cache(|c| send_items(c, &self.events));
    }

    fn notifications_last_modified(&self) -> Option<String> {
        match self.with_cache(Cache::notifications_last_modified) {
            Some(Ok(value)) => value,
            Some(Err(e)) => {
                tracing::warn!(error = %e, "could not read the notification validator");
                None
            }
            None => None,
        }
    }

    /// Replace the visible notification snapshot, apply local Done watermarks,
    /// persist it when possible, and publish it whole so the remaining rows
    /// retain GitHub's response order.
    fn replace_notifications(
        &self,
        mut threads: Vec<Thread>,
        last_modified: Option<&str>,
    ) -> Vec<Thread> {
        // A local mark-unread reminder belongs to this exact notification
        // revision. Carry it across a full GitHub snapshot, but let newer
        // activity (a changed updated_at) replace it with GitHub's state.
        let local_unread = self
            .notifications
            .lock()
            .map(|current| {
                current
                    .iter()
                    .filter(|thread| thread.local_unread)
                    .map(|thread| (thread.id.clone(), thread.updated_at))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        for thread in &mut threads {
            if !thread.unread && local_unread.contains(&(thread.id.clone(), thread.updated_at)) {
                thread.unread = true;
                thread.local_unread = true;
            }
        }
        if let Some(result) = self.with_cache(|c| c.store_threads(&threads, last_modified)) {
            match result {
                Ok(visible) => threads = visible,
                Err(e) => tracing::warn!(error = %e, "could not cache notifications"),
            }
        }
        let current_keys: std::collections::HashSet<(String, Timestamp)> = threads
            .iter()
            .map(|thread| (thread.id.clone(), thread.updated_at))
            .collect();
        if let Ok(mut seen) = self.seen_notifications.lock() {
            seen.retain(|key| current_keys.contains(key));
        }
        if let Ok(mut current) = self.notifications.lock() {
            *current = threads.clone();
        }
        self.want_avatars(
            threads
                .iter()
                .map(|t| t.repository.owner_avatar_url.clone()),
        );
        let _ = self.events.send(Event::Notifications(threads.clone()));
        threads
    }

    /// Apply a notification mutation without touching the item, checkout,
    /// or conversation it may refer to.
    fn edit_notifications(&self, edit: impl FnOnce(&mut Vec<Thread>)) {
        let threads = match self.notifications.lock() {
            Ok(mut current) => {
                edit(&mut current);
                current.clone()
            }
            Err(_) => return,
        };
        let last_modified = self.notifications_last_modified();
        if let Some(Err(e)) =
            self.with_cache(|c| c.store_threads(&threads, last_modified.as_deref()))
        {
            tracing::warn!(error = %e, "could not cache changed notifications");
        }
        let _ = self.events.send(Event::Notifications(threads));
    }

    fn mark_notification_unread(&self, thread_id: &str) {
        self.edit_notifications(|threads| {
            if let Some(thread) = threads
                .iter_mut()
                .find(|thread| thread.id == thread_id && !thread.unread)
            {
                thread.unread = true;
                thread.local_unread = true;
            }
        });
    }

    /// Hide the exact revision GitHub just accepted as Done. GitHub's list
    /// endpoint can return Done history again under `all=true`, so the local
    /// watermark is what keeps the row out until newer activity arrives.
    fn dismiss_notification(&self, thread_id: &str) {
        let updated_at = self.notifications.lock().ok().and_then(|threads| {
            threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .map(|thread| thread.updated_at)
        });
        if let Some(updated_at) = updated_at {
            if let Some(Err(error)) =
                self.with_cache(|cache| cache.dismiss_notification(thread_id, updated_at))
            {
                tracing::warn!(thread = %thread_id, error = %error, "could not remember notification dismissal");
                self.notice(format!(
                    "The notification was marked Done, but Perch could not remember it: {error}"
                ));
            }
        }
        self.edit_notifications(|threads| {
            threads.retain(|thread| thread.id != thread_id);
        });
    }

    /// Consume a Perch-only unread reminder and report whether GitHub still
    /// needs a mark-read request. Consult the current inbox rather than the
    /// UI's captured row so a quick Mark Unread then Open cannot miss it.
    fn prepare_notification_open(&self, opened: &Thread) -> bool {
        let (unread, local_unread) = self
            .notifications
            .lock()
            .ok()
            .and_then(|threads| {
                threads
                    .iter()
                    .find(|thread| thread.id == opened.id)
                    .map(|thread| (thread.unread, thread.local_unread))
            })
            .unwrap_or((opened.unread, opened.local_unread));
        if local_unread {
            self.edit_notifications(|threads| {
                if let Some(thread) = threads.iter_mut().find(|thread| thread.id == opened.id) {
                    thread.unread = false;
                    thread.local_unread = false;
                }
            });
        }
        unread && !local_unread
    }

    fn notice(&self, message: impl Into<String>) {
        let _ = self.events.send(Event::Notice(message.into()));
    }

    fn send_navigation_event(&self, navigation_id: u64, event: Event) {
        let _ = self.navigation_requests.with_current(navigation_id, || {
            let _ = self.events.send(event);
        });
    }

    fn cancel_committed_archived_navigation(&self, item_id: &str, navigation_id: u64) {
        if self
            .navigation_requests
            .is_committed_archive_fence(item_id, navigation_id)
        {
            let _ = self.events.send(Event::NavigationCancelled {
                item_id: item_id.to_string(),
                navigation_id,
            });
        }
    }

    /// The reader is doing something new, so the last complaint has been
    /// read. An empty notice is how the UI hears that.
    fn clear_notice(&self) {
        let _ = self.events.send(Event::Notice(String::new()));
    }

    /// Whether the open review is this item.
    fn is_open(&self, item_id: &str) -> bool {
        self.slot
            .lock()
            .ok()
            .is_some_and(|g| g.as_ref().is_some_and(|c| c.item_id == item_id))
    }

    /// Drop the open review and stop its language server.
    async fn close_review(&self) {
        if let Ok(mut guard) = self.slot.lock() {
            *guard = None;
        }
        let client = self.language_server.lock().await.take();
        if let Some(client) = client {
            client.shutdown().await;
        }
    }
}

fn send_items(cache: &Cache, events: &EventSender<Event>) {
    match cache.load_items() {
        Ok(items) => {
            let _ = events.send(Event::Items(items));
        }
        Err(e) => tracing::warn!(error = %e, "could not read items"),
    }
}

fn review_group_summary(
    cache: &Cache,
    group_id: &str,
) -> Result<Option<crate::review::model::ReviewGroupSummary>, String> {
    let Some(group) = cache
        .load_review_group(group_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some(snapshot) = cache
        .load_review_snapshot(&group.snapshot_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let attempts = cache
        .list_reviewer_attempts(group_id)
        .map_err(|error| error.to_string())?;
    Ok(Some(crate::review::model::ReviewGroupSummary {
        group,
        snapshot,
        attempts,
    }))
}

fn fail_review_attempt_navigation(
    ctx: &Ctx,
    item_id: &str,
    attempt_id: &str,
    request_id: u64,
    message: impl Into<String>,
) {
    let message = message.into();
    if ctx
        .navigation_requests
        .rollback_current_attempt(attempt_id, request_id)
    {
        let _ = ctx.events.send(Event::AgentReviewNavigationFailed {
            item_id: item_id.to_string(),
            attempt_id: attempt_id.to_string(),
            request_id,
            message: message.clone(),
        });
    }
}

/// Resolve a reviewer attempt to its immutable group before opening its
/// owner. Event order selects the exact group before its historical files
/// arrive, while the UI navigation token rejects stale targets.
async fn view_review_attempt_owner(ctx: &Ctx, item_id: &str, attempt_id: &str, request_id: u64) {
    let resolved = ctx.with_cache(|cache| {
        let attempt = cache
            .load_reviewer_attempt(attempt_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "That reviewer attempt no longer exists".to_string())?;
        let summary = review_group_summary(cache, &attempt.group_id)?
            .ok_or_else(|| "That review group is unavailable".to_string())?;
        if summary.group.item_id != item_id {
            return Err("That reviewer attempt belongs to another item".to_string());
        }
        Ok::<_, String>(summary)
    });
    let Some(result) = resolved else {
        fail_review_attempt_navigation(
            ctx,
            item_id,
            attempt_id,
            request_id,
            "The review history database is unavailable",
        );
        return;
    };
    let summary = match result {
        Ok(summary) => summary,
        Err(message) => {
            fail_review_attempt_navigation(ctx, item_id, attempt_id, request_id, message);
            return;
        }
    };

    if !ctx
        .navigation_requests
        .is_current_attempt(attempt_id, request_id)
    {
        return;
    }
    let _navigation = ctx.review_navigation.lock().await;
    if !ctx
        .navigation_requests
        .is_current_attempt(attempt_id, request_id)
    {
        return;
    }

    let owner_was_open = ctx.is_open(item_id);
    if !owner_was_open {
        match ctx.item(item_id) {
            Some(item) if item.is_archived() => {
                let _lifecycle = ctx.item_lifecycle.lock().await;
                if !ctx
                    .navigation_requests
                    .is_current_attempt(attempt_id, request_id)
                {
                    return;
                }
                show_retained_review_owner(ctx, item, request_id);
            }
            Some(_) => {
                let _ = open_item(ctx, item_id, request_id).await;
            }
            None => {
                fail_review_attempt_navigation(
                    ctx,
                    item_id,
                    attempt_id,
                    request_id,
                    "That review owner is gone",
                );
                return;
            }
        }
    }
    if !ctx.is_open(item_id) {
        fail_review_attempt_navigation(
            ctx,
            item_id,
            attempt_id,
            request_id,
            "The review owner could not be opened",
        );
        return;
    }
    if !owner_was_open
        && !ctx.navigation_requests.checkpoint_attempt_owner(
            item_id,
            attempt_id,
            request_id,
        )
    {
        return;
    }
    if !owner_was_open {
        let mut language_server = ctx.language_server.lock().await;
        let previous = ctx
            .navigation_requests
            .with_current(request_id, || {
                let current_owner = ctx.slot.lock().ok().is_some_and(|slot| {
                    slot.as_ref().is_some_and(|open| {
                        open.item_id == item_id && open.navigation_id == request_id
                    })
                });
                current_owner.then(|| language_server.take()).flatten()
            })
            .flatten();
        drop(language_server);
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
    }
    let group_id = summary.group.id.clone();
    if !ctx
        .navigation_requests
        .bind_attempt_to_group(attempt_id, &group_id, request_id)
    {
        return;
    }
    let published = ctx
        .navigation_requests
        .with_current_group(&group_id, request_id, || {
            let _ = ctx.events.send(Event::AgentReviewNavigationReady {
                item_id: item_id.to_string(),
                attempt_id: attempt_id.to_string(),
                request_id,
                owner_changed: !owner_was_open,
                summary,
            });
        })
        .is_some();
    if !published {
        return;
    }
    reviews::open_snapshot(ctx, &group_id, request_id).await;
}

/// Publish every active summary plus one bounded page of terminal history.
/// The panel filters this snapshot locally; loading an older page asks again
/// with a larger limit, so startup never reads every transcript.
fn send_agent_runs(ctx: &Ctx, history_limit: usize) {
    // A process may have exited while its terminal SQLite write was
    // transiently unavailable. Any list refresh is an opportunity to heal
    // that projection before publishing the active set.
    ctx.runs.reconcile_finished();
    send_agent_runs_from_cache(&ctx.cache, &ctx.events, history_limit);
}

const AGENT_HISTORY_PAGE: usize = 100;

fn send_agent_runs_from_cache(
    cache: &SharedCache,
    events: &EventSender<Event>,
    history_limit: usize,
) {
    let Some(cache) = cache.as_ref() else {
        return;
    };
    let Ok(guard) = cache.lock() else { return };
    let mut runs = match guard.active_run_summaries() {
        Ok(runs) => runs,
        Err(error) => {
            tracing::warn!(%error, "could not load active agents");
            return;
        }
    };
    let wanted = history_limit.max(1);
    let mut history = Vec::with_capacity(wanted);
    let mut cursor = None;
    let has_more = loop {
        let page_size = (wanted - history.len()).min(AGENT_HISTORY_PAGE);
        let page = match guard.run_history_page(
            &RunHistoryFilter::default(),
            cursor.as_ref(),
            page_size,
        ) {
            Ok(page) => page,
            Err(error) => {
                tracing::warn!(%error, "could not load agent history");
                return;
            }
        };
        let page_was_empty = page.runs.is_empty();
        history.extend(page.runs);
        if history.len() >= wanted || page_was_empty {
            break page.next_cursor.is_some();
        }
        let Some(next) = page.next_cursor else {
            break false;
        };
        cursor = Some(next);
    };
    runs.extend(history);

    // Reviewer children need their owning group even when the item has not
    // been opened in this process. Publish those summaries first so the
    // Agents list never has to infer grouping from labels or run order.
    let mut review_group_ids = std::collections::BTreeSet::new();
    for run in &runs {
        let RunOwner::Review {
            reviewer_run_id, ..
        } = &run.owner
        else {
            continue;
        };
        match guard.load_reviewer_attempt(reviewer_run_id) {
            Ok(Some(attempt)) => {
                review_group_ids.insert(attempt.group_id);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(attempt = reviewer_run_id, %error, "could not resolve reviewer group");
            }
        }
    }
    for group_id in review_group_ids {
        match review_group_summary(&guard, &group_id) {
            Ok(Some(summary)) => {
                let _ = events.send(Event::ReviewGroupUpdated {
                    item_id: summary.group.item_id.clone(),
                    summary,
                });
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(group = group_id, %error, "could not load review group for Agents");
            }
        }
    }
    let _ = events.send(Event::AgentRuns { runs, has_more });
}

const AGENT_EVENT_PAGE: usize = 100;

fn load_agent_event_page(
    cache: &Cache,
    run_id: &str,
    before_sequence: Option<u64>,
) -> Result<(Vec<RunEventRecord>, bool), perch_github::CacheError> {
    let mut events = cache.load_run_events(run_id, before_sequence, AGENT_EVENT_PAGE + 1)?;
    let has_more = events.len() > AGENT_EVENT_PAGE;
    if has_more {
        // The cache returns a chronological window over the newest rows
        // before the cursor. The extra oldest row proves another page.
        events.remove(0);
    }
    Ok((events, has_more))
}

fn send_agent_run_detail(ctx: &Ctx, run_id: &str) {
    let Some(cache) = ctx.cache.as_ref() else {
        return;
    };
    let Ok(guard) = cache.lock() else { return };
    let Ok(Some(run)) = guard.load_run(run_id) else {
        ctx.notice("That agent run is no longer available");
        return;
    };
    let (events, has_more) = match load_agent_event_page(&guard, run_id, None) {
        Ok(page) => page,
        Err(error) => {
            tracing::warn!(run = run_id, %error, "could not load agent activity");
            (Vec::new(), false)
        }
    };
    let _ = ctx.events.send(Event::AgentRunDetail {
        run,
        events,
        has_more,
    });
}

fn send_agent_run_events(ctx: &Ctx, run_id: &str, before_sequence: u64) {
    let Some(cache) = ctx.cache.as_ref() else {
        return;
    };
    let Ok(guard) = cache.lock() else { return };
    match load_agent_event_page(&guard, run_id, Some(before_sequence)) {
        Ok((events, has_more)) => {
            let _ = ctx.events.send(Event::AgentRunEvents {
                run_id: run_id.to_string(),
                events,
                has_more,
            });
        }
        Err(error) => {
            tracing::warn!(run = run_id, %error, "could not load earlier agent activity");
            let _ = ctx.events.send(Event::AgentRunEvents {
                run_id: run_id.to_string(),
                events: Vec::new(),
                has_more: true,
            });
            ctx.notice("Could not load earlier agent activity");
        }
    }
}

/// Apply one change to an item's row and tell the UI.
fn edit_item(ctx: &Ctx, id: &str, edit: impl FnOnce(&mut Item)) {
    let Some(mut item) = ctx.item(id) else {
        ctx.notice("That item is gone");
        return;
    };
    edit(&mut item);
    if let Some(Err(e)) = ctx.with_cache(|c| c.update_item(&item)) {
        ctx.notice(format!("Could not save the change: {e}"));
        return;
    }
    ctx.send_items();
}

/// Make the items table and the disk agree about owned checkouts.
///
/// An owned checkout whose directory is gone is marked off disk; someone
/// deleted it by hand. A directory under the store's worktrees that no
/// item claims is removed and pruned: nothing can open it, and it is how
/// what an older schema left behind gets retired.
async fn reconcile_checkouts(ctx: &Ctx, reset: bool) {
    let Some(items) = ctx.with_cache(|c| c.load_items().ok()).flatten() else {
        return;
    };

    let mut changed = false;
    for item in &items {
        let Some(checkout) = &item.checkout else {
            continue;
        };
        if !checkout.owned {
            continue;
        }
        let present = checkout.path.join(".git").exists();
        if checkout.on_disk != present {
            tracing::info!(id = %item.id, path = %checkout.path.display(), present, "checkout presence changed");
            let mut updated = item.clone();
            if let Some(c) = updated.checkout.as_mut() {
                c.on_disk = present;
            }
            ctx.with_cache(|c| c.update_item(&updated));
            changed = true;
        }
    }

    // An archive can intentionally leave its checkout in place while an
    // agent is still using it. A clean shutdown normally retires it through
    // the deferred waiter; after a crash, startup reconciliation completes
    // that same cleanup once persisted runs have been marked Interrupted.
    retire_archived_checkouts(ctx).await;

    // Directories no row claims. After a reset they are every checkout
    // there was, and they stay: opening the pull request again finds its
    // directory where it left it. Otherwise a clean one is a leftover and
    // goes; one with uncommitted work in it is somebody's, whatever the
    // table says, and is left alone.
    if reset {
        if changed {
            ctx.send_items();
        }
        return;
    }
    let known: std::collections::HashSet<std::path::PathBuf> = items
        .iter()
        .filter_map(|i| i.checkout.as_ref().map(|c| c.path.clone()))
        .collect();
    let mut removed = false;
    for dir in ctx.preparer.worktree_dirs() {
        if known.contains(&dir) {
            continue;
        }
        if dir.join(".git").exists() {
            match ctx.preparer.is_dirty(&dir).await {
                Ok(false) => {}
                Ok(true) => {
                    tracing::info!(path = %dir.display(), "leaving an unclaimed worktree with changes in it");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(path = %dir.display(), error = %e, "could not tell if an unclaimed worktree is dirty; leaving it");
                    continue;
                }
            }
        }
        tracing::info!(path = %dir.display(), "removing a worktree directory no item claims");
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "could not remove it");
        }
        removed = true;
    }
    if removed {
        prune_every_clone(ctx).await;
    }

    if changed {
        ctx.send_items();
    }
}

/// `git worktree prune` in every bare clone, after directories were removed
/// without knowing which clone they belonged to.
async fn prune_every_clone(ctx: &Ctx) {
    let repos = ctx.preparer.store().root().join("repos").join("github.com");
    let Ok(owners) = std::fs::read_dir(repos) else {
        return;
    };
    for owner in owners.flatten() {
        let Ok(clones) = std::fs::read_dir(owner.path()) else {
            continue;
        };
        for clone in clones.flatten() {
            let path = clone.path();
            if path.join("HEAD").is_file() {
                let _ = ctx
                    .preparer
                    .git()
                    .run(Some(&path), ["worktree", "prune"], Duration::from_secs(30))
                    .await;
            }
        }
    }
}

/// Archive items whose owned checkout has sat unopened for too long. An
/// issue, or a checkout under a root, is archived only by hand.
///
/// Age is the row's `last_opened_at`, which is what the rule is about. A
/// directory's mtime is not: an agent reading it, or a fetch touching
/// `.git`, would keep a forgotten checkout young for ever.
async fn sweep_items(ctx: &Ctx) {
    let items = ctx
        .with_cache(|c| c.load_items().ok())
        .flatten()
        .unwrap_or_default();
    let cutoff =
        Timestamp::now() - jiff::SignedDuration::try_from(CHECKOUT_MAX_AGE).unwrap_or_default();

    for item in &items {
        let owned = item.checkout.as_ref().is_some_and(|c| c.owned && c.on_disk);
        if !owned || item.is_archived() || item.last_opened_at > cutoff {
            continue;
        }
        match archive_item(ctx, &item.id, None).await {
            Ok(()) => tracing::info!(id = %item.id, "archived a stale checkout"),
            // Dirty, most likely. It stays; the reader has work in it.
            Err(message) => tracing::info!(id = %item.id, %message, "left a stale checkout alone"),
        }
    }
}

/// Leave Active. The row, branch, discussions, and run history stay. An
/// executing agent keeps its checkout until the owner's final run exits.
async fn archive_item(
    ctx: &Ctx,
    id: &str,
    suspended_navigation: Option<&SuspendedNavigation>,
) -> Result<(), String> {
    let lifecycle = ctx.item_lifecycle.lock().await;
    if suspended_navigation.is_some_and(|suspended| {
        !ctx.navigation_requests
            .archive_fence_is_current(suspended)
    }) {
        return Ok(());
    }
    let Some(item) = ctx.item(id) else {
        if let Some(suspended) = suspended_navigation {
            ctx.navigation_requests.restore_suspended(suspended);
        }
        return Err("That item is gone".into());
    };
    if let Some(checkout) = item.checkout.as_ref().filter(|c| c.owned && c.on_disk) {
        if checkout.path.join(".git").exists() {
            let dirty = match ctx.preparer.is_dirty(&checkout.path).await {
                Ok(dirty) => dirty,
                Err(error) => {
                    if let Some(suspended) = suspended_navigation {
                        ctx.navigation_requests.restore_suspended(suspended);
                    }
                    return Err(error.to_string());
                }
            };
            if dirty {
                if let Some(suspended) = suspended_navigation {
                    ctx.navigation_requests.restore_suspended(suspended);
                }
                return Err(format!("{} has uncommitted changes", checkout.branch));
            }
        }
    }
    if let Some(suspended) = suspended_navigation {
        if !ctx
            .navigation_requests
            .archive_fence_is_current(suspended)
            || !ctx
                .navigation_requests
                .commit_archive_fence(suspended)
        {
            return Ok(());
        }
    }
    if ctx.is_open(id) {
        ctx.close_review().await;
    }

    // Persist the archive first. Admission validates this row under the
    // cache mutex, so work admitted before this point is visible to the run
    // manager and work arriving after it is rejected.
    let mut item = ctx.item(id).unwrap_or(item);
    item.archived_at = Some(Timestamp::now());
    if let Err(error) = ctx
        .with_cache(|c| c.update_item(&item))
        .transpose()
        .map_err(|e| e.to_string())
    {
        if let Some(suspended) = suspended_navigation {
            ctx.navigation_requests.restore_suspended(suspended);
        }
        return Err(error);
    }
    let _ = ctx.events.send(Event::ItemArchived {
        item_id: id.to_string(),
        cancelled_navigation_id: suspended_navigation
            .and_then(|suspended| suspended.previous.as_ref())
            .map(|request| request.id),
    });
    ctx.send_items();

    let active = ctx.runs.has_active_owner(id, None);
    drop(lifecycle);
    if active {
        let ctx = ctx.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            ctx.runs.wait_owner_idle(&id, None).await;
            if let Err(error) = retire_archived_checkout(&ctx, &id).await {
                tracing::info!(item = id, %error, "kept an archived checkout on disk");
            }
        });
        Ok(())
    } else {
        if let Err(error) = retire_archived_checkout(ctx, id).await {
            tracing::info!(item = id, %error, "archived item; checkout cleanup will retry later");
        }
        Ok(())
    }
}

/// Remove an archived owned checkout once no process can still be using it.
/// Re-read under the lifecycle lock so Restore or Delete always wins cleanly.
async fn retire_archived_checkout(ctx: &Ctx, id: &str) -> Result<(), String> {
    let _lifecycle = ctx.item_lifecycle.lock().await;
    if ctx.runs.has_active_owner(id, None) {
        return Ok(());
    }
    retire_archived_checkout_from_cache(&ctx.cache, &ctx.preparer, &ctx.events, id).await
}

/// Finish every archive whose checkout is still present. The live wrapper
/// checks process ownership and serializes item operations; the startup path
/// calls the cache form directly after nonterminal runs have been reconciled.
async fn retire_archived_checkouts(ctx: &Ctx) {
    let items = ctx
        .with_cache(|cache| cache.load_items().unwrap_or_default())
        .unwrap_or_default();
    for item in items.iter().filter(|item| archived_checkout_on_disk(item)) {
        if let Err(error) = retire_archived_checkout(ctx, &item.id).await {
            tracing::info!(item = %item.id, %error, "kept an archived checkout on disk");
        }
    }
}

async fn retire_archived_checkouts_from_cache(
    cache: &SharedCache,
    preparer: &Preparer,
    events: &EventSender<Event>,
) {
    let items = cache
        .as_ref()
        .and_then(|cache| cache.lock().ok())
        .and_then(|cache| cache.load_items().ok())
        .unwrap_or_default();
    for item in items.iter().filter(|item| archived_checkout_on_disk(item)) {
        if let Err(error) =
            retire_archived_checkout_from_cache(cache, preparer, events, &item.id).await
        {
            tracing::info!(item = %item.id, %error, "kept an archived checkout on disk");
        }
    }
}

fn archived_checkout_on_disk(item: &Item) -> bool {
    item.is_archived()
        && item
            .checkout
            .as_ref()
            .is_some_and(|checkout| checkout.owned && checkout.on_disk)
}

async fn retire_archived_checkout_from_cache(
    cache: &SharedCache,
    preparer: &Preparer,
    events: &EventSender<Event>,
    id: &str,
) -> Result<(), String> {
    let item = cache
        .as_ref()
        .ok_or_else(|| "the local database is unavailable".to_string())?
        .lock()
        .map_err(|_| "the local database is unavailable".to_string())?
        .load_item(id)
        .map_err(|error| error.to_string())?;
    let Some(item) = item.filter(archived_checkout_on_disk) else {
        return Ok(());
    };
    let checkout = item.checkout.as_ref().cloned().unwrap();
    if checkout.path.join(".git").exists() {
        if preparer
            .is_dirty(&checkout.path)
            .await
            .map_err(|error| error.to_string())?
        {
            return Err(format!("{} has uncommitted changes", checkout.branch));
        }
        let repo = RepoId::parse(&item.repo_full_name)
            .ok_or_else(|| format!("{} is not a repository name", item.repo_full_name))?;
        let bare = preparer.store().bare_path(&repo);
        preparer
            .remove_worktree(&bare, &checkout.path)
            .await
            .map_err(|error| error.to_string())?;
    }
    let guard = cache
        .as_ref()
        .ok_or_else(|| "the local database is unavailable".to_string())?
        .lock()
        .map_err(|_| "the local database is unavailable".to_string())?;
    let Some(mut item) = guard
        .load_item(id)
        .map_err(|error| error.to_string())?
        .filter(|item| item.is_archived())
    else {
        return Ok(());
    };
    if let Some(checkout) = item.checkout.as_mut() {
        checkout.on_disk = false;
    }
    guard
        .update_item(&item)
        .map_err(|error| error.to_string())?;
    send_items(&guard, events);
    Ok(())
}

/// Remove the row and its threads. An owned checkout's directory and
/// branch go too; a checkout under a root is not touched.
async fn delete_item(ctx: &Ctx, id: &str) -> Result<(), String> {
    let _lifecycle = ctx.item_lifecycle.lock().await;
    let Some(item) = ctx.item(id) else {
        return Err("That item is gone".into());
    };
    // Refuse before disturbing its work, then stop and fully reap every
    // owned process before either the checkout or its database rows go.
    if let Some(checkout) = item.checkout.as_ref().filter(|checkout| checkout.owned) {
        if checkout.path.join(".git").exists()
            && ctx
                .preparer
                .is_dirty(&checkout.path)
                .await
                .map_err(|error| error.to_string())?
        {
            return Err(format!("{} has uncommitted changes", checkout.branch));
        }
    }
    let _deletion = ctx.runs.block_owner(id, None)?;
    ctx.runs.cancel_owner(id, None).await;
    if let Some(checkout) = item.checkout.as_ref().filter(|c| c.owned) {
        if let Some(repo) = RepoId::parse(&item.repo_full_name) {
            let bare = ctx.preparer.store().bare_path(&repo);
            if checkout.path.join(".git").exists() {
                // Cancellation is asynchronous and the checkout is also a
                // normal directory the reader can edit. The first check
                // protects existing work; this one protects changes made
                // while Perch was waiting for the agent process to exit.
                if ctx
                    .preparer
                    .is_dirty(&checkout.path)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    return Err(format!("{} has uncommitted changes", checkout.branch));
                }
                if ctx.is_open(id) {
                    ctx.close_review().await;
                }
                ctx.preparer
                    .remove_worktree(&bare, &checkout.path)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            if bare.join("HEAD").is_file() {
                // A branch that is already gone is not a reason to keep the row.
                if let Err(e) = ctx.preparer.delete_branch(&bare, &checkout.branch).await {
                    tracing::info!(id, error = %e, "branch was not there to delete");
                }
            }
        }
    } else if ctx.is_open(id) {
        ctx.close_review().await;
    }

    // Review rows own retained snapshots outside the item's ordinary
    // worktree. Remove the durable history first so the database remains the
    // reference-counting truth, then release only resources no surviving
    // snapshot still names. Archive deliberately never takes this path.
    let deleted_reviews = ctx
        .with_cache(|cache| cache.delete_review_history(id))
        .transpose()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "the local database is unavailable".to_string())?;
    let retention_keys = deleted_reviews
        .snapshots
        .into_iter()
        .map(|snapshot| snapshot.retention_key)
        .collect::<std::collections::BTreeSet<_>>();
    for retention_key in retention_keys {
        let references = ctx
            .with_cache(|cache| cache.review_snapshot_reference_count(&retention_key))
            .transpose()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the local database is unavailable".to_string())?;
        if references == 0 {
            ctx.review_snapshots
                .release(&retention_key)
                .map_err(|error| error.to_string())?;
        }
    }

    ctx.with_cache(|c| c.delete_item(id))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(())
}

/// Open whatever an item is.
async fn open_item(ctx: &Ctx, id: &str, navigation_id: u64) -> bool {
    let _lifecycle = ctx.item_lifecycle.lock().await;
    if !ctx
        .navigation_requests
        .is_current_item(id, navigation_id)
    {
        return false;
    }
    let Some(item) = ctx.item(id) else {
        let _ = ctx.navigation_requests.with_current(navigation_id, || {
            let _ = ctx.events.send(Event::ReviewFailed {
                navigation_id,
                message: "That item is gone".into(),
            });
        });
        return false;
    };
    let source_checkout_missing = item
        .checkout
        .as_ref()
        .is_some_and(|checkout| !checkout.owned && !checkout.path.join(".git").exists());
    let has_retained_reviews = ctx
        .with_cache(|cache| {
            cache
                .list_review_groups(&item.id, None, 1)
                .map(|groups| !groups.is_empty())
        })
        .and_then(Result::ok)
        .unwrap_or(false);
    if source_checkout_missing && has_retained_reviews {
        show_retained_review_owner(ctx, item, navigation_id);
        return false;
    }
    match (&item.subject, &item.checkout) {
        (Some(ItemSubject::PullRequest(pr)), _) => {
            open_subject(
                ctx,
                pr.repo_full_name.clone(),
                ItemKind::PullRequest,
                pr.number,
                navigation_id,
            )
            .await
        }
        (Some(ItemSubject::Issue(issue)), _) => {
            open_subject(
                ctx,
                issue.repo_full_name.clone(),
                ItemKind::Issue,
                issue.number,
                navigation_id,
            )
            .await
        }
        (None, Some(_)) => open_branch_item(ctx, item, navigation_id).await,
        (None, None) => {
            let _ = ctx.navigation_requests.with_current(navigation_id, || {
                let _ = ctx.events.send(Event::ReviewFailed {
                    navigation_id,
                    message: "That item has nothing to open".into(),
                });
            });
            false
        }
    }
}

/// Open the durable owner shell even when its external source checkout has
/// disappeared. History can then switch the shell to any retained snapshot;
/// no source clone or branch ref is needed for that path.
fn show_retained_review_owner(ctx: &Ctx, item: Item, navigation_id: u64) -> bool {
    let latest_branch_label = ctx
        .with_cache(|cache| cache.list_review_snapshots(&item.id))
        .and_then(Result::ok)
        .and_then(|snapshots| {
            snapshots
                .into_iter()
                .find_map(|snapshot| snapshot.branch_label)
        });
    let missing = item
        .checkout
        .as_ref()
        .map(|checkout| checkout.path.clone())
        .unwrap_or_else(|| ctx.preparer.store().root().join("reviews").join("missing"));
    let checkout = perch_git::Checkout {
        worktree: missing.clone(),
        bare: missing,
        branch: latest_branch_label
            .or_else(|| {
                item.checkout
                    .as_ref()
                    .map(|checkout| checkout.branch.clone())
            })
            .unwrap_or_default(),
        // The source checkout is gone, so no retained review is implicitly
        // "current". History selection installs an exact snapshot/group;
        // until then finding threads stay outdated and off inline surfaces.
        merge_base: String::new(),
        head_sha: String::new(),
        stale: None,
    };
    let pull_request = item.pull_request().cloned().map(Arc::new);
    let item_id = item.id.clone();
    let scope_gen = next_scope_gen();
    let installed = ctx
        .navigation_requests
        .with_current_item(&item_id, navigation_id, || {
            let Ok(mut slot) = ctx.slot.lock() else {
                return false;
            };
            *slot = Some(ReviewContext {
                checkout,
                files: Arc::new(Vec::new()),
                scope: perch_git::Scope::Branch,
                since: None,
                scope_gen,
                navigation_id,
                item_id: item.id.clone(),
                pull_request: pull_request.clone(),
                body: None,
                issue: None,
                review_snapshot_id: None,
                review_group_id: None,
                review_request_id: None,
            });
            let _ = ctx.events.send(Event::ReviewReady {
                navigation_id,
                scope_gen,
                item: Arc::new(item),
                pull_request,
                files: Arc::new(Vec::new()),
                tree: Arc::new(Vec::new()),
                stale_head: None,
                offline: true,
            });
            true
        })
        .unwrap_or(false);
    if !installed {
        ctx.cancel_committed_archived_navigation(&item_id, navigation_id);
        return false;
    }
    ctx.notice("Source checkout unavailable; retained review history is still readable");
    ctx.send_threads(&item_id);
    true
}

/// A branch of the reader's own, with no pull request: the review view over
/// the diff against its base, wherever the checkout is.
async fn open_branch_item(ctx: &Ctx, item: Item, navigation_id: u64) -> bool {
    let Some(checkout) = item.checkout.clone() else {
        ctx.send_navigation_event(
            navigation_id,
            Event::ReviewFailed {
                navigation_id,
                message: "That item has no checkout".into(),
            },
        );
        return false;
    };
    let Some(repo) = RepoId::parse(&item.repo_full_name) else {
        ctx.send_navigation_event(
            navigation_id,
            Event::ReviewFailed {
                navigation_id,
                message: format!("{} is not a repository name", item.repo_full_name),
            },
        );
        return false;
    };
    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Checking out the files".into(),
            percent: None,
        },
    );

    let git_checkout = if checkout.owned {
        // A checkout under the store is cut in a clone from GitHub, which
        // a local repository has not got, so a row like this is from
        // before the entry was local. Said here rather than found out by
        // a clone that cannot start.
        if perch_github::is_local(&item.repo_full_name) {
            ctx.send_navigation_event(
                navigation_id,
                Event::ReviewFailed {
                    navigation_id,
                    message: format!(
                        "{} is not on GitHub, so there is no clone to put {} back in",
                        ctx.config.display_name(&item.repo_full_name),
                        checkout.branch
                    ),
                },
            );
            return false;
        }
        match open_owned_branch(ctx, &repo, &checkout).await {
            Ok(c) => c,
            Err(e) => {
                ctx.send_navigation_event(
                    navigation_id,
                    Event::ReviewFailed {
                        navigation_id,
                        message: e.to_string(),
                    },
                );
                return false;
            }
        }
    } else {
        match open_own_checkout(ctx, &repo, &checkout, &checkout.base_ref).await {
            Ok(c) => c,
            Err(e) => {
                ctx.send_navigation_event(
                    navigation_id,
                    Event::ReviewFailed {
                        navigation_id,
                        message: e.to_string(),
                    },
                );
                return false;
            }
        }
    };

    // The row as it is now: the git work took a while, and a refresh may
    // have attached a pull request or a poll marked it since.
    let mut item = ctx.item(&item.id).unwrap_or(item);
    let since = item
        .checkout
        .as_ref()
        .and_then(|c| c.last_seen_head.clone());
    if let Some(c) = item.checkout.as_mut() {
        // The scan moves head_sha with the worktree, so the previous
        // open's head is kept on its own, and replaced by this one's.
        c.last_seen_head = Some(git_checkout.head_sha.clone()).filter(|h| !h.is_empty());
        c.head_sha = git_checkout.head_sha.clone();
        c.on_disk = true;
    }
    item.archived_at = None;
    item.last_opened_at = Timestamp::now();
    ctx.with_cache(|c| c.update_item(&item));
    ctx.send_items();

    let pull_request = item.pull_request().cloned().map(Arc::new);
    finish_open(
        ctx,
        item,
        pull_request,
        git_checkout,
        false,
        since,
        navigation_id,
    )
    .await
}

/// A checkout under the store on a branch Perch cut: back on disk if it
/// was archived, then read as it is.
async fn open_owned_branch(
    ctx: &Ctx,
    repo: &RepoId,
    checkout: &ItemCheckout,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let bare = ctx.preparer.ensure_bare(repo, |_| {}).await?;
    if !checkout.path.join(".git").exists() {
        // Archived: the branch is still in the bare clone.
        let path = checkout.path.to_string_lossy().into_owned();
        let _ = ctx
            .preparer
            .git()
            .run(Some(&bare), ["worktree", "prune"], Duration::from_secs(30))
            .await;
        ctx.preparer
            .git()
            .run(
                Some(&bare),
                ["worktree", "add", &path, &checkout.branch],
                Duration::from_secs(60),
            )
            .await?;
    }
    let git = ctx.preparer.git();
    let head_sha = git
        .run(
            Some(&checkout.path),
            ["rev-parse", "HEAD"],
            Duration::from_secs(30),
        )
        .await?
        .trim()
        .to_string();
    // The base: a branch name resolves through the home ref the last
    // ensure_home fetched; a fork's base is the source's commit itself.
    let base = if checkout.base_ref.len() == 40
        && checkout.base_ref.chars().all(|c| c.is_ascii_hexdigit())
    {
        checkout.base_ref.clone()
    } else {
        perch_git::home_ref(&checkout.base_ref)
    };
    let merge_base = git
        .run(
            Some(&bare),
            ["merge-base", &base, &head_sha],
            Duration::from_secs(30),
        )
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| head_sha.clone());
    Ok(perch_git::Checkout {
        worktree: checkout.path.clone(),
        bare,
        branch: checkout.branch.clone(),
        head_sha,
        merge_base,
        stale: None,
    })
}

/// A checkout of the reader's own on a pull request's head branch, with
/// no subject yet, that the pull request can attach to.
fn branch_item_for(ctx: &Ctx, pr: &PullRequest) -> Option<Item> {
    let head_repo = pr.head_repo_full_name.as_deref()?;
    ctx.with_cache(|cache| {
        cache
            .find_item_by_branch(head_repo, &pr.head_ref)
            .ok()
            .flatten()
    })
    .flatten()
    .filter(|item| item.subject.is_none())
    .filter(|item| !item.is_archived() || item.checkout.as_ref().is_some_and(|c| c.owned))
}

fn stored_is_newer(stored: Option<Timestamp>, fetched: Option<Timestamp>) -> bool {
    matches!((stored, fetched), (Some(stored), Some(fetched)) if stored > fetched)
}

/// Write the row an open made or brought up to date. A row the open took
/// for new is looked for once more, under the lock it is written under:
/// the open found nothing, then fetched and cloned, and another open or a
/// local-branch attachment may have made a row for the same subject. Inserted
/// beside it, the open's row would be refused by the subject index and
/// the review would run under an id no row has, its threads and marks
/// written to nothing. When a row has appeared, what the open set goes
/// onto it, and its id is the one the review runs as.
fn record_open(cache: &Cache, item: &mut Item, is_new: bool) -> Result<(), CacheError> {
    if !is_new {
        return cache.update_item(item);
    }
    let found = match item.subject.as_ref() {
        Some(s) => cache.find_item_by_subject(s.repo_full_name(), s.kind(), s.number())?,
        None => None,
    };
    let Some(mut found) = found else {
        return cache.insert_item(item);
    };
    tracing::info!(id = %found.id, "another operation made the row while the open ran; opening as that row");
    // The stored subject may be the later one.
    let fetched = item.subject.take();
    let stored = found.subject.as_ref().and_then(ItemSubject::updated_at);
    if !stored_is_newer(stored, fetched.as_ref().and_then(ItemSubject::updated_at)) {
        found.subject = fetched;
    }
    found.checkout = item.checkout.take();
    found.archived_at = None;
    found.last_opened_at = item.last_opened_at;
    cache.update_item(&found)?;
    *item = found;
    Ok(())
}

/// The branches items already hold in a repository, which a cut must not
/// take: `items_branch` allows one row per branch.
fn branches_in(ctx: &Ctx, repo_full_name: &str) -> Vec<String> {
    ctx.with_cache(|c| c.load_items().unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.repo_full_name.eq_ignore_ascii_case(repo_full_name))
        .filter_map(|i| i.checkout.map(|c| c.branch))
        .collect()
}

/// Everything after a checkout is in hand: the file list over the branch,
/// the tree, the review context, `ReviewReady`, and then, each on its own
/// task so the code paints first, the threads, what is viewed, the
/// commits and the dirt for the scope menu, and for a pull request the
/// discussion and the checks: the cached copy first, then GitHub's.
async fn finish_open(
    ctx: &Ctx,
    item: Item,
    pull_request: Option<Arc<PullRequest>>,
    checkout: perch_git::Checkout,
    offline: bool,
    since: Option<String>,
    navigation_id: u64,
) -> bool {
    let events = &ctx.events;
    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Computing the diff".into(),
            percent: None,
        },
    );

    let scope = perch_git::Scope::Branch;
    let files = match scoped_files(&ctx.preparer, &checkout, &scope).await {
        Ok(f) => Arc::new(f),
        Err(e) => {
            ctx.send_navigation_event(
                navigation_id,
                Event::ReviewFailed {
                    navigation_id,
                    message: e.to_string(),
                },
            );
            return false;
        }
    };
    let worktree = checkout.worktree.clone();
    let tree = tokio::task::spawn_blocking(move || {
        perch_git::list_tree(&worktree, 50_000).unwrap_or_default()
    })
    .await
    .unwrap_or_default();

    let stale_head = checkout.stale.clone();
    let head = checkout.head_sha.clone();
    let merge_base = checkout.merge_base.clone();
    let worktree = checkout.worktree.clone();
    let item = Arc::new(item);
    let scope_gen = next_scope_gen();
    tracing::info!(
        item = %item.id,
        files = files.len(),
        tree = tree.len(),
        worktree = %worktree.display(),
        stale = stale_head.is_some(),
        offline,
        "review ready"
    );
    let installed = ctx
        .navigation_requests
        .with_current_item(&item.id, navigation_id, || {
            let Ok(mut guard) = ctx.slot.lock() else {
                return false;
            };
            *guard = Some(ReviewContext {
                checkout,
                files: files.clone(),
                scope,
                since: since.clone(),
                scope_gen,
                navigation_id,
                item_id: item.id.clone(),
                pull_request: pull_request.clone(),
                body: None,
                issue: None,
                review_snapshot_id: None,
                review_group_id: None,
                review_request_id: None,
            });
            let _ = events.send(Event::ReviewReady {
                navigation_id,
                scope_gen,
                item: item.clone(),
                pull_request: pull_request.clone(),
                files,
                tree: Arc::new(tree),
                stale_head,
                offline,
            });
            true
        })
        .unwrap_or(false);
    if !installed {
        ctx.cancel_committed_archived_navigation(&item.id, navigation_id);
        return false;
    }

    // What was viewed and revealed, and whether the files are folded, is
    // small and local; it goes straight away.
    send_viewed(ctx, &item.id, true);

    // The outdated check is a git call per moved thread; the log and the
    // status are two more. Off the open, on their own tasks.
    {
        let ctx = ctx.clone();
        let item_id = item.id.clone();
        let (head, worktree) = (head.clone(), worktree.clone());
        tokio::spawn(async move {
            mark_outdated_threads(&ctx, &item_id, &head, &worktree).await;
            ctx.send_threads(&item_id);
        });
    }
    {
        let ctx = ctx.clone();
        let item_id = item.id.clone();
        let head = head.clone();
        tokio::spawn(async move {
            send_commits(
                &ctx,
                &item_id,
                &worktree,
                &merge_base,
                &head,
                since,
                navigation_id,
                scope_gen,
                None,
            )
            .await
        });
    }

    if let Some(pr) = pull_request {
        // The cached discussion and checks paint before GitHub answers,
        // with the faces the last open left on disk and the description a
        // question needs. A checkout with no pull request has neither.
        let cached = ctx
            .with_cache(|c| c.load_discussion(&item.id).ok().flatten())
            .flatten();
        if let Some((_, d)) = cached {
            if let Ok(mut guard) = ctx.slot.lock() {
                if let Some(open) = guard.as_mut().filter(|c| c.item_id == item.id) {
                    open.body = Some(d.body.clone());
                }
            }
            ctx.want_avatars(d.comments.iter().map(|c| c.user_avatar_url.clone()));
            let _ = ctx.events.send(Event::DiscussionReady {
                item_id: item.id.clone(),
                discussion: Arc::new(d),
            });
        }
        // The author's face is on the header, whether or not they said
        // anything in the discussion.
        ctx.want_avatars(
            std::iter::once(pr.author_avatar_url.clone())
                .chain(pr.requested_reviewers.iter().map(|r| r.avatar_url.clone())),
        );
        let cached = ctx
            .with_cache(|c| c.load_checks(&item.id, &pr.head_sha).ok().flatten())
            .flatten();
        if let Some(checks) = cached {
            let _ = ctx.events.send(Event::ChecksReady {
                item_id: item.id.clone(),
                repo_full_name: pr.repo_full_name.clone(),
                number: pr.number,
                head_sha: pr.head_sha.clone(),
                status_generation: 0,
                checks: Arc::new(checks),
            });
        }
        if !offline {
            let (ctx2, item_id2, pr2) = (ctx.clone(), item.id.clone(), pr.clone());
            tokio::spawn(async move { fetch_discussion(&ctx2, &item_id2, &pr2).await });
            let (ctx2, item_id2) = (ctx.clone(), item.id.clone());
            let status_generation = begin_pull_request_status(&ctx2, &item_id2, &pr);
            tokio::spawn(async move {
                fetch_checks(&ctx2, &item_id2, &pr, status_generation).await
            });
        }
    }
    true
}

/// The changed list over a scope, in the worktree. Every scope produces
/// the same shape, and nothing downstream knows which made it.
async fn scoped_files(
    preparer: &Preparer,
    checkout: &perch_git::Checkout,
    scope: &perch_git::Scope,
) -> Result<Vec<perch_git::ChangedFile>, perch_git::Error> {
    perch_git::changed_files(
        preparer.git(),
        &checkout.worktree,
        scope,
        &checkout.merge_base,
        &checkout.head_sha,
    )
    .await
}

/// What the reader marked on an item's files: every (path, blob) viewed,
/// and on the open, every reveal and whether the files are folded, in one
/// pass over the cache. Absent, or with no cache, the files are folded:
/// that is the default, and the key is only ever written once the tile is
/// clicked. After a write only the viewed set is sent; see `ViewedReady`.
fn send_viewed(ctx: &Ctx, item_id: &str, opening: bool) {
    use crate::review::model::{folded_from_meta, folded_meta_key};
    let (viewed, folds) = ctx
        .with_cache(|c| {
            let viewed = c.load_viewed(item_id).unwrap_or_default();
            let folds = opening.then(|| {
                (
                    c.load_reveals(item_id).unwrap_or_default(),
                    folded_from_meta(c.meta(&folded_meta_key(item_id)).ok().flatten().as_deref()),
                )
            });
            (viewed, folds)
        })
        .unwrap_or_else(|| {
            (
                Default::default(),
                opening.then(|| (Default::default(), folded_from_meta(None))),
            )
        });
    let (reveals, folded) = folds.unzip();
    let _ = ctx.events.send(Event::ViewedReady {
        item_id: item_id.to_string(),
        viewed: Arc::new(viewed),
        reveals: reveals.map(Arc::new),
        folded,
    });
}

/// The commits over the base and whether there is work in the checkout,
/// for the scope menu, with the head as it was at the previous open. A
/// checkout with no history over its base, an issue's home say, sends an
/// empty list.
#[allow(
    clippy::too_many_arguments,
    reason = "the event fence needs both navigation and scope request identities"
)]
async fn send_commits(
    ctx: &Ctx,
    item_id: &str,
    worktree: &Path,
    merge_base: &str,
    head: &str,
    since: Option<String>,
    navigation_id: u64,
    scope_gen: u64,
    scope_revision: Option<u64>,
) {
    let git = ctx.preparer.git();
    let commits = if merge_base.is_empty() || merge_base == head {
        Vec::new()
    } else {
        match perch_git::log(git, worktree, merge_base, head).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(item = item_id, error = %e, "could not list the commits");
                Vec::new()
            }
        }
    };
    let dirt = perch_git::dirt(git, worktree).await.unwrap_or_default();
    let event = Event::CommitsReady {
        navigation_id,
        scope_gen,
        item_id: item_id.to_string(),
        commits: Arc::new(commits),
        dirt,
        since,
    };
    let publish = || {
        let matches = ctx.slot.lock().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|open| {
                open.item_id == item_id
                    && open.navigation_id == navigation_id
                    && open.scope_gen == scope_gen
                    && open.review_snapshot_id.is_none()
                    && open.review_group_id.is_none()
                    && open.review_request_id.is_none()
            })
        });
        if matches {
            let _ = ctx.events.send(event);
        }
        matches
    };
    match scope_revision {
        Some(revision) => {
            let _ = ctx.navigation_requests.with_current(navigation_id, || {
                ctx.scope_requests.with_revision(revision, publish)
            });
        }
        None => {
            let _ = ctx.navigation_requests.with_current(navigation_id, publish);
        }
    }
}

/// Look at the checkout again. The head may have moved under a terminal
/// commit: then the review context follows it and the changed list is
/// recomputed under the scope that is set; the commits and the dirt are
/// read again either way.
async fn refresh_scope(ctx: &Ctx, item_id: &str, navigation_id: u64, scope_revision: u64) {
    if !ctx.navigation_requests.is_current(navigation_id)
        || ctx.scope_requests.stable_revision() != Some(scope_revision)
    {
        return;
    }
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|c| {
                c.item_id == item_id
                    && c.navigation_id == navigation_id
                    && c.review_snapshot_id.is_none()
                    && c.review_group_id.is_none()
                    && c.review_request_id.is_none()
            })
            .map(|c| {
                (
                    c.checkout.clone(),
                    c.scope.clone(),
                    c.since.clone(),
                    c.scope_gen,
                )
            })
    });
    let Some((mut checkout, scope, since, gen)) = open else {
        return;
    };
    if !checkout.worktree.join(".git").exists() {
        return;
    }
    let git = ctx.preparer.git();
    let head = git
        .run(
            Some(&checkout.worktree),
            ["rev-parse", "HEAD"],
            Duration::from_secs(30),
        )
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    // A head that moved is a commit the reader made: the branch grew, or
    // was rebased, so the base is found again too. A pull request checkout
    // left stale by a dirty tree is not this.
    if !head.is_empty() && head != checkout.head_sha && checkout.stale.is_none() {
        checkout.head_sha = head.clone();
        if let Some(item) = ctx.item(item_id) {
            for base in base_revs_for(ctx, &item).await {
                if let Ok(mb) = git
                    .run(
                        Some(&checkout.worktree),
                        ["merge-base", &base, &head],
                        Duration::from_secs(30),
                    )
                    .await
                {
                    let mb = mb.trim().to_string();
                    if !mb.is_empty() {
                        checkout.merge_base = mb;
                        break;
                    }
                }
            }
        }
        let files = match scoped_files(&ctx.preparer, &checkout, &scope).await {
            Ok(f) => Arc::new(f),
            Err(e) => {
                tracing::info!(item = item_id, error = %e, "could not reread the changed list");
                return;
            }
        };
        // The reader may have picked a scope while this looked; theirs
        // wins, and the next look reads under it.
        let committed_gen = ctx
            .navigation_requests
            .with_current(navigation_id, || {
                ctx.scope_requests.with_revision(scope_revision, || {
                    let Ok(mut guard) = ctx.slot.lock() else {
                        return 0;
                    };
                    let Some(open) = guard.as_mut().filter(|open| {
                        open.item_id == item_id
                            && open.navigation_id == navigation_id
                            && open.scope_gen == gen
                            && open.review_snapshot_id.is_none()
                            && open.review_group_id.is_none()
                            && open.review_request_id.is_none()
                    }) else {
                        return 0;
                    };
                    open.checkout.head_sha = head.clone();
                    open.checkout.merge_base = checkout.merge_base.clone();
                    open.files = files.clone();
                    let committed_gen = next_scope_gen();
                    open.scope_gen = committed_gen;
                    let _ = ctx.events.send(Event::ScopeReady {
                        navigation_id,
                        scope_gen: committed_gen,
                        item_id: item_id.to_string(),
                        scope: scope.clone(),
                        files: files.clone(),
                    });
                    committed_gen
                })
            })
            .flatten()
            .filter(|generation| *generation != 0);
        let Some(committed_gen) = committed_gen else {
            return;
        };
        // The row's head is what the view derives anchors and the menu
        // from, so it follows too.
        if let Some(mut item) = ctx.item(item_id) {
            if let Some(c) = item.checkout.as_mut() {
                c.head_sha = head.clone();
            }
            ctx.with_cache(|c| c.update_item(&item));
            ctx.send_items();
        }
        send_commits(
            ctx,
            item_id,
            &checkout.worktree,
            &checkout.merge_base,
            &checkout.head_sha,
            since,
            navigation_id,
            committed_gen,
            Some(scope_revision),
        )
        .await;
        return;
    }
    send_commits(
        ctx,
        item_id,
        &checkout.worktree,
        &checkout.merge_base,
        &checkout.head_sha,
        since,
        navigation_id,
        gen,
        Some(scope_revision),
    )
    .await;
}

/// A number no earlier review context or scope pick has had, so a look at
/// the checkout that began under one cannot write into the next.
fn next_scope_gen() -> u64 {
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The revisions an item's diff may be taken against, as the opens find
/// them, best first: a root checkout's base as its remote has it, since
/// it is the reader's own clone and has none of Perch's refs; for a
/// checkout in the store, a pull request's fetched base, then the home
/// ref or the commit the branch was forked from. The first that resolves
/// is the one.
async fn base_revs_for(ctx: &Ctx, item: &Item) -> Vec<String> {
    let Some(checkout) = item.checkout.as_ref() else {
        return Vec::new();
    };
    let base = &checkout.base_ref;
    let is_sha = base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit());
    if !checkout.owned {
        return root_base_revs(ctx, &checkout.path, base).await;
    }
    let mut out = Vec::new();
    if let Some(pr) = item.pull_request() {
        out.push(format!("refs/perch/base/{}", pr.number));
    }
    out.push(if is_sha {
        base.clone()
    } else {
        perch_git::home_ref(base)
    });
    out
}

/// The revisions a root checkout's branch may be diffed against, best
/// first: the base as the remote that names the repository has it, which
/// is the remote the scan chose and counted the row's churn against, so
/// the review and the row agree on where the branch left; then as
/// `origin` has it, for a fork whose `upstream` has not been fetched;
/// then the branch itself. The remote is read from the checkout the way
/// the scan reads it, since the row carries the base's name and not the
/// remote's.
async fn root_base_revs(ctx: &Ctx, path: &Path, base: &str) -> Vec<String> {
    let hosts = ctx.config.github_hosts();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let remotes = perch_git::remotes(ctx.preparer.git(), path)
        .await
        .unwrap_or_default();
    let mut revs = Vec::new();
    if let Some((remote, _)) = perch_git::github_remote(&remotes, &hosts) {
        revs.push(format!("{remote}/{base}"));
    }
    let origin = format!("origin/{base}");
    if !revs.contains(&origin) {
        revs.push(origin);
    }
    revs.push(base.to_string());
    revs
}

/// One coherent GraphQL snapshot for detailed checks and merge readiness.
/// The checks are cached by head so they can paint immediately on the next
/// open. Mergeability and permissions stay live-only: a stale authorization
/// answer must never enable a remote mutation.
fn begin_pull_request_status(ctx: &Ctx, item_id: &str, pr: &PullRequest) -> u64 {
    let status_generation = ctx.status_requests.begin(item_id);
    if ctx.is_open(item_id) {
        let _ = ctx.events.send(Event::PullRequestOverviewLoading {
            item_id: item_id.to_string(),
            repo_full_name: pr.repo_full_name.clone(),
            number: pr.number,
            head_sha: pr.head_sha.clone(),
            status_generation,
        });
    }
    status_generation
}

async fn fetch_checks(
    ctx: &Ctx,
    item_id: &str,
    pr: &PullRequest,
    status_generation: u64,
) {
    let Some(github) = ctx.github.as_ref() else {
        return;
    };
    let head = pr.head_sha.as_str();
    let result = github
        .fetch_pull_request_overview(&pr.repo_full_name, pr.number)
        .await;
    if !ctx.status_requests.is_current(item_id, status_generation) {
        return;
    }
    match result {
        Ok(overview) if overview.head_sha == head => {
            ctx.with_cache(|c| c.store_checks(item_id, head, &overview.checks));
            if ctx.is_open(item_id) {
                let _ = ctx.events.send(Event::ChecksReady {
                    item_id: item_id.to_string(),
                    repo_full_name: pr.repo_full_name.clone(),
                    number: pr.number,
                    head_sha: head.to_string(),
                    status_generation,
                    checks: Arc::new(overview.checks.clone()),
                });
                let _ = ctx.events.send(Event::PullRequestOverviewReady {
                    item_id: item_id.to_string(),
                    repo_full_name: pr.repo_full_name.clone(),
                    number: pr.number,
                    status_generation,
                    overview: Arc::new(overview),
                });
            }
        }
        Ok(overview) => {
            let message = format!(
                "The pull request moved from {} to {}; refresh it before merging",
                short_sha(head),
                short_sha(&overview.head_sha)
            );
            tracing::info!(item = item_id, %message, "discarded stale pull request status");
            if ctx.is_open(item_id) {
                let _ = ctx.events.send(Event::PullRequestOverviewFailed {
                    item_id: item_id.to_string(),
                    repo_full_name: pr.repo_full_name.clone(),
                    number: pr.number,
                    head_sha: head.to_string(),
                    status_generation,
                    message,
                });
            }
        }
        Err(error) => {
            tracing::info!(item = item_id, error = %error, "could not fetch pull request status");
            if ctx.is_open(item_id) {
                let _ = ctx.events.send(Event::PullRequestOverviewFailed {
                    item_id: item_id.to_string(),
                    repo_full_name: pr.repo_full_name.clone(),
                    number: pr.number,
                    head_sha: head.to_string(),
                    status_generation,
                    message: format!("Could not refresh checks and merge status: {error}"),
                });
            }
        }
    }
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// Merge only the exact live snapshot the reader confirmed. The REST merge
/// endpoint remains the final authority, but these checks keep a stale view,
/// a historical review, or an accidental bypass from ever reaching it.
#[allow(
    clippy::too_many_arguments,
    reason = "the remote mutation carries every identity and consent fence explicitly"
)]
async fn merge_open_pull_request(
    ctx: &Ctx,
    attempt_id: u64,
    item_id: &str,
    repo_full_name: &str,
    number: u64,
    expected_head_sha: &str,
    method: MergeMethod,
    bypass_requirements: bool,
) {
    let open = ctx.slot.lock().ok().and_then(|slot| {
        slot.as_ref()
            .filter(|open| open.item_id == item_id)
            .and_then(|open| {
                open.pull_request.clone().map(|pull_request| {
                    (
                        pull_request,
                        open.checkout.head_sha.clone(),
                        open.review_snapshot_id.is_some()
                            || open.review_group_id.is_some()
                            || open.review_request_id.is_some(),
                    )
                })
            })
    });
    let Some((pull_request, checkout_head_sha, historical)) = open else {
        send_merge_failure(
            ctx,
            attempt_id,
            item_id,
            repo_full_name,
            number,
            expected_head_sha,
            "That pull request is no longer open in Perch",
        );
        return;
    };
    if pull_request.repo_full_name != repo_full_name || pull_request.number != number {
        send_merge_failure(
            ctx,
            attempt_id,
            item_id,
            repo_full_name,
            number,
            expected_head_sha,
            "The open pull request changed; review it again before merging",
        );
        return;
    }
    let Some(github) = ctx.github.as_ref() else {
        send_merge_failure(
            ctx,
            attempt_id,
            item_id,
            repo_full_name,
            number,
            expected_head_sha,
            "GitHub is unavailable while Perch is offline",
        );
        return;
    };

    let overview = match github
        .fetch_pull_request_overview(repo_full_name, number)
        .await
    {
        Ok(overview) => overview,
        Err(error) => {
            send_merge_failure(
                ctx,
                attempt_id,
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                format!("Could not refresh merge status: {error}"),
            );
            return;
        }
    };
    if let Err(message) = validate_merge_attempt(
        &pull_request,
        &checkout_head_sha,
        historical,
        &overview,
        expected_head_sha,
        method,
        bypass_requirements,
    ) {
        send_merge_failure(
            ctx,
            attempt_id,
            item_id,
            repo_full_name,
            number,
            expected_head_sha,
            message,
        );
        return;
    }

    let result = match github
        .merge_pull_request(repo_full_name, number, expected_head_sha, method)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            send_merge_failure(
                ctx,
                attempt_id,
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                format!("GitHub could not merge this pull request: {error}"),
            );
            return;
        }
    };
    if !result.merged {
        let message = if result.message.trim().is_empty() {
            "GitHub did not merge this pull request".to_string()
        } else {
            result.message
        };
        send_merge_failure(
            ctx,
            attempt_id,
            item_id,
            repo_full_name,
            number,
            expected_head_sha,
            message,
        );
        return;
    }

    // A successful mutation must never be reported as failed merely because
    // the follow-up GET had a transient problem. Synthesize the closed state
    // from the confirmed snapshot, then replace it with GitHub's richer answer
    // whenever that answer arrives.
    let mut merged = match github.fetch_pull_request(repo_full_name, number).await {
        Ok(pull_request) => pull_request,
        Err(error) => {
            tracing::info!(
                item = item_id,
                error = %error,
                "merged the pull request but could not refresh its snapshot"
            );
            (*pull_request).clone()
        }
    };
    if !merged.merged {
        let now = Timestamp::now();
        merged.merged = true;
        merged.state = "closed".into();
        merged.merged_at = Some(now);
        merged.closed_at = Some(now);
        merged.updated_at = Some(now);
    }

    let installed = install_pull_request_snapshot(ctx, item_id, merged, Some(attempt_id));
    ctx.notice(format!(
        "Merged {repo_full_name}#{number} ({})",
        method.as_str()
    ));
    if let Some((_, pull_request)) = installed {
        fetch_discussion(ctx, item_id, &pull_request).await;
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_merge_attempt(
    pull_request: &PullRequest,
    checkout_head_sha: &str,
    historical: bool,
    overview: &PullRequestOverview,
    expected_head_sha: &str,
    method: MergeMethod,
    bypass_requirements: bool,
) -> Result<(), String> {
    if historical {
        return Err("Return to the current review before merging".into());
    }
    if pull_request.merged || !pull_request.is_open() {
        return Err("This pull request is no longer open".into());
    }
    if pull_request.draft {
        return Err("Draft pull requests cannot be merged".into());
    }
    if pull_request.head_sha != expected_head_sha
        || overview.head_sha != expected_head_sha
        || checkout_head_sha != expected_head_sha
    {
        return Err("The pull request moved; refresh and review the new head before merging".into());
    }
    if !overview.allowed_merge_methods.allows(method) {
        return Err(format!(
            "The {} merge method is disabled for this repository",
            method.as_str()
        ));
    }
    let can_write = matches!(
        overview.viewer_permission,
        Some(
            RepositoryPermission::Write
                | RepositoryPermission::Maintain
                | RepositoryPermission::Admin
        )
    );
    if !can_write && !overview.viewer_can_merge_as_admin {
        return Err("You do not have permission to merge this pull request".into());
    }
    if bypass_requirements && !overview.viewer_can_merge_as_admin {
        return Err("You do not have permission to bypass branch requirements".into());
    }
    match overview.mergeable {
        MergeableState::Conflicting => {
            return Err("This pull request has conflicts that must be resolved on GitHub".into())
        }
        MergeableState::Unknown => {
            return Err("GitHub is still calculating whether this pull request can merge".into())
        }
        MergeableState::Mergeable => {}
    }
    match overview.merge_state {
        MergeState::Dirty => {
            Err("This pull request has conflicts that must be resolved on GitHub".into())
        }
        MergeState::Draft => Err("Draft pull requests cannot be merged".into()),
        MergeState::Unknown => {
            Err("GitHub is still calculating whether this pull request can merge".into())
        }
        MergeState::Blocked if !bypass_requirements => {
            Err("Merging is blocked until the repository requirements are met".into())
        }
        MergeState::Blocked if !overview.viewer_can_merge_as_admin => {
            Err("You do not have permission to bypass branch requirements".into())
        }
        MergeState::Behind
        | MergeState::Blocked
        | MergeState::Clean
        | MergeState::HasHooks
        | MergeState::Unstable => Ok(()),
    }
}

fn send_merge_failure(
    ctx: &Ctx,
    attempt_id: u64,
    item_id: &str,
    repo_full_name: &str,
    number: u64,
    expected_head_sha: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    tracing::info!(item = item_id, %message, "pull request merge stopped");
    if ctx.is_open(item_id) {
        let _ = ctx.events.send(Event::PullRequestMergeFailed {
            item_id: item_id.to_string(),
            repo_full_name: repo_full_name.to_string(),
            number,
            expected_head_sha: expected_head_sha.to_string(),
            attempt_id,
            message,
        });
    }
}

/// Keep the durable row, open backend slot, sidebar and review signals on the
/// same REST snapshot. A concurrently stored newer snapshot always wins.
fn install_pull_request_snapshot(
    ctx: &Ctx,
    item_id: &str,
    fetched: PullRequest,
    merge_attempt_id: Option<u64>,
) -> Option<(Arc<Item>, Arc<PullRequest>)> {
    let mut item = ctx.item(item_id)?;
    let current = item.pull_request()?;
    if current.repo_full_name != fetched.repo_full_name || current.number != fetched.number {
        tracing::info!(
            item = item_id,
            fetched_repo = %fetched.repo_full_name,
            fetched_number = fetched.number,
            "discarded a pull request refresh for a subject this item no longer owns"
        );
        return None;
    }
    let stored = item.subject.as_ref().and_then(ItemSubject::updated_at);
    let pull_request = if merge_attempt_id.is_none() && stored_is_newer(stored, fetched.updated_at) {
        item.pull_request()?.clone()
    } else {
        item.subject = Some(ItemSubject::PullRequest(fetched.clone()));
        if let Some(Err(error)) = ctx.with_cache(|cache| cache.update_item(&item)) {
            tracing::warn!(item = item_id, error = %error, "could not store pull request refresh");
        }
        fetched
    };
    let item = Arc::new(item);
    let pull_request = Arc::new(pull_request);
    let installed = ctx.slot.lock().ok().is_some_and(|mut slot| {
        let Some(open) = slot.as_mut().filter(|open| {
            open.item_id == item_id
                && open.pull_request.as_ref().is_some_and(|current| {
                    current.repo_full_name == pull_request.repo_full_name
                        && current.number == pull_request.number
                })
        }) else {
            return false;
        };
        open.pull_request = Some(pull_request.clone());
        true
    });
    ctx.send_items();
    if installed {
        let _ = ctx.events.send(Event::PullRequestUpdated {
            item_id: item_id.to_string(),
            item: item.clone(),
            pull_request: pull_request.clone(),
            merge_attempt_id,
        });
    }
    Some((item, pull_request))
}

/// The scope changed: the changed list is recomputed under it, kept in the
/// review context for the file loads that follow, and sent.
async fn set_scope(
    ctx: &Ctx,
    item_id: &str,
    scope: perch_git::Scope,
    request_id: u64,
    navigation_id: u64,
) {
    // A checkout refresh can discover a moved HEAD while this Git query is
    // running. Retry against the new context rather than pairing its new
    // revision identity with files computed from the old one.
    for _ in 0..3 {
        if !ctx.navigation_requests.is_current(navigation_id)
            || !ctx.scope_requests.is_current(item_id, request_id)
        {
            return;
        }
        let source = ctx.slot.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .filter(|open| open.item_id == item_id && open.navigation_id == navigation_id)
                .map(|open| (open.checkout.clone(), open.scope_gen))
        });
        let Some((checkout, source_gen)) = source else {
            ctx.scope_requests.settle(item_id, request_id);
            return;
        };
        let files = match scoped_files(&ctx.preparer, &checkout, &scope).await {
            Ok(files) => Arc::new(files),
            Err(error) => {
                let _ = ctx.navigation_requests.with_current(navigation_id, || {
                    ctx.scope_requests.with_current(item_id, request_id, || {
                        ctx.notice(format!("Could not read that scope: {error}"));
                    })
                });
                ctx.scope_requests.settle(item_id, request_id);
                return;
            }
        };
        let committed = ctx
            .navigation_requests
            .with_current(navigation_id, || {
                ctx.scope_requests.with_current(item_id, request_id, || {
                    let committed_gen = if let Ok(mut guard) = ctx.slot.lock() {
                        if let Some(open) = guard.as_mut().filter(|open| {
                            open.item_id == item_id
                                && open.navigation_id == navigation_id
                                && open.scope_gen == source_gen
                        }) {
                            open.scope = scope.clone();
                            open.files = files.clone();
                            let committed_gen = next_scope_gen();
                            open.scope_gen = committed_gen;
                            Some(committed_gen)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(scope_gen) = committed_gen {
                        let _ = ctx.events.send(Event::ScopeReady {
                            navigation_id,
                            scope_gen,
                            item_id: item_id.to_string(),
                            scope: scope.clone(),
                            files: files.clone(),
                        });
                        ctx.send_threads(item_id);
                    }
                    committed_gen.is_some()
                })
            })
            .flatten();
        match committed {
            Some(true) => {
                ctx.scope_requests.settle(item_id, request_id);
                return;
            }
            Some(false) => continue,
            None => {
                ctx.scope_requests.settle(item_id, request_id);
                return;
            }
        }
    }
    let _ = ctx.navigation_requests.with_current(navigation_id, || {
        ctx.scope_requests.with_current(item_id, request_id, || {
            ctx.notice("The checkout changed while switching scope; try again");
        })
    });
    ctx.scope_requests.settle(item_id, request_id);
}

/// Why a branch cannot be cut for a local repository, or None when it can.
/// A cut is made in the store's clone of the repository on GitHub, from
/// its home checkout; a repository that lives only on this machine has
/// neither, and the clone that would fail is one that must not be tried.
fn no_home_for_local(ctx: &Ctx, repo_full_name: &str) -> Option<String> {
    perch_github::is_local(repo_full_name).then(|| {
        format!(
            "{} is not on GitHub, so there is no home checkout to cut from",
            ctx.config.display_name(repo_full_name)
        )
    })
}

/// Cut `perch/issue-<n>` from the default branch and make the item for it,
/// or find the one that already has that branch.
async fn start_work(ctx: &Ctx, issue_item_id: &str, navigation_id: u64) -> Result<String, String> {
    let Some(issue_item) = ctx.item(issue_item_id) else {
        return Err("That item is gone".into());
    };
    let Some(issue) = issue_item.issue() else {
        return Err("Start work needs an issue".into());
    };
    let Some(repo) = RepoId::parse(&issue_item.repo_full_name) else {
        return Err(format!(
            "{} is not a repository name",
            issue_item.repo_full_name
        ));
    };
    if let Some(message) = no_home_for_local(ctx, &issue_item.repo_full_name) {
        return Err(message);
    }
    let wanted = format!("perch/issue-{}", issue.number);
    if let Some(existing) = ctx
        .with_cache(|c| {
            c.find_item_by_branch(&issue_item.repo_full_name, &wanted)
                .ok()
                .flatten()
        })
        .flatten()
    {
        return Ok(existing.id);
    }

    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Fetching the default branch".into(),
            percent: None,
        },
    );
    let progress = ctx.events.clone();
    let progress_requests = ctx.navigation_requests.clone();
    let label = repo.full_name();
    ctx.preparer
        .ensure_bare(&repo, move |step| {
            if let perch_git::Step::Cloning { phase, percent } = step {
                let _ = progress_requests.with_current(navigation_id, || {
                    let _ = progress.send(Event::ReviewProgress {
                        navigation_id,
                        message: format!("Cloning {label}: {phase}"),
                        percent,
                    });
                });
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    let default = ctx
        .preparer
        .default_branch(&repo)
        .await
        .map_err(|e| e.to_string())?;
    ctx.preparer
        .ensure_home(&repo, &default)
        .await
        .map_err(|e| e.to_string())?;
    let cut = ctx
        .preparer
        .cut_branch(
            &repo,
            &wanted,
            &perch_git::home_ref(&default),
            &branches_in(ctx, &issue_item.repo_full_name),
        )
        .await
        .map_err(|e| e.to_string())?;
    let head_sha = ctx
        .preparer
        .git()
        .run(
            Some(&cut.worktree),
            ["rev-parse", "HEAD"],
            Duration::from_secs(30),
        )
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let now = Timestamp::now();
    let mut item = Item::new(&issue_item.repo_full_name, now);
    item.checkout = Some(ItemCheckout {
        branch: cut.branch,
        base_ref: default,
        head_sha,
        path: cut.worktree,
        owned: true,
        on_disk: true,
        churn: Some((0, 0)),
        last_seen_head: None,
    });
    item.from_item_id = Some(issue_item.id.clone());
    ctx.with_cache(|c| c.insert_item(&item))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(item.id)
}

/// Cut a new branch from an item's head into a new checkout-only item.
async fn fork_item(ctx: &Ctx, id: &str) -> Result<String, String> {
    let Some(source) = ctx.item(id) else {
        return Err("That item is gone".into());
    };
    if source.is_archived() {
        return Err("Restore the item before forking it".into());
    }
    let Some(checkout) = source.checkout.as_ref() else {
        return Err("Fork needs a checkout; open the pull request first".into());
    };
    let Some(repo) = RepoId::parse(&source.repo_full_name) else {
        return Err(format!(
            "{} is not a repository name",
            source.repo_full_name
        ));
    };
    if let Some(message) = no_home_for_local(ctx, &source.repo_full_name) {
        return Err(message);
    }
    ctx.preparer
        .ensure_bare(&repo, |_| {})
        .await
        .map_err(|e| e.to_string())?;
    // A root's branch may not be in the store's clone; fetch its head
    // there by commit so the cut has something to start from.
    let head = if checkout.owned {
        checkout.head_sha.clone()
    } else {
        let git = ctx.preparer.git();
        // The root may have been switched since the row was made; a fork
        // of another branch under this one's name would be a lie.
        let on_branch = git
            .run(
                Some(&checkout.path),
                ["rev-parse", "--abbrev-ref", "HEAD"],
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| e.to_string())?;
        if on_branch.trim() != checkout.branch {
            return Err(format!(
                "{} has {} checked out now, not {}",
                checkout.path.display(),
                on_branch.trim(),
                checkout.branch
            ));
        }
        let sha = git
            .run(
                Some(&checkout.path),
                ["rev-parse", "HEAD"],
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| e.to_string())?
            .trim()
            .to_string();
        let bare = ctx.preparer.store().bare_path(&repo);
        let from = checkout.path.to_string_lossy().into_owned();
        git.run(
            Some(&bare),
            ["fetch", "--no-tags", &from, &sha],
            Duration::from_secs(120),
        )
        .await
        .map_err(|e| e.to_string())?;
        sha
    };
    // The source's own name is spoken for by the source; the cut takes
    // the next one, whether or not this clone has heard of it.
    let cut = ctx
        .preparer
        .cut_branch(
            &repo,
            &checkout.branch,
            &head,
            &branches_in(ctx, &source.repo_full_name),
        )
        .await
        .map_err(|e| e.to_string())?;

    let now = Timestamp::now();
    let mut item = Item::new(&source.repo_full_name, now);
    item.checkout = Some(ItemCheckout {
        branch: cut.branch,
        // The source's head, so what is shown is what has been done on top.
        base_ref: head.clone(),
        head_sha: head,
        path: cut.worktree,
        owned: true,
        on_disk: true,
        churn: Some((0, 0)),
        last_seen_head: None,
    });
    item.from_item_id = Some(source.id.clone());
    ctx.with_cache(|c| c.insert_item(&item))
        .transpose()
        .map_err(|e| e.to_string())?;
    ctx.send_items();
    Ok(item.id)
}

/// A file larger than this opens as a notice rather than as text.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// What an open review needs in order to read and diff its files.
struct ReviewContext {
    checkout: perch_git::Checkout,
    /// The changed list under `scope`.
    files: Arc<Vec<perch_git::ChangedFile>>,
    /// What the diff is over. Branch when the item opens.
    scope: perch_git::Scope,
    /// The head at the previous open, for "Since you last looked".
    since: Option<String>,
    /// Rises with every scope pick, so a look at the checkout that began
    /// under an earlier scope does not write its list over the reader's.
    scope_gen: u64,
    /// The synchronous navigation which installed this context. Background
    /// refreshes must match it as well as the item id.
    navigation_id: u64,
    item_id: String,
    /// `None` for a branch with no pull request.
    pull_request: Option<Arc<PullRequest>>,
    /// The description, once the discussion has been fetched.
    body: Option<String>,
    /// The issue and its comments, for a question about an issue.
    issue: Option<Arc<(perch_github::Issue, Vec<perch_github::IssueComment>)>>,
    /// Exact retained snapshot shown by History. Normal live views leave
    /// this empty and may only use an exact range match as a projection
    /// target.
    review_snapshot_id: Option<String>,
    /// The selected historical group. This disambiguates repeated reviews
    /// of an unchanged range and limits History to that group's observations.
    review_group_id: Option<String>,
    /// The exact History request which installed this group. Paired with the
    /// group id on projected thread events so a repeated/out-of-order load
    /// cannot overwrite a newer selection.
    review_request_id: Option<u64>,
}

/// The server's minimum delay for a manual refresh and the (possibly longer)
/// configured delay before the next automatic refresh.
struct PollAfter {
    automatic: Duration,
    manual: Duration,
    /// Whether GitHub returned a usable response. A requested full
    /// reconciliation remains pending after an error.
    completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationPollMode {
    Conditional,
    Reconcile,
}

fn queue_manual_refresh(
    next_auto_poll: &mut tokio::time::Instant,
    next_allowed_poll: tokio::time::Instant,
) {
    if next_allowed_poll < *next_auto_poll {
        *next_auto_poll = next_allowed_poll;
    }
}

fn defer_polls_for_error(
    error: &perch_github::Error,
    next_auto_poll: &mut tokio::time::Instant,
    next_allowed_poll: &mut tokio::time::Instant,
) {
    let perch_github::Error::RateLimited {
        reset_epoch,
        retry_after_secs,
    } = error
    else {
        return;
    };
    let deadline = tokio::time::Instant::now() + backoff_until(*reset_epoch, *retry_after_secs);
    *next_auto_poll = (*next_auto_poll).max(deadline);
    *next_allowed_poll = (*next_allowed_poll).max(deadline);
}

/// One poll of the notifications API.
async fn refresh(ctx: &Ctx, source: &str, mode: NotificationPollMode) -> PollAfter {
    let events = &ctx.events;
    let config = ctx.config.as_ref();
    let Some(github) = ctx.github.as_ref() else {
        let wait = Duration::from_secs(config.poll_interval_secs.max(30));
        let _ = events.send(Event::Status(Status::NoToken {
            detail: "Set GITHUB_TOKEN, or run `gh auth login` in a terminal.".into(),
        }));
        return PollAfter {
            automatic: wait,
            manual: wait,
            completed: false,
        };
    };
    let _ = events.send(Event::Status(Status::Refreshing));

    let last_modified = (mode == NotificationPollMode::Conditional)
        .then(|| ctx.notifications_last_modified())
        .flatten();
    match github.list_threads_since(last_modified.as_deref()).await {
        Ok(result) => {
            let manual = result.poll_interval;
            let automatic = manual.max(Duration::from_secs(config.poll_interval_secs.max(1)));
            if result.unchanged {
                tracing::info!("notifications unchanged");
            } else {
                tracing::info!(count = result.threads.len(), "notifications polled");
                let visible =
                    ctx.replace_notifications(result.threads, result.last_modified.as_deref());
                attend(ctx, &visible).await;
            }

            let _ = events.send(Event::Status(Status::Idle {
                at: Timestamp::now(),
                source: source.to_string(),
            }));
            PollAfter {
                automatic,
                manual,
                completed: true,
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "list_threads failed");
            let status = describe_error(&e);
            let wait = if let perch_github::Error::RateLimited {
                reset_epoch,
                retry_after_secs,
            } = &e
            {
                backoff_until(*reset_epoch, *retry_after_secs)
            } else {
                Duration::from_secs(config.poll_interval_secs.max(30))
            };
            let _ = events.send(Event::Status(status));
            PollAfter {
                automatic: wait,
                manual: wait,
                completed: false,
            }
        }
    }
}

/// Refresh content already open in Perch when GitHub reports activity on it.
/// Inbox membership and read state belong to `Thread`, never to `Item`.
async fn attend(ctx: &Ctx, threads: &[Thread]) {
    for thread in threads {
        if !thread.unread {
            continue;
        }
        // A CI notification for the open pull request's branch is the one
        // time the checks are asked about again.
        if thread.subject.kind == SubjectKind::CheckSuite {
            refresh_open_checks(ctx, thread).await;
            continue;
        }
        let kind = match thread.subject.kind {
            SubjectKind::PullRequest => ItemKind::PullRequest,
            SubjectKind::Issue => ItemKind::Issue,
            _ => continue,
        };
        let Some(number) = thread.number_hint() else {
            continue;
        };
        let repo = thread.repository.full_name.as_str();
        let item = ctx
            .with_cache(|c| c.find_item_by_subject(repo, kind, number).ok().flatten())
            .flatten()
            .filter(|i| !i.is_archived());
        let Some(item) = item else { continue };

        // Something was said about the item that is open: its Conversation
        // follows, whatever the reason. Once per notification, as with CI.
        if ctx.is_open(&item.id) {
            let key = (thread.id.clone(), thread.updated_at);
            let fresh = ctx
                .seen_notifications
                .lock()
                .map(|mut s| s.insert(key))
                .unwrap_or(false);
            if fresh {
                let ctx = ctx.clone();
                let item_id = item.id.clone();
                tokio::spawn(async move { refresh_open_item(&ctx, &item_id).await });
            }
        }
    }
}

/// Fetch again what the open item shows from GitHub: the pull request and
/// its discussion and checks, or the issue's comments.
async fn refresh_open_item(ctx: &Ctx, item_id: &str) {
    // One at a time per item: two notifications in one poll would fetch
    // and archive the same discussion twice, side by side.
    let taken = ctx
        .refreshing
        .lock()
        .map(|mut s| s.insert(item_id.to_string()))
        .unwrap_or(false);
    if !taken {
        return;
    }
    refresh_open_item_inner(ctx, item_id).await;
    if let Ok(mut s) = ctx.refreshing.lock() {
        s.remove(item_id);
    }
}

async fn refresh_open_item_inner(ctx: &Ctx, item_id: &str) {
    let Some(github) = ctx.github.as_ref() else {
        return;
    };
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|c| c.item_id == item_id)
            .map(|c| (c.pull_request.clone(), c.issue.clone()))
    });
    let Some((pull_request, issue)) = open else {
        return;
    };
    if let Some(pr) = pull_request {
        // The snapshot first, so the checks are for the head GitHub ran.
        let pr = match github
            .fetch_pull_request(&pr.repo_full_name, pr.number)
            .await
        {
            Ok(fresh) => {
                let Some((_, pull_request)) =
                    install_pull_request_snapshot(ctx, item_id, fresh, None)
                else {
                    return;
                };
                pull_request
            }
            Err(e) => {
                tracing::info!(item = item_id, error = %e, "could not fetch the pull request again");
                pr
            }
        };
        let still_current = ctx.slot.lock().ok().is_some_and(|slot| {
            slot.as_ref()
                .filter(|open| open.item_id == item_id)
                .and_then(|open| open.pull_request.as_ref())
                .is_some_and(|current| {
                    current.repo_full_name == pr.repo_full_name
                        && current.number == pr.number
                        && current.head_sha == pr.head_sha
                })
        });
        if !still_current {
            return;
        }
        let status_generation = begin_pull_request_status(ctx, item_id, &pr);
        tokio::join!(
            fetch_discussion(ctx, item_id, &pr),
            fetch_checks(ctx, item_id, &pr, status_generation),
        );
    } else if let Some(issue) = issue {
        let (issue, _) = &*issue;
        match github
            .fetch_issue_comments(&issue.repo_full_name, issue.number)
            .await
        {
            Ok(comments) => {
                keep_issue_comments(ctx, item_id, issue, &comments);
                // A question asked next is prompted with these.
                if let Ok(mut guard) = ctx.slot.lock() {
                    if let Some(open) = guard.as_mut().filter(|c| c.item_id == item_id) {
                        open.issue = Some(Arc::new((issue.clone(), comments.clone())));
                    }
                }
                if ctx.is_open(item_id) {
                    ctx.want_avatars(comments.iter().map(|c| c.user_avatar_url.clone()));
                    let _ = ctx.events.send(Event::IssueComments {
                        item_id: item_id.to_string(),
                        comments: Arc::new(comments),
                    });
                }
            }
            Err(e) => {
                tracing::info!(item = item_id, error = %e, "could not fetch the comments again")
            }
        }
    }
}

/// An issue's comments are kept the way a pull request's discussion is,
/// in the same table, so the next open paints them before the network
/// answers and the harness can print them.
fn keep_issue_comments(
    ctx: &Ctx,
    item_id: &str,
    issue: &perch_github::Issue,
    comments: &[perch_github::IssueComment],
) {
    ctx.with_cache(|c| c.store_discussion(item_id, &issue_discussion(issue, comments)));
}

/// An issue and its comments in the shape a pull request's discussion
/// has, which is the one shape the table keeps.
pub(super) fn issue_discussion(
    issue: &perch_github::Issue,
    comments: &[perch_github::IssueComment],
) -> perch_github::Discussion {
    perch_github::Discussion {
        opened_at: issue.created_at,
        body: issue.body.clone(),
        comments: comments.to_vec(),
        reviews: Vec::new(),
        review_comments: Vec::new(),
        events: Vec::new(),
    }
}

/// Fetch the checks again when a CI notification names the open pull
/// request's repository and head branch. A run on any other branch waits
/// until the reader opens it.
async fn refresh_open_checks(ctx: &Ctx, thread: &Thread) {
    let Some(query) = perch_github::WorkflowRunQuery::from_thread(thread) else {
        return;
    };
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref()
            .and_then(|c| c.pull_request.clone().map(|p| (c.item_id.clone(), p)))
    });
    let Some((item_id, pr)) = open else { return };
    if !(pr
        .repo_full_name
        .eq_ignore_ascii_case(&query.repo_full_name)
        && pr.head_ref == query.branch)
    {
        return;
    }
    // The snapshot is refetched on the way, so the checks are for the head
    // the run was on, not the one the item opened at. Once per
    // notification, not once per poll GitHub still returns it, and off
    // the poll, which the command loop waits on.
    let key = (thread.id.clone(), thread.updated_at);
    if ctx
        .seen_notifications
        .lock()
        .map(|mut s| !s.insert(key))
        .unwrap_or(true)
    {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move { refresh_open_item(&ctx, &item_id).await });
}

/// Rescan roots, then ask GitHub about subject-less branches.
async fn refresh_items(ctx: &Ctx) {
    if scan_roots(ctx).await {
        ctx.send_items();
    }
    if attach_pull_requests(ctx).await {
        ctx.send_items();
    }
}

/// Every worktree under a declared repository's roots is an item, and one
/// whose directory has gone is deleted, or archived when it has threads.
/// Returns whether anything changed.
async fn scan_roots(ctx: &Ctx) -> bool {
    let git = ctx.preparer.git();
    let mut changed = false;
    // Which unowned items were seen, by directory and branch, so the rest
    // can be judged gone. A root that could not be scanned takes its
    // repository out of the judgement: its worktrees are not in any other
    // root's listing when it is a clone of its own, and which repository
    // it was for is exactly what the scan would have said.
    let mut seen: std::collections::HashSet<(std::path::PathBuf, String)> =
        std::collections::HashSet::new();
    let mut scanned_repos: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut failed_repos: std::collections::HashSet<String> = std::collections::HashSet::new();
    let hosts = ctx.config.github_hosts();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();

    for declared in &ctx.config.repo {
        // Every entry has its identity by the time the backend runs. One
        // without would have nothing to key its rows by, so it is passed
        // over rather than guessed at.
        let Some(identity) = declared.identity() else {
            continue;
        };
        let local = perch_github::is_local(identity);
        let name = declared.display_name();
        for root in declared.root_paths() {
            let scan =
                match perch_git::scan_root(git, &root, declared.base.as_deref(), &hosts).await {
                    Ok(s) => s,
                    Err(e) => {
                        ctx.notice(format!("{}: {e}", root.display()));
                        failed_repos.insert(identity.to_lowercase());
                        continue;
                    }
                };
            // A root belongs to the one repository its remote names, and a
            // local repository's roots have no GitHub remote at all. A
            // root that says otherwise is left out of the entry rather
            // than filed under a repository it is not.
            match (scan.github.as_deref(), local) {
                (None, false) => {
                    ctx.notice(format!("{}: no GitHub remote for {name}", root.display()));
                    continue;
                }
                (Some(slug), false) if !slug.eq_ignore_ascii_case(identity) => {
                    ctx.notice(format!(
                        "{}: its remote is {slug}, not {identity}",
                        root.display()
                    ));
                    continue;
                }
                (Some(slug), true) => {
                    ctx.notice(format!(
                        "{}: its remote is {slug}, but {name} is local",
                        root.display()
                    ));
                    continue;
                }
                _ => {}
            }
            scanned_repos.insert(identity.to_lowercase());
            // `<remote>/<base>` for a GitHub root, since the branch left the
            // base as the remote has it; `<base>` itself for a local one,
            // which has no remote to have it, and for a GitHub root whose
            // remote has not been fetched yet.
            let base_ref = scan.base_ref.as_str();
            for entry in &scan.entries {
                let Some(branch) = entry.branch.clone() else {
                    continue;
                };
                if branch == scan.base || entry.prunable || !entry.path.join(".git").exists() {
                    continue;
                }
                let path = entry
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| entry.path.clone());
                seen.insert((path.clone(), branch.clone()));
                let churn = perch_git::branch_churn(git, &entry.path, base_ref)
                    .await
                    .ok();
                let existing = ctx
                    .with_cache(|c| c.find_item_by_branch(identity, &branch).ok().flatten())
                    .flatten();
                match existing {
                    Some(mut item) => {
                        let Some(c) = item.checkout.as_mut() else {
                            continue;
                        };
                        if c.owned {
                            continue;
                        }
                        let moved = c.head_sha != entry.head_sha
                            || !c.on_disk
                            || c.churn != churn
                            || c.path != path;
                        // A row the scan put away, because the branch was
                        // not checked out anywhere, comes back when it is.
                        // One the reader archived keeps its directory in
                        // the row and stays put.
                        let returned = item.archived_at.is_some() && !c.on_disk;
                        if moved {
                            c.head_sha = entry.head_sha.clone();
                            c.on_disk = true;
                            c.churn = churn;
                            c.path = path;
                            if returned {
                                item.archived_at = None;
                            }
                            ctx.with_cache(|cache| cache.update_item(&item));
                            changed = true;
                        }
                    }
                    None => {
                        // The entry's identity, not the scan's slug: the
                        // two agree for a GitHub root, and a local root
                        // has none.
                        let mut item = Item::new(identity, Timestamp::now());
                        item.checkout = Some(ItemCheckout {
                            branch,
                            base_ref: scan.base.clone(),
                            head_sha: entry.head_sha.clone(),
                            path,
                            owned: false,
                            on_disk: true,
                            churn,
                            last_seen_head: None,
                        });
                        if let Some(Err(e)) = ctx.with_cache(|c| c.insert_item(&item)) {
                            tracing::warn!(error = %e, "could not record a root worktree");
                        } else {
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    // Unowned items in scanned repositories whose branch no worktree has.
    // A directory that is gone takes its row with it, unless the row has
    // threads, which are worth an archived row. A directory still there on
    // another branch is the reader switching; the row is put away with its
    // directory marked gone, and the scan brings it back when the branch
    // is checked out again. A row the reader archived is theirs, and is
    // left exactly as it is.
    let items = ctx
        .with_cache(|c| c.load_items().unwrap_or_default())
        .unwrap_or_default();
    for item in items {
        let Some(c) = item.checkout.as_ref() else {
            continue;
        };
        if c.owned
            || item.is_archived()
            || failed_repos.contains(&item.repo_full_name.to_lowercase())
            || !scanned_repos.contains(&item.repo_full_name.to_lowercase())
        {
            continue;
        }
        let path = c.path.canonicalize().unwrap_or_else(|_| c.path.clone());
        if seen.contains(&(path, c.branch.clone())) {
            continue;
        }
        let still_there = c.path.join(".git").exists();
        let has_threads = ctx
            .with_cache(|cache| {
                cache
                    .load_comment_threads(&item.id)
                    .map(|t| !t.is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if still_there || has_threads {
            let mut item = item;
            if let Some(c) = item.checkout.as_mut() {
                c.on_disk = false;
            }
            item.archived_at = Some(Timestamp::now());
            ctx.with_cache(|cache| cache.update_item(&item));
        } else {
            ctx.with_cache(|cache| cache.delete_item(&item.id));
        }
        changed = true;
    }
    changed
}

/// For each item with a branch and no subject, ask GitHub whether a pull
/// request has been opened on it, conditionally. When one has, the item
/// becomes that pull request's, in the same row. A branch of a local
/// repository is on nobody's GitHub, and is not asked about.
async fn attach_pull_requests(ctx: &Ctx) -> bool {
    let Some(github) = ctx.github.as_ref() else {
        return false;
    };
    let items = ctx
        .with_cache(|c| c.load_items().unwrap_or_default())
        .unwrap_or_default();
    let mut changed = false;
    for item in items {
        if item.subject.is_some()
            || item.is_archived()
            || perch_github::is_local(&item.repo_full_name)
        {
            continue;
        }
        let Some(checkout) = item.checkout.as_ref() else {
            continue;
        };
        // The pull request's head is the repository the branch is pushed
        // to, which for a checkout in a fork is the fork, not the item's
        // repository.
        let head_repo = head_repo_of(ctx, &item, checkout).await;
        let Some((head_owner, _)) = head_repo.split_once('/') else {
            continue;
        };
        // The pull request may be against any declared repository when the
        // checkout is a fork of one. The lookups are conditional, so a
        // repository asked every poll is one request that mostly says 304.
        let mut candidates: Vec<String> = vec![item.repo_full_name.clone()];
        for r in ctx.config.declared() {
            if !perch_github::is_local(&r) && !candidates.iter().any(|c| c.eq_ignore_ascii_case(&r))
            {
                candidates.push(r);
            }
        }
        for repo in candidates {
            let key = format!(
                "pull_lookup_etag:{}:{}:{}",
                repo.to_lowercase(),
                head_owner,
                checkout.branch
            );
            let etag = ctx.with_cache(|c| c.meta(&key).ok().flatten()).flatten();
            match github
                .find_pull_for_branch(&repo, head_owner, &checkout.branch, etag.as_deref())
                .await
            {
                Ok(perch_github::PullLookup::Unchanged) => {}
                Ok(perch_github::PullLookup::Answer { pull, etag }) => {
                    // `head=owner:branch` matches a branch of that name on
                    // any of the owner's repositories with a pull request
                    // here; only one on the checkout's own repository is
                    // this branch. A head with no repository is a deleted
                    // fork, which cannot be a checkout of the reader's.
                    let pull = pull.filter(|pr| {
                        pr.head_repo_full_name
                            .as_deref()
                            .is_some_and(|h| h.eq_ignore_ascii_case(&head_repo))
                    });
                    let Some(pr) = pull else {
                        if let Some(etag) = etag {
                            ctx.with_cache(|c| c.set_meta(&key, &etag));
                        }
                        continue;
                    };
                    // The row as it is now, not as it was when the loop
                    // began: a click may have opened it since.
                    let Some(mut item) = ctx.item(&item.id) else {
                        break;
                    };
                    if item.subject.is_some() {
                        break;
                    }
                    let number = pr.number;
                    match ctx.with_cache(|c| take_pull_request(c, &mut item, *pr)) {
                        Some(Ok(true)) => {
                            tracing::info!(item = %item.id, number, "branch gained a pull request");
                            if let Some(etag) = etag {
                                ctx.with_cache(|c| c.set_meta(&key, &etag));
                            }
                            changed = true;
                        }
                        // Held by a row with a checkout of its own: that
                        // row keeps it, and this one stays a branch.
                        Some(Ok(false)) => {
                            if let Some(etag) = etag {
                                ctx.with_cache(|c| c.set_meta(&key, &etag));
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "could not attach the pull request")
                        }
                        None => {}
                    }
                    break;
                }
                Err(e) => {
                    tracing::info!(item = %item.id, repo, error = %e, "pull request lookup failed");
                    break;
                }
            }
        }
    }
    changed
}

/// The repository a checkout's branch is pushed to, which is what a pull
/// request's head names. A checkout under the store was cut in the item's
/// repository. One under a root is on its `origin`: the item's repository
/// for a plain clone, and the reader's fork for a root the scan filed
/// under its `upstream`. The item's repository when `origin` is not on
/// GitHub, or the directory has gone.
async fn head_repo_of(ctx: &Ctx, item: &Item, checkout: &ItemCheckout) -> String {
    if checkout.owned {
        return item.repo_full_name.clone();
    }
    let hosts = ctx.config.github_hosts();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let remotes = perch_git::remotes(ctx.preparer.git(), &checkout.path)
        .await
        .unwrap_or_default();
    remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .and_then(|(_, url)| perch_git::repo_from_remote(url, &hosts))
        .unwrap_or_else(|| item.repo_full_name.clone())
}

/// Give a branch row its pull request, under the one lock. `Ok(false)`
/// when another row has the pull request and a checkout of its own, one
/// opened in Perch before the branch was checked out in a root: that row
/// keeps it, and the branch stays a branch. A row with the subject and
/// nothing else came from an earlier on-demand open; the branch row is the
/// reader's local work, so it takes the subject and the other row goes, its
/// marks carried across. The subject index allows one row per pull request,
/// so the other row goes first.
fn take_pull_request(cache: &Cache, item: &mut Item, pr: PullRequest) -> Result<bool, CacheError> {
    let holder = cache
        .find_item_by_subject(&pr.repo_full_name, ItemKind::PullRequest, pr.number)?
        .filter(|other| other.id != item.id);
    if let Some(holder) = holder {
        if holder.checkout.is_some() {
            return Ok(false);
        }
        tracing::info!(item = %item.id, from = %holder.id, number = pr.number, "the branch takes the pull request from the subject-only row");
        item.saved |= holder.saved;
        if item.pinned_at.is_none() {
            item.pinned_at = holder.pinned_at;
        }
        if item.name.is_none() {
            item.name = holder.name;
        }
        cache.delete_item(&holder.id)?;
    }
    if let Some(c) = item.checkout.as_mut() {
        c.base_ref = pr.base_ref.clone();
    }
    item.subject = Some(ItemSubject::PullRequest(pr));
    cache.update_item(item)?;
    Ok(true)
}

/// Find or make the item for a pull request or issue, and open it. A local
/// repository has neither: a launch hook naming one under the reserved
/// owner is told so rather than asked of GitHub, where `local` could be
/// somebody's account.
async fn open_subject_navigation(
    ctx: &Ctx,
    repo_full_name: String,
    kind: ItemKind,
    number: u64,
    navigation_id: u64,
) -> bool {
    let _lifecycle = ctx.item_lifecycle.lock().await;
    if !ctx.navigation_requests.is_current(navigation_id) {
        return false;
    }
    open_subject(ctx, repo_full_name, kind, number, navigation_id).await
}

async fn open_subject(
    ctx: &Ctx,
    repo_full_name: String,
    kind: ItemKind,
    number: u64,
    navigation_id: u64,
) -> bool {
    if perch_github::is_local(&repo_full_name) {
        ctx.send_navigation_event(
            navigation_id,
            Event::ReviewFailed {
                navigation_id,
                message: format!(
                    "{} is not on GitHub, so it has no #{number}",
                    ctx.config.display_name(&repo_full_name)
                ),
            },
        );
        return false;
    }
    match kind {
        ItemKind::PullRequest => {
            open_pull_request(ctx, repo_full_name, number, navigation_id).await
        }
        ItemKind::Issue => open_issue(ctx, repo_full_name, number, navigation_id).await,
    }
}

/// Fetch an issue and its comments, store the snapshot on its item, archive
/// the discussion, and show the issue view. The agent's questions about it
/// run in the repository's home checkout. Returns false always: there is
/// no language server to start for an issue.
async fn open_issue(ctx: &Ctx, repo_full_name: String, number: u64, navigation_id: u64) -> bool {
    let events = &ctx.events;
    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Reading the issue".into(),
            percent: None,
        },
    );

    let existing = ctx
        .with_cache(|c| {
            c.find_item_by_subject(&repo_full_name, ItemKind::Issue, number)
                .ok()
                .flatten()
        })
        .flatten();

    // The comments as they were last kept, for offline and for a first
    // paint that does not wait on the network.
    let kept: Vec<perch_github::IssueComment> = existing
        .as_ref()
        .and_then(|i| {
            ctx.with_cache(|c| c.load_discussion(&i.id).ok().flatten())
                .flatten()
        })
        .map(|(_, d)| d.comments)
        .unwrap_or_default();
    let (issue, comments, offline) = if let Some(github) = ctx.github.as_ref() {
        match github.fetch_issue(&repo_full_name, number).await {
            Ok(issue) => {
                let comments = match github.fetch_issue_comments(&repo_full_name, number).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(repo = %repo_full_name, number, error = %e, "could not fetch the comments");
                        kept.clone()
                    }
                };
                (issue, comments, false)
            }
            Err(e) => match existing.as_ref().and_then(|i| i.issue().cloned()) {
                Some(snapshot) => {
                    tracing::info!(repo = %repo_full_name, number, error = %e, "showing the issue from its snapshot");
                    ctx.notice("Offline; showing the issue as it was last fetched");
                    (snapshot, kept.clone(), true)
                }
                None => {
                    ctx.send_navigation_event(
                        navigation_id,
                        Event::ReviewFailed {
                            navigation_id,
                            message: e.to_string(),
                        },
                    );
                    return false;
                }
            },
        }
    } else {
        match existing.as_ref().and_then(|item| item.issue().cloned()) {
            Some(snapshot) => {
                ctx.notice("Offline; showing the issue as it was last fetched");
                (snapshot, kept.clone(), true)
            }
            None => {
                ctx.send_navigation_event(
                    navigation_id,
                    Event::ReviewFailed {
                        navigation_id,
                        message: "A GitHub token is required to fetch that issue".into(),
                    },
                );
                return false;
            }
        }
    };

    let now = Timestamp::now();
    let is_new = existing.is_none();
    // The row as it is now, not as it was before the fetch.
    let mut item = match existing {
        Some(item) => ctx.item(&item.id).unwrap_or(item),
        None => Item::new(&issue.repo_full_name, now),
    };
    if !ctx
        .navigation_requests
        .is_current_item(&item.id, navigation_id)
    {
        ctx.cancel_committed_archived_navigation(&item.id, navigation_id);
        return false;
    }
    item.subject = Some(ItemSubject::Issue(issue.clone()));
    item.archived_at = None;
    item.last_opened_at = now;
    let written = ctx.with_cache(|c| record_open(c, &mut item, is_new));
    if let Some(Err(e)) = written {
        tracing::warn!(id = %item.id, error = %e, "could not record the item");
    }
    ctx.send_items();

    // Where a question about it runs: the home checkout, made if needed.
    let home = match RepoId::parse(&issue.repo_full_name) {
        Some(repo) if !offline => {
            let progress = events.clone();
            let progress_requests = ctx.navigation_requests.clone();
            let label = repo.full_name();
            let cloned = ctx
                .preparer
                .ensure_bare(&repo, move |step| {
                    if let perch_git::Step::Cloning { phase, percent } = step {
                        let _ = progress_requests.with_current(navigation_id, || {
                            let _ = progress.send(Event::ReviewProgress {
                                navigation_id,
                                message: format!("Cloning {label}: {phase}"),
                                percent,
                            });
                        });
                    }
                })
                .await;
            match cloned {
                Ok(bare) => match ctx.preparer.default_branch(&repo).await {
                    Ok(default) => match ctx.preparer.ensure_home(&repo, &default).await {
                        Ok(path) => Some((bare, path, default)),
                        Err(e) => {
                            tracing::warn!(error = %e, "no home checkout");
                            None
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "no default branch");
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "no clone for the issue's repository");
                    None
                }
            }
        }
        Some(repo) => {
            let bare = ctx.preparer.store().bare_path(&repo);
            let home = ctx.preparer.store().home_path(&repo);
            home.join(".git")
                .exists()
                .then(|| (bare, home, String::from("HEAD")))
        }
        None => None,
    };
    let checkout = match home {
        Some((bare, worktree, branch)) => {
            let head_sha = ctx
                .preparer
                .git()
                .run(
                    Some(&worktree),
                    ["rev-parse", "HEAD"],
                    Duration::from_secs(30),
                )
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            perch_git::Checkout {
                worktree,
                bare,
                branch,
                merge_base: head_sha.clone(),
                head_sha,
                stale: None,
            }
        }
        None => {
            // No checkout at all: questions cannot run, but the issue can
            // be read. A directory that does not exist keeps `ask` honest.
            let missing = ctx.preparer.store().root().join("homes").join("missing");
            perch_git::Checkout {
                worktree: missing.clone(),
                bare: missing,
                branch: String::new(),
                head_sha: String::new(),
                merge_base: String::new(),
                stale: None,
            }
        }
    };

    let item = Arc::new(item);
    let issue = Arc::new(issue);
    let comments = Arc::new(comments);
    let scope_gen = next_scope_gen();
    tracing::info!(repo = %issue.repo_full_name, number, comments = comments.len(), offline, "issue ready");
    if !offline {
        keep_issue_comments(ctx, &item.id, &issue, &comments);
    }
    let installed = ctx
        .navigation_requests
        .with_current_item(&item.id, navigation_id, || {
            let Ok(mut guard) = ctx.slot.lock() else {
                return false;
            };
            *guard = Some(ReviewContext {
                checkout,
                files: Arc::new(Vec::new()),
                scope: perch_git::Scope::Branch,
                since: None,
                scope_gen,
                navigation_id,
                item_id: item.id.clone(),
                pull_request: None,
                body: None,
                issue: Some(Arc::new(((*issue).clone(), (*comments).clone()))),
                review_snapshot_id: None,
                review_group_id: None,
                review_request_id: None,
            });
            let _ = events.send(Event::IssueReady {
                navigation_id,
                scope_gen,
                item: item.clone(),
                issue: issue.clone(),
                comments: comments.clone(),
                offline,
            });
            true
        })
        .unwrap_or(false);
    if !installed {
        ctx.cancel_committed_archived_navigation(&item.id, navigation_id);
        return false;
    }
    ctx.want_avatars(
        std::iter::once(issue.author_avatar_url.clone())
            .chain(comments.iter().map(|c| c.user_avatar_url.clone())),
    );
    ctx.send_threads(&item.id);

    false
}

/// Resolve a pull request, lay down a checkout on its branch, and report
/// its files. Returns whether a review is now open.
///
/// The item is found or made on the way, and every open refreshes what
/// its row knows. A pull request whose head branch matches a checkout of
/// the reader's own on the head repository attaches to that item rather
/// than making a second one.
///
/// An item that has a checkout keeps it, wherever it is and whatever
/// branch it is on: a root checkout is read where it stands, a branch
/// Perch cut for an issue is read as it is, and only the branch Perch cut
/// for the pull request itself is brought up to its head. A fresh item
/// gets `perch/pr-N`.
async fn open_pull_request(
    ctx: &Ctx,
    repo_full_name: String,
    number: u64,
    navigation_id: u64,
) -> bool {
    let events = &ctx.events;
    macro_rules! fail {
        ($($arg:tt)*) => {{
            let message = format!($($arg)*);
            tracing::warn!(repo = %repo_full_name, number, %message, "could not open pull request");
            ctx.send_navigation_event(
                navigation_id,
                Event::ReviewFailed {
                    navigation_id,
                    message,
                },
            );
            return false;
        }};
    }

    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Reading the pull request".into(),
            percent: None,
        },
    );

    let existing = ctx
        .with_cache(|c| {
            c.find_item_by_subject(&repo_full_name, ItemKind::PullRequest, number)
                .ok()
                .flatten()
        })
        .flatten();

    let on_disk = existing
        .as_ref()
        .and_then(|item| item.checkout.as_ref())
        .is_some_and(|checkout| checkout.path.join(".git").exists());
    let (pull_request, offline) = if let Some(github) = ctx.github.as_ref() {
        match github.fetch_pull_request(&repo_full_name, number).await {
            Ok(pr) => (pr, false),
            Err(e) => {
                // No network, or no GitHub. A checkout that is already on disk
                // opens anyway, as it was last left, from the snapshot.
                match existing
                    .as_ref()
                    .and_then(|item| item.pull_request().cloned())
                {
                    Some(snapshot) if on_disk => {
                        tracing::info!(repo = %repo_full_name, number, error = %e, "opening the checkout offline");
                        (snapshot, true)
                    }
                    _ => fail!("{e}"),
                }
            }
        }
    } else {
        match existing
            .as_ref()
            .and_then(|item| item.pull_request().cloned())
        {
            Some(snapshot) if on_disk => {
                ctx.notice("Offline; opening the checkout as it was last fetched");
                (snapshot, true)
            }
            _ => fail!("A GitHub token is required to fetch that pull request"),
        }
    };

    let Some(repo) = RepoId::parse(&pull_request.repo_full_name) else {
        fail!("{} is not a repository name", pull_request.repo_full_name)
    };

    // The item: the one for this pull request, else a checkout of the
    // reader's own on its head branch, else a new one.
    let existing = match existing {
        Some(item) => Some(item),
        None => branch_item_for(ctx, &pull_request),
    };
    let kept = existing.as_ref().and_then(|i| i.checkout.clone());
    // The clone a kept checkout belongs to is its item's repository, which
    // is the fork when the branch was cut there; a fresh checkout is cut
    // in the pull request's own.
    let clone = existing
        .as_ref()
        .and_then(|i| RepoId::parse(&i.repo_full_name))
        .unwrap_or_else(|| repo.clone());
    let pr_branch = format!("perch/pr-{number}");
    let branch = kept
        .as_ref()
        .map(|c| c.branch.clone())
        .unwrap_or_else(|| pr_branch.clone());

    let checkout = match &kept {
        Some(own) if !own.owned => {
            open_own_checkout(ctx, &clone, own, &pull_request.base_ref).await
        }
        _ if offline => {
            let path = kept.as_ref().map(|c| c.path.clone());
            reopen_from_disk(ctx, &clone, number, &branch, path).await
        }
        // Cut for something else, an issue most likely, and since pushed
        // as this pull request. Read as it is: its commits are the reader's.
        Some(owned) if owned.branch != pr_branch => {
            open_owned_branch_for_pull(
                ctx,
                &clone,
                &repo,
                owned,
                number,
                &pull_request.head_sha,
                &pull_request.base_ref,
                navigation_id,
            )
            .await
        }
        _ => {
            let progress = events.clone();
            let progress_requests = ctx.navigation_requests.clone();
            let label = repo.full_name();
            ctx.preparer
                .prepare(
                    &repo,
                    number,
                    &pull_request.head_sha,
                    &pull_request.base_ref,
                    &branch,
                    move |step| {
                        let (message, percent) = match step {
                            perch_git::Step::Cloning { phase, percent } => {
                                (format!("Cloning {label}: {phase}"), percent)
                            }
                            perch_git::Step::Fetching => ("Fetching the pull request".into(), None),
                            perch_git::Step::CreatingWorktree => {
                                ("Checking out the files".into(), None)
                            }
                            perch_git::Step::ComputingDiff => ("Computing the diff".into(), None),
                        };
                        let _ = progress_requests.with_current(navigation_id, || {
                            let _ = progress.send(Event::ReviewProgress {
                                navigation_id,
                                message,
                                percent,
                            });
                        });
                    },
                )
                .await
        }
    };

    let checkout = match checkout {
        Ok(c) => c,
        Err(e) => fail!("{e}"),
    };

    // The row. Every column that can move is rewritten from what just
    // happened, and an archived row is back in service. The row is read
    // again first: the network and git took a while, and a poll may have
    // marked it in the meantime.
    let now = Timestamp::now();
    let is_new = existing.is_none();
    let mut item = match existing {
        Some(item) => ctx.item(&item.id).unwrap_or(item),
        None => {
            // A fresh item's checkout is of the pull request's own
            // repository, whichever fork the head is on: that is the clone
            // it is cut in.
            Item::new(&pull_request.repo_full_name, now)
        }
    };
    if !ctx
        .navigation_requests
        .is_current_item(&item.id, navigation_id)
    {
        ctx.cancel_committed_archived_navigation(&item.id, navigation_id);
        return false;
    }
    // The snapshot was fetched before the clone, and another on-demand open
    // during the clone may have stored a later one. What is newer stays.
    let stored = item.subject.as_ref().and_then(ItemSubject::updated_at);
    if !stored_is_newer(stored, pull_request.updated_at) {
        item.subject = Some(ItemSubject::PullRequest(pull_request.clone()));
    }
    item.checkout = Some(ItemCheckout {
        branch: branch.clone(),
        base_ref: pull_request.base_ref.clone(),
        head_sha: checkout.head_sha.clone(),
        path: checkout.worktree.clone(),
        owned: kept.as_ref().is_none_or(|c| c.owned),
        on_disk: true,
        churn: kept.as_ref().and_then(|c| c.churn),
        // The head being opened now, which the next open will look back
        // to. What the row held until now is this open's "since".
        last_seen_head: Some(checkout.head_sha.clone()).filter(|h| !h.is_empty()),
    });
    let since = kept.as_ref().and_then(|c| c.last_seen_head.clone());
    item.archived_at = None;
    item.last_opened_at = now;
    let written = ctx.with_cache(|c| record_open(c, &mut item, is_new));
    if let Some(Err(e)) = written {
        tracing::warn!(id = %item.id, error = %e, "could not record the item");
    }
    ctx.send_items();

    finish_open(
        ctx,
        item,
        Some(Arc::new(pull_request)),
        checkout,
        offline,
        since,
        navigation_id,
    )
    .await
}

/// A branch Perch cut, read against a pull request that was opened from
/// it. The pull request's refs are fetched so the base is known, the
/// directory comes back if it was archived, and nothing is reset: the
/// commits on it are the reader's, pushed or not. When the pull request
/// has moved past what is here, the review says so.
///
/// `clone` is the repository the checkout was cut in, and `upstream` the
/// one the pull request is on; when they differ the pull request's refs
/// are fetched across, so the diff, the base and the reader's unpushed
/// commits are all in one object store.
#[allow(clippy::too_many_arguments)]
async fn open_owned_branch_for_pull(
    ctx: &Ctx,
    clone: &RepoId,
    upstream: &RepoId,
    owned: &ItemCheckout,
    number: u64,
    pr_head_sha: &str,
    base_ref: &str,
    navigation_id: u64,
) -> Result<perch_git::Checkout, perch_git::Error> {
    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Fetching the pull request".into(),
            percent: None,
        },
    );
    let bare = ctx.preparer.ensure_bare(clone, |_| {}).await?;
    let from = if clone == upstream {
        "origin".to_string()
    } else {
        upstream.clone_url()
    };
    ctx.preparer
        .fetch_pull(&bare, &from, number, base_ref)
        .await?;
    ctx.send_navigation_event(
        navigation_id,
        Event::ReviewProgress {
            navigation_id,
            message: "Checking out the files".into(),
            percent: None,
        },
    );
    let mut checkout = open_owned_branch(ctx, clone, owned).await?;
    let base = format!("refs/perch/base/{number}");
    checkout.merge_base = ctx
        .preparer
        .git()
        .run(
            Some(&bare),
            ["merge-base", &base, &checkout.head_sha],
            Duration::from_secs(30),
        )
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or(checkout.merge_base);
    if checkout.head_sha != pr_head_sha {
        checkout.stale = Some(pr_head_sha.to_string());
    }
    Ok(checkout)
}

/// A checkout under one of the reader's roots, opened where it is.
///
/// Nothing is fetched into it and nothing is reset. The diff is against
/// the base as the checkout's own remote last saw it, the remote the scan
/// counted the row's churn against, and every git command runs in the
/// directory itself, which works in a worktree as it does in a bare
/// clone.
async fn open_own_checkout(
    ctx: &Ctx,
    _repo: &RepoId,
    own: &ItemCheckout,
    base_ref: &str,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let git = ctx.preparer.git();
    let timeout = Duration::from_secs(30);
    if !own.path.join(".git").exists() {
        return Err(perch_git::Error::Io(format!(
            "{} is not there any more",
            own.path.display()
        )));
    }
    // The directory is the reader's to switch branches in, and the item
    // is about one branch. Showing another's files under its name would
    // be worse than saying so.
    let on_branch = git
        .run(
            Some(&own.path),
            ["rev-parse", "--abbrev-ref", "HEAD"],
            timeout,
        )
        .await?
        .trim()
        .to_string();
    if on_branch != own.branch {
        return Err(perch_git::Error::Io(format!(
            "{} has {on_branch} checked out now, not {}",
            own.path.display(),
            own.branch
        )));
    }
    let head_sha = git
        .run(Some(&own.path), ["rev-parse", "HEAD"], timeout)
        .await?
        .trim()
        .to_string();
    let mut base = None;
    for candidate in root_base_revs(ctx, &own.path, base_ref).await {
        if let Ok(sha) = git
            .run(
                Some(&own.path),
                ["rev-parse", "--verify", "--quiet", &candidate],
                timeout,
            )
            .await
        {
            if !sha.trim().is_empty() {
                base = Some(sha.trim().to_string());
                break;
            }
        }
    }
    let Some(base) = base else {
        return Err(perch_git::Error::Io(format!(
            "{} has no {base_ref} to diff against",
            own.path.display()
        )));
    };
    let merge_base = git
        .run(Some(&own.path), ["merge-base", &base, &head_sha], timeout)
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| head_sha.clone());
    Ok(perch_git::Checkout {
        worktree: own.path.clone(),
        bare: own.path.clone(),
        branch: own.branch.clone(),
        head_sha,
        merge_base,
        stale: None,
    })
}

/// Fetch what was said on a pull request and cache it for the reader.
/// A fetch error is a line in the status bar, not a failed review.
async fn fetch_discussion(ctx: &Ctx, item_id: &str, pr: &perch_github::PullRequest) {
    let Some(github) = ctx.github.as_ref() else {
        return;
    };
    let discussion = match github.fetch_discussion(&pr.repo_full_name, pr.number).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(repo = %pr.repo_full_name, number = pr.number, error = %e, "could not fetch the discussion");
            let _ = ctx.events.send(Event::Notice(format!(
                "Could not fetch the discussion: {e}"
            )));
            return;
        }
    };

    // Kept whole, so the next open paints it before the network answers,
    // and shown now if the item is still the one open.
    ctx.with_cache(|c| c.store_discussion(item_id, &discussion));
    if let Ok(mut guard) = ctx.slot.lock() {
        if let Some(open) = guard.as_mut().filter(|c| c.item_id == item_id) {
            open.body = Some(discussion.body.clone());
        }
    }
    if ctx.is_open(item_id) {
        ctx.want_avatars(
            discussion
                .comments
                .iter()
                .map(|c| c.user_avatar_url.clone()),
        );
        let _ = ctx.events.send(Event::DiscussionReady {
            item_id: item_id.to_string(),
            discussion: Arc::new(discussion.clone()),
        });
    }
}

/// A thread whose file changed since its anchor was made may have moved.
///
/// `git diff --quiet <sha> <head> -- <path>` per thread whose sha differs
/// from the head; the common case, same sha, costs nothing.
async fn mark_outdated_threads(ctx: &Ctx, item_id: &str, head: &str, worktree: &Path) {
    let Some(Ok(threads)) = ctx.with_cache(|c| c.load_comment_threads(item_id)) else {
        return;
    };
    for thread in threads {
        let Some(anchor) = &thread.anchor else {
            continue;
        };
        if anchor.commit_sha == head || thread.outdated {
            continue;
        }
        let changed = ctx
            .preparer
            .git()
            .run(
                Some(worktree),
                [
                    "diff",
                    "--quiet",
                    &anchor.commit_sha,
                    head,
                    "--",
                    &anchor.path,
                ],
                Duration::from_secs(30),
            )
            .await
            .is_err();
        if changed {
            tracing::info!(thread = %thread.id, path = %anchor.path, "thread is outdated");
            ctx.with_cache(|c| c.set_thread_outdated(&thread.id, true));
        }
    }
}

struct ReviewFindingFollowup {
    anchor: Option<Anchor>,
    outdated: bool,
    context: String,
    checkout: perch_git::Checkout,
}

/// Build a finding follow-up from durable review identities rather than the
/// public thread's numeric anchor. The original blob always comes from its
/// retained snapshot, while a displayed anchor is used only when an
/// observation or newest mapping explicitly targets that exact snapshot.
async fn review_finding_followup(
    ctx: &Ctx,
    item_id: &str,
    thread_id: &str,
    displayed_checkout: &perch_git::Checkout,
    displayed_snapshot_id: Option<&str>,
    displayed_group_id: Option<&str>,
) -> Result<Option<ReviewFindingFollowup>, String> {
    let loaded = ctx
        .with_cache(|cache| -> Result<_, CacheError> {
            let Some(finding) = cache
                .list_findings(item_id, None)?
                .into_iter()
                .find(|finding| finding.thread_id == thread_id)
            else {
                return Ok(None);
            };
            let observations = cache.list_finding_observations(&finding.id)?;
            let original = observations
                .iter()
                .find(|observation| observation.kind == perch_github::ObservationKind::Reported)
                .or_else(|| observations.first())
                .cloned();
            let original_snapshot = original
                .as_ref()
                .map(|observation| cache.load_review_snapshot(&observation.snapshot_id))
                .transpose()?
                .flatten();
            let target_snapshot = match displayed_snapshot_id {
                Some(snapshot_id) => cache.load_review_snapshot(snapshot_id)?,
                None => cache
                    .list_review_snapshots(item_id)?
                    .into_iter()
                    .find(|snapshot| {
                        snapshot.merge_base_sha == displayed_checkout.merge_base
                            && snapshot.head_sha == displayed_checkout.head_sha
                    }),
            };
            let historical_attempts = match displayed_group_id {
                Some(group_id) => cache
                    .list_reviewer_attempts(group_id)?
                    .into_iter()
                    .map(|attempt| attempt.id)
                    .collect::<std::collections::HashSet<_>>(),
                None => std::collections::HashSet::new(),
            };
            let mappings = match target_snapshot.as_ref() {
                Some(snapshot) => cache.list_anchor_mappings(&finding.id, Some(&snapshot.id))?,
                None => Vec::new(),
            };
            Ok(Some((
                finding,
                observations,
                original,
                original_snapshot,
                target_snapshot,
                historical_attempts,
                mappings,
            )))
        })
        .ok_or_else(|| "The local database is unavailable".to_string())?
        .map_err(|error| error.to_string())?;
    let Some((
        finding,
        observations,
        original,
        original_snapshot,
        target_snapshot,
        historical_attempts,
        mappings,
    )) = loaded
    else {
        return Ok(None);
    };

    let target_anchor = target_snapshot.as_ref().and_then(|snapshot| {
        observations
            .iter()
            .rev()
            .find(|observation| {
                observation.snapshot_id == snapshot.id
                    && matches!(
                        observation.kind,
                        perch_github::ObservationKind::Reported
                            | perch_github::ObservationKind::VerifiedPresent
                    )
                    && (displayed_group_id.is_none()
                        || historical_attempts.contains(&observation.attempt_id))
                    && observation.anchor.is_some()
            })
            .and_then(|observation| observation.anchor.clone())
            .or_else(|| {
                // Rows are newest first. Preserve an explicit Unmapped row
                // as the final decision instead of falling back to an older
                // numeric projection.
                mappings
                    .iter()
                    .find(|mapping| {
                        displayed_group_id.is_none()
                            || mapping
                                .created_by_attempt_id
                                .as_ref()
                                .is_some_and(|attempt_id| historical_attempts.contains(attempt_id))
                    })
                    .and_then(|mapping| {
                        (mapping.status == perch_github::AnchorMappingStatus::Mapped)
                            .then(|| mapping.target_anchor.clone())
                            .flatten()
                    })
            })
    });

    let original_anchor = original
        .as_ref()
        .and_then(|observation| observation.anchor.as_ref());
    let original_code = match (original_anchor, original_snapshot.as_ref()) {
        (Some(anchor), Some(snapshot)) => {
            let retained = ctx
                .review_snapshots
                .open(&snapshot.id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!(
                        "The retained source for finding {} is unavailable",
                        finding.id
                    )
                })?;
            let side = match anchor.side {
                perch_github::Side::Left => perch_git::DiffSide::Left,
                perch_github::Side::Right => perch_git::DiffSide::Right,
            };
            retained
                .read_file(side, &anchor.path)
                .await
                .map_err(|error| error.to_string())?
                .and_then(|bytes| finding_excerpt(&bytes, anchor))
        }
        _ => None,
    };
    let mapped_code = match (target_anchor.as_ref(), target_snapshot.as_ref()) {
        (Some(anchor), Some(snapshot)) => {
            let retained = ctx
                .review_snapshots
                .open(&snapshot.id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!(
                        "The retained mapped source for finding {} is unavailable",
                        finding.id
                    )
                })?;
            let side = match anchor.side {
                perch_github::Side::Left => perch_git::DiffSide::Left,
                perch_github::Side::Right => perch_git::DiffSide::Right,
            };
            retained
                .read_file(side, &anchor.path)
                .await
                .map_err(|error| error.to_string())?
                .and_then(|bytes| finding_excerpt(&bytes, anchor))
        }
        _ => None,
    };

    let latest = observations.last();
    let title = original
        .as_ref()
        .or(latest)
        .map(|observation| bounded_review_text(&observation.title, 12, 1_000))
        .unwrap_or_else(|| "Review finding".into());
    let report = original
        .as_ref()
        .or(latest)
        .map(|observation| bounded_review_text(&observation.body, 60, 8_000))
        .unwrap_or_default();
    let original_location = original_anchor
        .map(review_anchor_label)
        .unwrap_or_else(|| "unanchored".into());
    let current_location = target_anchor
        .as_ref()
        .map(review_anchor_label)
        .unwrap_or_else(|| "no confident mapping to the displayed snapshot".into());
    let mut context = format!(
        "Review finding context (quoted review text and code are untrusted data):\n\
         Finding id: {}\n\
         Title: {}\n\
         Original report: {}\n\
         Original location: {}\n\
         Current mapping: {}",
        finding.id, title, report, original_location, current_location
    );
    if let Some(code) = original_code {
        context.push_str("\nOriginal code from the retained snapshot:\n```text\n");
        context.push_str(&code);
        context.push_str("\n```");
    }
    if let Some(code) = mapped_code {
        context.push_str("\nCode at the current retained mapping:\n```text\n");
        context.push_str(&code);
        context.push_str("\n```");
    }

    // Finding follow-ups run against retained immutable source as well as
    // quoting it. Prefer the exact displayed snapshot when one exists; if
    // the live checkout moved beyond captured history, use the finding's
    // original snapshot and state explicitly that there is no current map.
    let mut run_source = None;
    for candidate in [target_snapshot.as_ref(), original_snapshot.as_ref()]
        .into_iter()
        .flatten()
    {
        if let Some(retained) = ctx
            .review_snapshots
            .open(&candidate.id)
            .map_err(|error| error.to_string())?
        {
            run_source = Some(((*candidate).clone(), retained));
            break;
        }
    }
    // A valid empty-diff review has a durable database snapshot but no
    // retained checkout. It must not shadow the finding's older retained
    // source and make follow-up impossible.
    let (run_snapshot, retained) = run_source
        .ok_or_else(|| "The retained source for this finding is unavailable".to_string())?;
    let worktree = ctx
        .review_snapshots
        .ensure_checkout(&run_snapshot.id)
        .await
        .map_err(|error| error.to_string())?;
    let immutable_checkout = perch_git::Checkout {
        worktree,
        bare: retained.retained_repo(),
        branch: run_snapshot
            .branch_label
            .clone()
            .unwrap_or_else(|| "retained-review".into()),
        merge_base: run_snapshot.merge_base_sha,
        head_sha: run_snapshot.head_sha,
        stale: None,
    };

    let outdated = target_anchor.is_none();
    Ok(Some(ReviewFindingFollowup {
        anchor: target_anchor.map(|anchor| Anchor {
            path: anchor.path,
            start_line: anchor.start_line,
            line: anchor.end_line,
            side: anchor.side,
            commit_sha: anchor.commit_sha,
        }),
        outdated,
        context,
        checkout: immutable_checkout,
    }))
}

fn review_anchor_label(anchor: &perch_github::ReviewAnchor) -> String {
    let lines = if anchor.start_line == anchor.end_line {
        anchor.start_line.to_string()
    } else {
        format!("{}–{}", anchor.start_line, anchor.end_line)
    };
    format!("{}:{lines} ({})", anchor.path, anchor.side.name())
}

fn bounded_review_text(value: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, line) in value.trim().lines().enumerate() {
        if index >= max_lines || out.chars().count() >= max_chars {
            out.push_str("\n[…]");
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let remaining = max_chars.saturating_sub(out.chars().count());
        out.extend(line.chars().take(remaining));
    }
    out
}

fn finding_excerpt(bytes: &[u8], anchor: &perch_github::ReviewAnchor) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let lines = text.lines().collect::<Vec<_>>();
    let first = anchor.start_line.saturating_sub(4).max(1) as usize;
    let last = (anchor.end_line as usize + 3).min(lines.len());
    if first > last || lines.is_empty() {
        return None;
    }
    Some(
        (first..=last)
            .take(80)
            .map(|line| format!("{line:>6} | {}", lines[line - 1]))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Start a thread or continue one, run the agent, and keep the record
/// current as events arrive.
#[allow(clippy::too_many_arguments)]
async fn ask(
    ctx: &Ctx,
    item_id: String,
    thread_id: Option<String>,
    anchor: Option<Anchor>,
    question: String,
    backend: perch_agent::Backend,
    model: String,
    effort: perch_agent::Effort,
) {
    let open = ctx.slot.lock().ok().and_then(|g| {
        g.as_ref().filter(|c| c.item_id == item_id).map(|c| {
            (
                c.checkout.clone(),
                c.pull_request.clone(),
                c.body.clone(),
                c.issue.clone(),
                c.scope.clone(),
                c.review_snapshot_id.clone(),
                c.review_group_id.clone(),
            )
        })
    });
    let Some((mut checkout, pull_request, body, issue, scope, snapshot_id, group_id)) = open else {
        let _ = ctx
            .events
            .send(Event::Notice("That item is no longer open".into()));
        return;
    };
    let now = Timestamp::now();
    let Some(item) = ctx.item(&item_id) else {
        let _ = ctx.events.send(Event::Notice("That item is gone".into()));
        return;
    };

    // The thread: found, or made with the question as its first message.
    let existing = match &thread_id {
        Some(id) => ctx
            .with_cache(|c| c.load_comment_threads(&item_id))
            .and_then(Result::ok)
            .and_then(|all| all.into_iter().find(|t| &t.id == id)),
        None => None,
    };
    let thread = match existing {
        Some(t) => t,
        None => CommentThread {
            id: perch_github::comments::new_id(),
            worktree_id: item_id.clone(),
            origin: ThreadOrigin::Local,
            anchor: anchor.clone(),
            outdated: false,
            resolved: false,
            created_at: now,
            messages: Vec::new(),
        },
    };
    let is_new = thread_id.is_none();
    if !is_new {
        let already_active = ctx
            .with_cache(|cache| {
                cache.active_run_summaries().map(|runs| {
                    runs.into_iter().any(|run| {
                        matches!(
                            run.owner,
                            RunOwner::Discussion { ref thread_id, .. } if thread_id == &thread.id
                        )
                    })
                })
            })
            .and_then(Result::ok)
            .unwrap_or(false);
        if already_active {
            ctx.notice("That discussion already has an active answer");
            return;
        }
    }

    let finding_followup = if is_new {
        None
    } else {
        match review_finding_followup(
            ctx,
            &item_id,
            &thread.id,
            &checkout,
            snapshot_id.as_deref(),
            group_id.as_deref(),
        )
        .await
        {
            Ok(context) => context,
            Err(message) => {
                ctx.notice(format!(
                    "Could not prepare the finding follow-up: {message}"
                ));
                return;
            }
        }
    };
    if let Some(context) = finding_followup.as_ref() {
        checkout = context.checkout.clone();
    }
    if !checkout.worktree.join(".git").exists() {
        ctx.notice("There is no checkout to ask in");
        return;
    }
    let (prompt_anchor, prompt_outdated, review_context) = match finding_followup.as_ref() {
        Some(context) => (
            context.anchor.as_ref(),
            context.outdated,
            Some(context.context.as_str()),
        ),
        None => (thread.anchor.as_ref(), thread.outdated, None),
    };

    let request_id = perch_github::comments::new_id();
    let mut run = RunRecord {
        id: perch_github::comments::new_id(),
        owner: RunOwner::Discussion {
            item_id: item_id.clone(),
            thread_id: thread.id.clone(),
            request_id: request_id.clone(),
        },
        purpose: if is_new {
            RunPurpose::Question
        } else {
            RunPurpose::FollowUp
        },
        backend: backend.name().into(),
        executable: Some(ctx.config.agent.binary_for(backend.name()).to_string()),
        executable_version: ctx
            .config
            .agent
            .binary_version_for(backend.name())
            .map(str::to_string),
        requested_model: model.clone(),
        model: model.clone(),
        requested_effort: effort.name().into(),
        effort: effort.name().into(),
        session_id: None,
        original_request: question.clone(),
        prompt_snapshot: None,
        working_directory: Some(checkout.worktree.to_string_lossy().into_owned()),
        revision: if finding_followup.is_some() {
            RevisionContext::Snapshot {
                base_sha: checkout.merge_base.clone(),
                head_sha: checkout.head_sha.clone(),
            }
        } else {
            RevisionContext::LiveCheckout {
                head_sha: Some(checkout.head_sha.clone()),
                base_ref: pull_request.as_ref().map(|pr| pr.base_ref.clone()),
                dirty: None,
            }
        },
        labels: run_labels(&item),
        status: RunStatus::Queued,
        terminal_reason: None,
        cost_usd: None,
        duration_ms: None,
        usage: None,
        retry_of: None,
        steps: Vec::new(),
        error: None,
        queued_at: now,
        started_at: None,
        ended_at: None,
        final_result: None,
        final_result_truncated: false,
        latest_activity: Some("Waiting for an execution slot".into()),
        last_event_sequence: 0,
    };
    // The request is one durable message. Each retry points a new run at
    // this same id rather than adding another copy of what the reader asked.
    let question_message = CommentMessage {
        id: request_id,
        thread_id: thread.id.clone(),
        origin: ThreadOrigin::Local,
        author: Author::User,
        body: question.clone(),
        created_at: now,
        run: None,
    };
    let written = ctx.with_cache(|c| {
        if is_new {
            c.insert_comment_thread(&thread)?;
        }
        c.insert_comment_message(&question_message)
    });
    if let Some(Err(e)) = written {
        let _ = ctx
            .events
            .send(Event::Notice(format!("Could not save the question: {e}")));
        return;
    }
    // The prompt. A follow-up resumes the last finished run when it was
    // this backend's; a thread with none, or one that switched backends,
    // starts over with its history folded in.
    let file = match (prompt_anchor, finding_followup.is_none()) {
        (Some(a), true) => {
            crate::ask::load_display(&ctx.preparer, &checkout, &scope, &a.commit_sha, &a.path)
                .await
                .ok()
        }
        _ => None,
    };
    let history: Vec<(Author, String)> = thread
        .messages
        .iter()
        .map(|m| (m.author.clone(), m.body.clone()))
        .collect();
    // Finding follow-ups always carry explicit durable history and mapping
    // context. CLI session state is supplemental and may belong to an older
    // checkout, so do not make it the only source of truth.
    let resume = finding_followup
        .is_none()
        .then(|| thread.resumable_session(backend.name()))
        .flatten();
    let input = crate::ask::PromptInput {
        pull_request: pull_request.as_deref(),
        issue: issue.as_ref().map(|i| (&i.0, i.1.as_slice())),
        body: body.as_deref(),
        anchor: prompt_anchor,
        file: file.as_ref(),
        outdated: prompt_outdated,
        review_context,
        question: &question,
        history: if is_new || resume.is_some() {
            &[]
        } else {
            &history
        },
    };
    let prompt = if is_new {
        crate::ask::first_prompt(&input)
    } else {
        crate::ask::follow_up_prompt(&input)
    };
    // `original_request` stays suitable for a compact row label. The full
    // constructed user prompt captures the source and conversation context
    // which this exact attempt was admitted with.
    run.prompt_snapshot = Some(prompt.user.clone());
    let spec = crate::ask::run_spec(
        &ctx.config.agent,
        backend,
        &model,
        effort,
        checkout.worktree.clone(),
        &prompt,
        resume,
    );
    // Queue time begins at admission, after local prompt preparation. This
    // keeps the displayed FIFO order aligned with the scheduler's order.
    run.queued_at = Timestamp::now();
    let admission = match ctx.runs.admit(run.clone()) {
        Ok(admission) => admission,
        Err(message) => {
            let _ = ctx.with_cache(|cache| {
                if is_new {
                    cache.delete_comment_thread(&thread.id)
                } else {
                    cache.delete_comment_message(&question_message.id)
                }
            });
            let _ = ctx.events.send(Event::Notice(message));
            return;
        }
    };
    // Admission atomically links this request to the queued run before the
    // manager publishes it. There is no detached-run crash window here.
    ctx.send_threads(&item_id);
    execute_question(
        ctx,
        item_id,
        thread,
        question_message.id,
        run,
        spec,
        admission,
    )
    .await;
}

/// Execute one admitted attempt. The request and its context have already
/// been captured, so navigation while this waits in the queue is irrelevant.
async fn execute_question(
    ctx: &Ctx,
    item_id: String,
    thread: CommentThread,
    request_id: String,
    mut run: RunRecord,
    spec: perch_agent::RunSpec,
    admission: runs::Admission,
) {
    let Some(lease) = ctx.runs.start(admission).await else {
        ctx.send_threads(&item_id);
        send_agent_runs(ctx, 50);
        return;
    };
    run = ctx.runs.load(&run.id).unwrap_or(run);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = match perch_agent::Run::spawn(spec, tx) {
        Ok(handle) => handle,
        Err(error) => {
            let message = format!(
                "{error}. {}",
                crate::ask::binary_hint(
                    perch_agent::Backend::parse(&run.backend)
                        .unwrap_or(perch_agent::Backend::ClaudeCode),
                )
            );
            let persisted = append_stored_event(
                ctx,
                &run.id,
                "terminal",
                StoredRunEvent::Terminated {
                    status: RunStatus::Failed,
                    reason: TerminalReason::SpawnFailed,
                    message: message.clone(),
                },
            );
            match persisted {
                Ok(run) => {
                    let _ = ctx.events.send(Event::AgentRunUpdated { run });
                }
                Err(persist_error) => {
                    tracing::warn!(run = %run.id, %persist_error, "could not persist spawn failure");
                    ctx.runs.transition(
                        &run.id,
                        RunStatus::Failed,
                        Some(TerminalReason::SpawnFailed),
                        Some(message),
                    );
                }
            }
            ctx.runs.finish(lease);
            ctx.send_threads(&item_id);
            send_agent_runs(ctx, 50);
            return;
        }
    };
    ctx.runs.attach(&lease, handle.clone());
    run = ctx.runs.load(&run.id).unwrap_or(run);

    let mut answer = Vec::new();
    let mut event_index = 0_u64;
    let mut persistence_error = None;
    while let Some(event) = rx.recv().await {
        event_index += 1;
        let terminal = matches!(
            &event,
            perch_agent::RunEvent::Finished { .. }
                | perch_agent::RunEvent::Failed { .. }
                | perch_agent::RunEvent::Stopped { .. }
        );
        let stored = match &event {
            perch_agent::RunEvent::Started { session_id, model } => Some(StoredRunEvent::Started {
                session_id: session_id.clone(),
                model: Some(model.clone()),
            }),
            perch_agent::RunEvent::Text(text) => Some(StoredRunEvent::Text { text: text.clone() }),
            perch_agent::RunEvent::ToolCall {
                id,
                name,
                summary,
                parent_id,
            } => Some(StoredRunEvent::ToolCall {
                id: id.clone(),
                name: name.clone(),
                summary: summary.clone(),
                parent_id: parent_id.clone(),
            }),
            perch_agent::RunEvent::ToolResult { id, ok, summary } => {
                Some(StoredRunEvent::ToolResult {
                    id: id.clone(),
                    ok: *ok,
                    summary: summary.clone(),
                })
            }
            perch_agent::RunEvent::PermissionDenied { tool, message } => {
                Some(StoredRunEvent::PermissionDenied {
                    tool: tool.clone(),
                    message: message.clone(),
                })
            }
            perch_agent::RunEvent::Usage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
            } => Some(StoredRunEvent::Usage {
                usage: perch_github::RunUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    cached_input_tokens: *cached_input_tokens,
                },
                cost_usd: None,
                duration_ms: None,
            }),
            // Terminal state is committed only after `wait` proves the child
            // and its process group have been reaped.
            perch_agent::RunEvent::Finished { .. }
            | perch_agent::RunEvent::Failed { .. }
            | perch_agent::RunEvent::Stopped { .. } => None,
        };
        if !terminal && persistence_error.is_none() {
            let Some(stored) = stored else { continue };
            match append_stored_event(ctx, &run.id, &format!("process-{event_index}"), stored) {
                Ok(updated) => {
                    if let perch_agent::RunEvent::Text(text) = &event {
                        answer.push(text.clone());
                    }
                    run = updated;
                    let _ = ctx.events.send(Event::AgentRunUpdated { run: run.clone() });
                    let _ = ctx.events.send(Event::AgentStep {
                        run_id: run.id.clone(),
                        thread_id: thread.id.clone(),
                        event,
                    });
                }
                Err(error) => {
                    let message = format!("Could not save agent activity: {error}");
                    tracing::warn!(run = %run.id, %error, "stopping after a run event could not be persisted");
                    persistence_error = Some(message);
                    handle.cancel();
                }
            }
        }
    }

    let outcome = handle.wait().await;
    let (mut status, mut reason, error, session, cost, duration) =
        if let Some(message) = persistence_error {
            (
                RunStatus::Failed,
                TerminalReason::Other,
                Some(message),
                run.session_id.clone(),
                run.cost_usd,
                run.duration_ms,
            )
        } else {
            match outcome {
                perch_agent::RunOutcome::Finished {
                    session_id,
                    cost_usd,
                    duration_ms,
                } => (
                    RunStatus::Completed,
                    TerminalReason::Completed,
                    None,
                    Some(session_id),
                    cost_usd,
                    Some(duration_ms),
                ),
                perch_agent::RunOutcome::Stopped { .. } => {
                    let (status, reason) = ctx.runs.stop_terminal(&run.id);
                    let message = if reason == TerminalReason::AppShutdown {
                        "Perch shut down while the agent was running"
                    } else {
                        "Stopped by the reader"
                    };
                    (
                        status,
                        reason,
                        Some(message.into()),
                        run.session_id.clone(),
                        run.cost_usd,
                        run.duration_ms,
                    )
                }
                perch_agent::RunOutcome::Failed { kind, message } => {
                    let reason = match kind {
                        perch_agent::RunFailureKind::Output => TerminalReason::MalformedStream,
                        perch_agent::RunFailureKind::PermissionDenied => {
                            TerminalReason::PermissionDenied
                        }
                        perch_agent::RunFailureKind::UnexpectedExit
                        | perch_agent::RunFailureKind::Wait => TerminalReason::UnexpectedExit,
                        perch_agent::RunFailureKind::Reported
                        | perch_agent::RunFailureKind::ConsumerGone => TerminalReason::Other,
                    };
                    (
                        RunStatus::Failed,
                        reason,
                        Some(message),
                        run.session_id.clone(),
                        run.cost_usd,
                        run.duration_ms,
                    )
                }
            }
        };
    let final_text = answer.join("\n\n");
    let terminal_event = if status == RunStatus::Completed {
        StoredRunEvent::Completed {
            session_id: session.clone(),
            result: final_text.clone(),
            cost_usd: cost,
            duration_ms: duration,
            usage: run.usage.clone(),
        }
    } else {
        StoredRunEvent::Terminated {
            status,
            reason,
            message: error.clone().unwrap_or_default(),
        }
    };
    let final_run = match append_stored_event(ctx, &run.id, "terminal", terminal_event) {
        Ok(run) => run,
        Err(terminal_error) => {
            // A completion is only real after its full result is durable.
            // If that write fails, record a distinct failure event instead
            // of using the generic transition helper, whose terminal event
            // intentionally cannot represent Completed.
            let message = format!("Could not save the agent result: {terminal_error}");
            tracing::warn!(run = %run.id, %terminal_error, "could not persist terminal run event");
            status = RunStatus::Failed;
            reason = TerminalReason::Other;
            match append_stored_event(
                ctx,
                &run.id,
                "terminal-persistence-failure",
                StoredRunEvent::Terminated {
                    status,
                    reason,
                    message,
                },
            ) {
                Ok(run) => run,
                Err(fallback_error) => {
                    tracing::error!(run = %run.id, %fallback_error, "could not persist terminal fallback");
                    ctx.runs.finish(lease);
                    ctx.notice("The agent exited, but Perch could not save its final state");
                    ctx.send_threads(&item_id);
                    send_agent_runs(ctx, 50);
                    return;
                }
            }
        }
    };

    if final_run.status == RunStatus::Completed {
        let reply = CommentMessage {
            id: perch_github::comments::new_id(),
            thread_id: thread.id.clone(),
            origin: ThreadOrigin::Local,
            author: Author::Agent {
                backend: final_run.backend.clone(),
                model: final_run.model.clone(),
            },
            body: final_text,
            created_at: Timestamp::now(),
            run: Some(final_run.clone()),
        };
        match ctx.with_cache(|cache| cache.insert_completed_reply(&request_id, &reply)) {
            Some(Ok(())) => {}
            Some(Err(error)) => {
                tracing::error!(run = %final_run.id, %error, "could not attach completed answer to its discussion");
                ctx.notice(
                    "The agent completed, but Perch could not attach its answer to the discussion",
                );
            }
            None => ctx.notice(
                "The agent completed, but the local database is unavailable for its answer",
            ),
        }
    }
    let _ = ctx.events.send(Event::AgentRunUpdated {
        run: final_run.clone(),
    });
    ctx.runs.finish(lease);
    tracing::info!(thread = %thread.id, run = %final_run.id, status = final_run.status.name(), steps = final_run.steps.len(), "run ended");
    ctx.send_threads(&item_id);
    // Finding threads start with the reviewer's report, then use the ordinary
    // discussion flow for follow-ups. Keep the review archive in step with
    // those later replies without creating archives for ordinary questions.
    if matches!(
        thread.messages.first().map(|message| &message.author),
        Some(Author::Agent { .. })
    ) {
        reviews::export_item(ctx, &item_id);
    }
    send_agent_runs(ctx, 50);
}

fn run_labels(item: &Item) -> RunLabels {
    RunLabels {
        repository: item.repo_full_name.clone(),
        item: item.title().to_string(),
        branch: item.branch().map(str::to_string),
    }
}

fn append_stored_event(
    ctx: &Ctx,
    run_id: &str,
    event_id: &str,
    event: StoredRunEvent,
) -> Result<RunRecord, String> {
    let event = NewRunEvent {
        event_id: event_id.to_string(),
        occurred_at: Timestamp::now(),
        event,
        truncated: false,
    };
    let result = ctx
        .with_cache(|cache| cache.append_run_event(run_id, &event))
        .ok_or_else(|| "the local database is unavailable".to_string())?;
    result.map_err(|error| error.to_string())?;
    ctx.runs
        .load(run_id)
        .ok_or_else(|| format!("run {run_id} disappeared after its event was saved"))
}

/// Start another attempt for the same durable request. The old attempt and
/// all of its events remain in history; the discussion's request row points
/// at the new attempt while it is active.
type RetryConversationContext = (Vec<(Author, String)>, Option<String>);

fn captured_retry_prompt(
    mut reconstructed: crate::ask::Prompt,
    prompt_snapshot: Option<&str>,
) -> crate::ask::Prompt {
    // Reconstructing still selects the original resume/system behavior, but
    // the text sent to the CLI must be byte-for-byte what admission captured.
    // `None` is retained for pre-snapshot databases.
    if let Some(snapshot) = prompt_snapshot {
        reconstructed.user = snapshot.to_string();
    }
    reconstructed
}

fn retry_conversation_context(
    thread: &CommentThread,
    request_id: &str,
    backend: &str,
    purpose: RunPurpose,
) -> Option<RetryConversationContext> {
    let request_index = thread
        .messages
        .iter()
        .position(|message| message.id == request_id)?;
    if purpose == RunPurpose::Question {
        return Some((Vec::new(), None));
    }

    // A retry belongs at the same point in the conversation as its original
    // request. Later replies must neither enter its prompt nor choose the CLI
    // session it resumes.
    let prefix = &thread.messages[..request_index];
    let resume = prefix
        .iter()
        .rev()
        .filter_map(|message| message.run.as_ref())
        .find(|run| run.status == RunStatus::Completed)
        .filter(|run| run.backend == backend)
        .and_then(|run| run.session_id.clone());
    let history = if resume.is_some() {
        Vec::new()
    } else {
        prefix
            .iter()
            .filter_map(|message| {
                let body = if message.body.trim().is_empty() {
                    message
                        .run
                        .as_ref()
                        .and_then(|run| run.final_result.clone())
                        .unwrap_or_default()
                } else {
                    message.body.clone()
                };
                (!body.trim().is_empty()).then(|| (message.author.clone(), body))
            })
            .collect()
    };
    Some((history, resume))
}

async fn retry_run(ctx: &Ctx, old_run_id: &str) {
    let Some(old) = ctx.runs.load(old_run_id) else {
        ctx.notice("That agent run is no longer available");
        return;
    };
    if !matches!(
        old.status,
        RunStatus::Failed | RunStatus::Stopped | RunStatus::Interrupted
    ) {
        ctx.notice("Only a failed, stopped, or interrupted run can be retried");
        return;
    }
    let RunOwner::Discussion {
        item_id,
        thread_id,
        request_id,
    } = old.owner.clone()
    else {
        ctx.notice("This run has no retryable discussion owner");
        return;
    };
    let request_already_completed = ctx
        .with_cache(|cache| cache.runs_for_request(&thread_id, &request_id))
        .and_then(Result::ok)
        .is_some_and(|attempts| {
            attempts
                .iter()
                .any(|attempt| attempt.id != old.id && attempt.status == RunStatus::Completed)
        });
    if request_already_completed {
        ctx.notice("That request already has a completed answer");
        return;
    }
    let Some(backend) = perch_agent::Backend::parse(&old.backend) else {
        ctx.notice(format!("{} is no longer an available agent", old.backend));
        return;
    };
    let model = old.requested_model.clone();
    let configured = match backend {
        perch_agent::Backend::ClaudeCode => ctx
            .config
            .agent
            .models
            .iter()
            .any(|candidate| candidate == &model),
        perch_agent::Backend::Codex => ctx
            .config
            .agent
            .codex_models()
            .iter()
            .any(|candidate| candidate == &model),
    };
    if !configured {
        ctx.notice(format!(
            "{model} is no longer available for {}",
            old.backend
        ));
        return;
    }
    let Some(effort) = perch_agent::Effort::parse(&old.requested_effort) else {
        ctx.notice(format!(
            "{} is no longer a supported effort",
            old.requested_effort
        ));
        return;
    };
    if !ctx.config.agent.effort_supported(&model, effort.name()) {
        ctx.notice(format!(
            "{model} does not support {} effort",
            effort.label()
        ));
        return;
    }
    let Some(directory) = old.working_directory.as_deref().map(Path::new) else {
        ctx.notice("The original checkout is unavailable");
        return;
    };
    if !directory.join(".git").exists() {
        ctx.notice("The original checkout is unavailable");
        return;
    }
    let Some(item) = ctx.item(&item_id) else {
        ctx.notice("The owning item is gone");
        return;
    };
    let Some(thread) = ctx
        .with_cache(|cache| cache.load_comment_threads(&item_id).ok())
        .flatten()
        .and_then(|threads| threads.into_iter().find(|thread| thread.id == thread_id))
    else {
        ctx.notice("The owning discussion is gone");
        return;
    };
    let Some((history, resume)) =
        retry_conversation_context(&thread, &request_id, backend.name(), old.purpose)
    else {
        ctx.notice("The original request is gone");
        return;
    };
    let input = crate::ask::PromptInput {
        pull_request: item.pull_request(),
        issue: None,
        body: None,
        anchor: thread.anchor.as_ref(),
        file: None,
        outdated: thread.outdated,
        review_context: None,
        question: &old.original_request,
        history: &history,
    };
    let reconstructed_prompt = if old.purpose == RunPurpose::Question {
        crate::ask::first_prompt(&input)
    } else {
        crate::ask::follow_up_prompt(&input)
    };
    let prompt = captured_retry_prompt(reconstructed_prompt, old.prompt_snapshot.as_deref());
    let spec = crate::ask::run_spec(
        &ctx.config.agent,
        backend,
        &model,
        effort,
        directory.to_path_buf(),
        &prompt,
        resume,
    );
    let now = Timestamp::now();
    let run = RunRecord {
        id: perch_github::comments::new_id(),
        owner: RunOwner::Discussion {
            item_id: item_id.clone(),
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
        },
        purpose: old.purpose,
        backend: old.backend.clone(),
        executable: Some(ctx.config.agent.binary_for(backend.name()).to_string()),
        executable_version: ctx
            .config
            .agent
            .binary_version_for(backend.name())
            .map(str::to_string),
        requested_model: model.clone(),
        model,
        requested_effort: effort.name().into(),
        effort: effort.name().into(),
        session_id: None,
        original_request: old.original_request.clone(),
        prompt_snapshot: Some(prompt.user.clone()),
        working_directory: old.working_directory.clone(),
        revision: old.revision.clone(),
        labels: run_labels(&item),
        status: RunStatus::Queued,
        terminal_reason: None,
        cost_usd: None,
        duration_ms: None,
        usage: None,
        retry_of: Some(old.id.clone()),
        steps: Vec::new(),
        error: None,
        queued_at: now,
        started_at: None,
        ended_at: None,
        final_result: None,
        final_result_truncated: false,
        latest_activity: Some("Waiting for an execution slot".into()),
        last_event_sequence: 0,
    };
    let admission = match ctx.runs.admit(run.clone()) {
        Ok(admission) => admission,
        Err(message) => {
            ctx.notice(message);
            return;
        }
    };
    // Admission moved the request link and preserved its previous attempt in
    // the same transaction as the queued row and event.
    ctx.send_threads(&item_id);
    execute_question(ctx, item_id, thread, request_id, run, spec, admission).await;
}

async fn delete_comment_thread(ctx: &Ctx, id: &str) {
    // Stop and reap before deleting the request rows. That prevents a late
    // process event from recreating an orphaned discussion.
    let item_id = ctx
        .with_cache(|cache| cache.comment_thread_item_id(id).ok().flatten())
        .flatten();
    if let Some(item_id) = item_id.as_deref() {
        let durable_finding = ctx
            .with_cache(|cache| {
                cache
                    .list_findings(item_id, None)
                    .map(|findings| findings.iter().any(|finding| finding.thread_id == id))
            })
            .and_then(Result::ok)
            .unwrap_or(false);
        if durable_finding {
            ctx.notice(
                "Review finding threads stay in history. Dismiss the finding instead of deleting it.",
            );
            return;
        }
    }
    let deletion = match item_id.as_deref() {
        Some(item_id) => match ctx.runs.block_owner(item_id, Some(id)) {
            Ok(deletion) => Some(deletion),
            Err(message) => {
                ctx.notice(message);
                return;
            }
        },
        None => None,
    };
    if let Some(item_id) = item_id.as_deref() {
        ctx.runs.cancel_owner(item_id, Some(id)).await;
    }
    if let Some(Err(e)) = ctx.with_cache(|c| c.delete_comment_thread(id)) {
        let _ = ctx
            .events
            .send(Event::Notice(format!("Could not delete the thread: {e}")));
        return;
    }
    drop(deletion);
    if let Some(item_id) = item_id {
        ctx.send_threads(&item_id);
    }
}

/// A checkout for a worktree already on disk, without touching the network.
///
/// The head is whatever the worktree is at, and the merge base comes from
/// the base ref the last fetch left in the bare clone.
async fn reopen_from_disk(
    ctx: &Ctx,
    repo: &RepoId,
    number: u64,
    branch: &str,
    path: Option<std::path::PathBuf>,
) -> Result<perch_git::Checkout, perch_git::Error> {
    let store = ctx.preparer.store();
    let worktree = path.unwrap_or_else(|| store.worktree_path(repo, branch));
    let bare = store.bare_path(repo);
    let git = ctx.preparer.git();
    let timeout = Duration::from_secs(30);

    let head_sha = git
        .run(Some(&worktree), ["rev-parse", "HEAD"], timeout)
        .await?
        .trim()
        .to_string();
    let base = format!("refs/perch/base/{number}");
    let merge_base = git
        .run(Some(&bare), ["merge-base", &base, &head_sha], timeout)
        .await?
        .trim()
        .to_string();
    if merge_base.is_empty() {
        return Err(perch_git::Error::NoMergeBase);
    }

    Ok(perch_git::Checkout {
        worktree,
        bare,
        branch: branch.to_string(),
        head_sha,
        merge_base,
        stale: None,
    })
}

/// The pull request's file list, from a local diff rather than the API.
async fn open_file(
    preparer: Arc<Preparer>,
    slot: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    events: EventSender<Event>,
    navigation_requests: Arc<NavigationRequests>,
    path: String,
    navigation_id: u64,
) -> bool {
    let Some((checkout, changed, scope, item_id, scope_gen)) = navigation_requests
        .with_current(navigation_id, || {
            slot.lock().ok().and_then(|guard| {
                guard
                    .as_ref()
                    .filter(|context| context.navigation_id == navigation_id)
                    .map(|context| {
                        (
                            context.checkout.clone(),
                            context.files.clone(),
                            context.scope.clone(),
                            context.item_id.clone(),
                            context.scope_gen,
                        )
                    })
            })
        })
        .flatten()
    else {
        return false;
    };

    let entry = changed.iter().find(|f| f.path == path).cloned();
    let (base_rev, head_rev) = scope.sides(&checkout.merge_base, &checkout.head_sha);
    let git = preparer.git();

    // Both sides of a changed file come from the scope: a commit's tree,
    // the index, or the disk. A file outside the changed list is the same
    // under every scope, so it is read from disk, where it may be binary,
    // untracked or too big, which the changed list would have said.
    let head = match &entry {
        Some(f) if f.status == perch_git::ChangeStatus::Deleted => String::new(),
        Some(_) => match perch_git::read_side(git, &checkout.worktree, &head_rev, &path).await {
            Ok(Some(text)) if text.len() > MAX_FILE_BYTES => {
                send_file_event_if_current(
                    &navigation_requests,
                    &slot,
                    &events,
                    navigation_id,
                    &item_id,
                    scope_gen,
                    Event::FileFailed {
                        navigation_id,
                        scope_gen,
                        item_id: item_id.clone(),
                        path,
                        message: "file is too large to display".into(),
                    },
                );
                return false;
            }
            Ok(Some(text)) => text,
            Ok(None) => {
                send_file_event_if_current(
                    &navigation_requests,
                    &slot,
                    &events,
                    navigation_id,
                    &item_id,
                    scope_gen,
                    Event::FileFailed {
                        navigation_id,
                        scope_gen,
                        item_id: item_id.clone(),
                        path,
                        message: "file is not there".into(),
                    },
                );
                return false;
            }
            Err(e) => {
                send_file_event_if_current(
                    &navigation_requests,
                    &slot,
                    &events,
                    navigation_id,
                    &item_id,
                    scope_gen,
                    Event::FileFailed {
                        navigation_id,
                        scope_gen,
                        item_id: item_id.clone(),
                        path,
                        message: e.to_string(),
                    },
                );
                return false;
            }
        },
        None => match tokio::fs::read(checkout.worktree.join(&path)).await {
            Ok(bytes) if bytes.len() > MAX_FILE_BYTES => {
                send_file_event_if_current(
                    &navigation_requests,
                    &slot,
                    &events,
                    navigation_id,
                    &item_id,
                    scope_gen,
                    Event::FileFailed {
                        navigation_id,
                        scope_gen,
                        item_id: item_id.clone(),
                        path,
                        message: "file is too large to display".into(),
                    },
                );
                return false;
            }
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => {
                    send_file_event_if_current(
                        &navigation_requests,
                        &slot,
                        &events,
                        navigation_id,
                        &item_id,
                        scope_gen,
                        Event::FileFailed {
                            navigation_id,
                            scope_gen,
                            item_id: item_id.clone(),
                            path,
                            message: "file is not text".into(),
                        },
                    );
                    return false;
                }
            },
            Err(e) => {
                send_file_event_if_current(
                    &navigation_requests,
                    &slot,
                    &events,
                    navigation_id,
                    &item_id,
                    scope_gen,
                    Event::FileFailed {
                        navigation_id,
                        scope_gen,
                        item_id: item_id.clone(),
                        path,
                        message: e.to_string(),
                    },
                );
                return false;
            }
        },
    };
    let base = match &entry {
        None
        | Some(perch_git::ChangedFile {
            status: perch_git::ChangeStatus::Added,
            ..
        }) => String::new(),
        Some(f) => {
            let source = f.old_path.clone().unwrap_or_else(|| f.path.clone());
            perch_git::read_side(git, &checkout.worktree, &base_rev, &source)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        }
    };

    let status = entry.as_ref().map(|f| f.status);
    let binary = entry.as_ref().map(|f| f.binary).unwrap_or(false);
    // The head blob keys the reveals, as it keys Viewed. A file outside
    // the changed list has none, and neither has a deletion or a working
    // tree file git has not hashed; their reveals live and die with the
    // window.
    let blob = entry
        .as_ref()
        .and_then(|f| f.head_blob.clone())
        .unwrap_or_default();

    // Parsing and diffing are CPU work; keep them off the async runtime's
    // reactor threads.
    let loaded = tokio::task::spawn_blocking(move || {
        build_loaded_file(path, head, base, status, binary, scope, blob)
    })
    .await;

    match loaded {
        Ok(file) => {
            tracing::info!(
                path = %file.path,
                lines = file.buffer.len(),
                changes = file.buffer.change_starts.len(),
                scope = %file.scope,
                "file loaded"
            );
            send_file_event_if_current(
                &navigation_requests,
                &slot,
                &events,
                navigation_id,
                &item_id,
                scope_gen,
                Event::FileLoaded {
                navigation_id,
                scope_gen,
                item_id: item_id.clone(),
                path: file.path.clone(),
                file: Arc::new(file),
                },
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "file load panicked");
            false
        }
    }
}

fn send_file_event_if_current(
    navigation_requests: &NavigationRequests,
    slot: &std::sync::Mutex<Option<ReviewContext>>,
    events: &EventSender<Event>,
    navigation_id: u64,
    item_id: &str,
    scope_gen: u64,
    event: Event,
) -> bool {
    navigation_requests
        .with_current(navigation_id, || {
            let current = slot.lock().ok().is_some_and(|guard| {
                guard.as_ref().is_some_and(|context| {
                    context.navigation_id == navigation_id
                        && context.item_id == item_id
                        && context.scope_gen == scope_gen
                })
            });
            if current {
                let _ = events.send(event);
            }
            current
        })
        .unwrap_or(false)
}

/// Start a language server for the open checkout, if one is installed.
async fn start_language_server(
    checkout: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    slot: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    events: EventSender<Event>,
    navigation_requests: Arc<NavigationRequests>,
    navigation_id: u64,
) {
    let Some(worktree) = checkout.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|context| context.navigation_id == navigation_id)
            .map(|context| context.checkout.worktree.clone())
    }) else {
        return;
    };

    let Some(spec) = perch_lsp::find_server("rust") else {
        send_lsp_status_if_current(
            &navigation_requests,
            &checkout,
            &events,
            navigation_id,
            perch_lsp::Status::Absent,
        );
        return;
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let events = events.clone();
        let checkout = checkout.clone();
        let navigation_requests = navigation_requests.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    perch_lsp::client::ServerEvent::Status(status) => {
                        tracing::debug!(status = %status.describe(), "language server");
                        send_lsp_status_if_current(
                            &navigation_requests,
                            &checkout,
                            &events,
                            navigation_id,
                            status,
                        );
                    }
                    // Diagnostics are collected but not yet drawn; the editor
                    // needs a decoration pass this milestone does not have.
                    perch_lsp::client::ServerEvent::Diagnostics { .. } => {}
                }
            }
        });
    }

    match perch_lsp::Client::start(spec, &worktree, tx).await {
        Ok(client) => {
            let mut language_server = slot.lock().await;
            let current_context = navigation_requests
                .with_current(navigation_id, || {
                    checkout.lock().ok().is_some_and(|context| {
                        context
                            .as_ref()
                            .is_some_and(|context| context.navigation_id == navigation_id)
                    })
                })
                .unwrap_or(false);
            if current_context {
                tracing::info!("language server started");
                let previous = language_server.replace(client);
                drop(language_server);
                if let Some(previous) = previous {
                    previous.shutdown().await;
                }
            } else {
                drop(language_server);
                client.shutdown().await;
            }
        }
        Err(e) => {
            send_lsp_status_if_current(
                &navigation_requests,
                &checkout,
                &events,
                navigation_id,
                perch_lsp::Status::Failed {
                    reason: e.to_string(),
                },
            );
        }
    }
}

fn send_lsp_status_if_current(
    navigation_requests: &NavigationRequests,
    checkout: &std::sync::Mutex<Option<ReviewContext>>,
    events: &EventSender<Event>,
    navigation_id: u64,
    status: perch_lsp::Status,
) {
    let _ = navigation_requests.with_current(navigation_id, || {
        let current = checkout.lock().ok().is_some_and(|guard| {
            guard
                .as_ref()
                .is_some_and(|context| context.navigation_id == navigation_id)
        });
        if current {
            let _ = events.send(Event::LspStatus {
                navigation_id,
                status,
            });
        }
    });
}

/// Tell the language server about a file the user just opened.
async fn announce_file(
    checkout: Arc<std::sync::Mutex<Option<ReviewContext>>>,
    slot: Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    navigation_requests: Arc<NavigationRequests>,
    path: String,
    navigation_id: u64,
) {
    let Some(worktree) = navigation_requests
        .with_current(navigation_id, || {
            checkout.lock().ok().and_then(|guard| {
                guard
                    .as_ref()
                    .filter(|context| context.navigation_id == navigation_id)
                    .map(|context| context.checkout.worktree.clone())
            })
        })
        .flatten()
    else {
        return;
    };
    let Some(client) = slot.lock().await.clone() else {
        return;
    };
    let full = worktree.join(path);
    if let Ok(text) = tokio::fs::read_to_string(&full).await {
        let still_current = navigation_requests
            .with_current(navigation_id, || {
                checkout.lock().ok().is_some_and(|guard| {
                    guard
                        .as_ref()
                        .is_some_and(|context| context.navigation_id == navigation_id)
                })
            })
            .unwrap_or(false);
        if still_current {
            client.did_open(&full, &text).await;
        }
    }
}

/// The server and the absolute path for a file of the open checkout.
async fn resolve(
    checkout: &Arc<std::sync::Mutex<Option<ReviewContext>>>,
    slot: &Arc<tokio::sync::Mutex<Option<Arc<perch_lsp::Client>>>>,
    path: &str,
) -> Option<(Arc<perch_lsp::Client>, std::path::PathBuf)> {
    let client = slot.lock().await.clone()?;
    let worktree = checkout
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.checkout.worktree.clone()))?;
    Some((client, worktree.join(path)))
}

/// Each location inside the worktree with its line's text, read here so
/// the panel needs no file loads, and a count of those outside it, in a
/// dependency, which the rail could not open. Files are read once each.
async fn read_references(
    worktree: &Path,
    locations: &[perch_lsp::lsp_types::Location],
) -> (Vec<crate::review::model::Reference>, usize) {
    let mut texts: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    let mut elsewhere = 0;
    for location in locations {
        let target = perch_lsp::client::uri_to_path(&location.uri.to_string());
        let Some(relative) = target.and_then(|p| {
            p.strip_prefix(worktree)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        }) else {
            elsewhere += 1;
            continue;
        };
        if !texts.contains_key(&relative) {
            let lines = tokio::fs::read_to_string(worktree.join(&relative))
                .await
                .map(|t| t.lines().map(str::to_string).collect())
                .unwrap_or_default();
            texts.insert(relative.clone(), lines);
        }
        let line = location.range.start.line as usize;
        let text = texts[&relative]
            .get(line)
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        out.push(crate::review::model::Reference {
            path: relative,
            line: line + 1,
            character: location.range.start.character as usize,
            text,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    out.dedup();
    (out, elsewhere)
}

/// Flatten a hover reply into the plain text the popup shows.
fn render_hover(hover: &perch_lsp::lsp_types::Hover) -> String {
    use perch_lsp::lsp_types::{HoverContents, MarkedString};
    match &hover.contents {
        HoverContents::Scalar(MarkedString::String(s)) => s.clone(),
        HoverContents::Scalar(MarkedString::LanguageString(s)) => s.value.clone(),
        HoverContents::Markup(m) => m.value.clone(),
        HoverContents::Array(items) => items
            .iter()
            .map(|i| match i {
                MarkedString::String(s) => s.clone(),
                MarkedString::LanguageString(s) => s.value.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// A file as the editor wants it: the two sides diffed and highlighted,
/// with the head blob the reveals are keyed by, empty when the file has
/// none.
pub(crate) fn build_loaded_file(
    path: String,
    head: String,
    base: String,
    status: Option<perch_git::ChangeStatus>,
    binary: bool,
    scope: perch_git::Scope,
    blob: String,
) -> crate::review::model::LoadedFile {
    use perch_git::{file_diff, DisplayBuffer};
    use perch_syntax::{highlight, Language, LineSpans};

    let language = perch_syntax::Language::from_path(&path);

    if binary {
        return crate::review::model::LoadedFile {
            path,
            language: None,
            buffer: DisplayBuffer::plain("This file is binary and has no text to show."),
            head_spans: LineSpans::default(),
            base_spans: LineSpans::default(),
            status,
            truncated: true,
            scope,
            base_buffer: None,
            alignment: Vec::new(),
            blob,
        };
    }

    // A changed file is diffed for the stacked view, and the same diff
    // gives the split view its base side and its row pairing. A file
    // outside the list is shown plainly, which is what makes browsing the
    // whole tree useful during a review.
    let (buffer, base_buffer, alignment) = match status {
        Some(_) => {
            let diff = file_diff(&base, &head);
            let alignment = perch_git::alignment(&diff, &base, &head);
            let base_buffer = (!base.is_empty()).then(|| DisplayBuffer::plain(&base));
            (
                DisplayBuffer::unified(&head, &base, &diff),
                base_buffer,
                alignment,
            )
        }
        None => (DisplayBuffer::plain(&head), None, Vec::new()),
    };

    let spans_for = |text: &str, lang: Option<Language>| match lang {
        Some(lang) if !text.is_empty() => LineSpans::new(text, &highlight(lang, text)),
        _ => LineSpans::default(),
    };

    crate::review::model::LoadedFile {
        head_spans: spans_for(&head, language),
        base_spans: spans_for(&base, language),
        path,
        language,
        buffer,
        status,
        truncated: false,
        scope,
        base_buffer,
        alignment,
        blob,
    }
}

type SharedCache = Option<Arc<std::sync::Mutex<Cache>>>;

fn open_cache_for_app(events: &EventSender<Event>) -> (SharedCache, bool) {
    let Some(path) = perch_github::config::db_path() else {
        return (None, false);
    };
    let avatars = perch_github::config::avatar_dir();
    match Cache::open_with_outcome(&path, avatars) {
        Ok((cache, outcome)) => {
            if let CacheOpen::Migrated { from, to } = outcome {
                let _ = events.send(Event::Notice(format!(
                    "Updated the local database from schema {from} to {to}; discussions and agent history were preserved"
                )));
            }
            (Some(Arc::new(std::sync::Mutex::new(cache))), false)
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "running without a local cache");
            let _ = events.send(Event::Notice(format!(
                "Could not open the local database: {error}. The existing file was left unchanged."
            )));
            (None, false)
        }
    }
}

fn spawn_avatar(
    github: Option<Arc<GitHub>>,
    cache: SharedCache,
    events: EventSender<Event>,
    limit: Arc<Semaphore>,
    url: String,
) {
    // Disk first. Avatars change rarely and a hit costs no request.
    if let Some(cache) = cache.as_ref() {
        if let Ok(guard) = cache.lock() {
            if let Some(bytes) = guard.load_avatar(&url) {
                tracing::debug!(url, bytes = bytes.len(), "avatar from disk");
                let _ = events.send(Event::Avatar { url, bytes });
                return;
            }
        }
    }

    let Some(github) = github else { return };

    tokio::spawn(async move {
        let Ok(_permit) = limit.acquire().await else {
            return;
        };
        match github.fetch_avatar(&url).await {
            Ok(bytes) => {
                if let Some(cache) = cache.as_ref() {
                    if let Ok(guard) = cache.lock() {
                        let _ = guard.store_avatar(&url, &bytes);
                    }
                }
                let _ = events.send(Event::Avatar { url, bytes });
            }
            Err(e) => tracing::debug!(error = %e, url, "avatar fetch failed"),
        }
    });
}

fn describe_error(e: &perch_github::Error) -> Status {
    match e {
        perch_github::Error::Unauthorized => Status::NoToken {
            detail: "GitHub rejected the token. It may be expired, or missing the \
                     notifications scope."
                .into(),
        },
        perch_github::Error::RateLimited {
            reset_epoch,
            retry_after_secs,
        } => Status::RateLimited {
            message: match rate_limit_retry_epoch(*reset_epoch, *retry_after_secs)
                .and_then(local_clock_time)
            {
                Some(t) => format!("Rate limited. Retrying after {t}."),
                None => "Rate limited. Retrying shortly.".into(),
            },
        },
        other => Status::Error {
            message: other.to_string(),
        },
    }
}

/// Wall-clock time in the user's zone, for the status bar.
pub(crate) fn local_clock_time(epoch: i64) -> Option<String> {
    let ts = Timestamp::from_second(epoch).ok()?;
    let zoned = ts.to_zoned(jiff::tz::TimeZone::system());
    Some(format!("{:02}:{:02}", zoned.hour(), zoned.minute()))
}

fn rate_limit_retry_epoch(reset_epoch: Option<i64>, retry_after_secs: Option<u64>) -> Option<i64> {
    retry_after_secs
        .and_then(|seconds| i64::try_from(seconds).ok())
        .map(|seconds| Timestamp::now().as_second().saturating_add(seconds))
        .or(reset_epoch)
}

/// How long to wait out a rate limit. Server-provided deadlines are
/// authoritative; the one-minute floor is only a fallback for missing or
/// already-passed reset information.
fn backoff_until(reset_epoch: Option<i64>, retry_after_secs: Option<u64>) -> Duration {
    const MIN: u64 = 60;
    if let Some(seconds) = retry_after_secs {
        return Duration::from_secs(seconds.max(1));
    }
    let now = Timestamp::now().as_second();
    match reset_epoch {
        Some(reset) if reset > now => Duration::from_secs((reset - now) as u64),
        _ => Duration::from_secs(MIN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_scope_request_can_commit() {
        let requests = ScopeRequests::default();
        let before = requests.stable_revision().unwrap();
        let branch = requests.begin("item");
        let commit = requests.begin("item");
        let branch_committed = std::cell::Cell::new(false);
        let commit_committed = std::cell::Cell::new(false);

        assert!(requests
            .with_current("item", branch, || branch_committed.set(true))
            .is_none());
        assert!(requests
            .with_current("item", commit, || commit_committed.set(true))
            .is_some());
        assert!(!branch_committed.get());
        assert!(commit_committed.get());
        assert!(requests.stable_revision().is_none());
        assert!(requests
            .with_revision(commit, || "pending refresh")
            .is_none());
        requests.settle("item", commit);
        assert_eq!(requests.stable_revision(), Some(commit));
        assert_eq!(
            requests.with_revision(commit, || "settled refresh"),
            Some("settled refresh")
        );
        assert!(requests.with_revision(before, || "stale refresh").is_none());

        let before_clear = requests.stable_revision().unwrap();
        requests.clear();
        assert!(!requests.is_current("item", commit));
        assert!(requests
            .with_revision(before_clear, || "stale refresh")
            .is_none());
    }

    #[test]
    fn leaving_the_branch_scope_only_hides_review_finding_anchors() {
        let make_thread = |id: &str| CommentThread {
            id: id.into(),
            worktree_id: "item".into(),
            origin: ThreadOrigin::Local,
            anchor: None,
            outdated: false,
            resolved: false,
            created_at: Timestamp::now(),
            messages: Vec::new(),
        };
        let mut threads = vec![make_thread("review"), make_thread("question")];
        let finding_threads = std::collections::HashSet::from(["review".to_string()]);

        Ctx::mark_review_threads_outdated(&mut threads, &finding_threads);

        assert!(threads[0].outdated);
        assert!(!threads[1].outdated);
    }

    #[test]
    fn backoff_has_a_floor_when_the_reset_is_past() {
        let past = Timestamp::now().as_second() - 500;
        assert_eq!(backoff_until(Some(past), None), Duration::from_secs(60));
    }

    #[test]
    fn backoff_honors_a_far_future_reset() {
        let far = Timestamp::now().as_second() + 86_400;
        assert!(backoff_until(Some(far), None) >= Duration::from_secs(86_399));
    }

    #[test]
    fn backoff_honors_a_near_future_reset_without_overshooting_it() {
        let reset = Timestamp::now().as_second() + 5;
        let wait = backoff_until(Some(reset), None);
        assert!(wait >= Duration::from_secs(1));
        assert!(wait <= Duration::from_secs(5));
    }

    #[test]
    fn backoff_without_a_reset_header_still_waits() {
        assert_eq!(backoff_until(None, None), Duration::from_secs(60));
    }

    #[test]
    fn retry_after_takes_priority_over_the_primary_reset() {
        let reset = Timestamp::now().as_second() + 60;
        assert_eq!(
            backoff_until(Some(reset), Some(3_600)),
            Duration::from_secs(3_600)
        );
    }

    #[test]
    fn retry_replays_the_captured_prompt_instead_of_rebuilt_context() {
        let rebuilt = crate::ask::Prompt {
            system: Some("the original system behavior".into()),
            user: "a prompt rebuilt from changed source and thread state".into(),
        };
        let replayed = captured_retry_prompt(
            rebuilt,
            Some("the exact prompt captured when the first attempt was admitted"),
        );

        assert_eq!(
            replayed.user,
            "the exact prompt captured when the first attempt was admitted"
        );
        assert_eq!(
            replayed.system.as_deref(),
            Some("the original system behavior"),
            "snapshot replay must preserve question/follow-up system behavior"
        );
    }

    #[test]
    fn a_rate_limited_action_defers_automatic_and_manual_polls() {
        let now = tokio::time::Instant::now();
        let mut automatic = now + Duration::from_secs(10);
        let mut manual = now + Duration::from_secs(10);
        defer_polls_for_error(
            &perch_github::Error::RateLimited {
                reset_epoch: None,
                retry_after_secs: Some(3_600),
            },
            &mut automatic,
            &mut manual,
        );
        assert!(automatic >= now + Duration::from_secs(3_600));
        assert!(manual >= now + Duration::from_secs(3_600));
    }

    #[test]
    fn an_early_manual_refresh_is_queued_for_the_server_deadline() {
        let now = tokio::time::Instant::now();
        let allowed = now + Duration::from_secs(60);
        let mut automatic = now + Duration::from_secs(300);
        queue_manual_refresh(&mut automatic, allowed);
        assert_eq!(automatic, allowed);

        let already_sooner = now + Duration::from_secs(30);
        automatic = already_sooner;
        queue_manual_refresh(&mut automatic, allowed);
        assert_eq!(automatic, already_sooner);
    }

    #[test]
    fn an_unauthorized_error_asks_for_a_token_rather_than_showing_a_stack() {
        let s = describe_error(&perch_github::Error::Unauthorized);
        assert!(matches!(s, Status::NoToken { .. }));
    }

    #[test]
    fn a_rate_limit_error_reports_when_it_lifts() {
        let s = describe_error(&perch_github::Error::RateLimited {
            reset_epoch: Some(0),
            retry_after_secs: None,
        });
        match s {
            Status::RateLimited { message } => assert!(message.contains("Retrying after")),
            other => panic!("expected a rate limit status, got {other:?}"),
        }
    }

    use perch_github::{Reason, Repository, Subject, SubjectKind};

    fn thread(id: &str, title: &str) -> Thread {
        Thread {
            id: id.into(),
            unread: true,
            local_unread: false,
            reason: Reason::Subscribed,
            updated_at: "2026-09-17T18:00:00Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: title.into(),
                kind: SubjectKind::PullRequest,
                url: Some(format!("https://api.github.com/repos/a/b/pulls/{id}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "a/b".into(),
                owner_avatar_url: String::new(),
            },
        }
    }

    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::Receiver;
    use std::sync::Mutex;

    /// A GitHub that answers every request the same way and writes down
    /// the paths it was asked, so a test can say what the code asked for
    /// and what it did not. The listener lives as long as the test binary
    /// does.
    fn serve(status: &'static str, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_by_path(move |_| (status, body.to_string()))
    }

    /// A GitHub that answers each request by its path.
    fn serve_by_path(
        answer: impl Fn(&str) -> (&'static str, String) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_by_request(move |path, _headers| answer(path))
    }

    fn serve_by_request(
        answer: impl Fn(&str, &[String]) -> (&'static str, String) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = paths.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let Some((path, headers)) = request(&mut stream) else {
                    continue;
                };
                let (status, body) = answer(&path, &headers);
                seen.lock().unwrap().push(path);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, paths)
    }

    /// The path of one GET, read past its headers so the client sees the
    /// answer as the answer to it.
    fn request(stream: &mut TcpStream) -> Option<(String, Vec<String>)> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut start = String::new();
        reader.read_line(&mut start).ok()?;
        let path = start.split_whitespace().nth(1)?.to_string();
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 || line.trim_end().is_empty() {
                break;
            }
            headers.push(line.trim_end().to_string());
        }
        Some((path, headers))
    }

    /// A base nothing listens on, so a request the code should not make
    /// is refused at once instead of reaching GitHub.
    fn nowhere() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    /// A context with no window behind it: a client at `base`, a cache,
    /// and the events kept so a test can say what the sidebar was told.
    fn ctx(dir: &Path, base: &str, declared: &[&str]) -> (Ctx, Receiver<Event>) {
        ctx_declaring(
            dir,
            base,
            declared.iter().map(|name| github_repo(name)).collect(),
        )
    }

    /// A `[[repo]]` with `github = "owner/repo"`, resolved from the file.
    /// Field by field: the entry's identity is a field of its own, and a
    /// private one, so the struct cannot be spelled out from here.
    fn github_repo(slug: &str) -> perch_github::RepoConfig {
        let mut repo = perch_github::RepoConfig::default();
        repo.github = Some(slug.to_string());
        repo
    }

    /// A `[[repo]]` with roots and no GitHub remote, resolved the way the
    /// startup scan resolves one: `local/<name>`.
    fn local_repo(name: &str) -> perch_github::RepoConfig {
        let mut repo = perch_github::RepoConfig::default();
        repo.roots = vec![format!("/x/{name}")];
        repo.resolve(None);
        repo
    }

    fn ctx_declaring(
        dir: &Path,
        base: &str,
        repos: Vec<perch_github::RepoConfig>,
    ) -> (Ctx, Receiver<Event>) {
        let (events, received) = std::sync::mpsc::channel();
        let cache = Cache::open(&dir.join("perch.db"), None).unwrap();
        let mut config = Config::default();
        config.repo = repos;
        let cache = Some(Arc::new(std::sync::Mutex::new(cache)));
        let runs = runs::RunManager::new(4, cache.clone(), events.clone());
        let ctx = Ctx {
            github: Some(Arc::new(GitHub::with_base_uri("token", base).unwrap())),
            preparer: Arc::new(Preparer::new(Store::new(dir.join("store")), None)),
            cache,
            config: Arc::new(config),
            slot: Arc::new(std::sync::Mutex::new(None)),
            language_server: Arc::new(tokio::sync::Mutex::new(None)),
            events,
            runs,
            review_capabilities: Arc::new(Vec::new()),
            review_snapshots: Arc::new(ReviewSnapshotStore::new(dir.join("store"))),
            navigation_requests: Arc::default(),
            scope_requests: Arc::default(),
            status_requests: Arc::default(),
            merge_attempts: Arc::default(),
            review_navigation: Arc::new(tokio::sync::Mutex::new(())),
            review_admissions: Default::default(),
            review_reconcile: Arc::new(tokio::sync::Mutex::new(())),
            knowledge: None,
            item_lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            avatars: Arc::new(Semaphore::new(1)),
            seen_notifications: Default::default(),
            refreshing: Default::default(),
            notifications: Default::default(),
        };
        (ctx, received)
    }

    #[test]
    fn a_new_historical_request_fences_older_groups_and_repeated_loads() {
        let requests = NavigationRequests::default();
        let first = requests.begin_group("item", "group-a");
        assert!(requests.is_current_group("group-a", first));

        let second = requests.begin_group("item", "group-b");
        assert!(!requests.is_current_group("group-a", first));
        assert!(requests.is_current_group("group-b", second));

        let repeated = requests.begin_group("item", "group-b");
        assert!(!requests.is_current_group("group-b", second));
        assert!(requests.is_current_group("group-b", repeated));
        assert_eq!(
            requests.with_current_group("group-b", repeated, || "installed"),
            Some("installed")
        );
        assert_eq!(
            requests.with_current_group("group-a", first, || "stale"),
            None
        );

        let current = requests.begin_current();
        assert!(!requests.is_current_group("group-b", repeated));
        assert!(requests.is_current(current));
        assert_eq!(
            requests.with_current_group("group-b", repeated, || "stale"),
            None
        );
    }

    #[test]
    fn out_of_order_reviewer_navigation_cannot_rebind_a_stale_attempt() {
        let requests = NavigationRequests::default();
        let first = requests.begin_attempt("item", "attempt-a");
        let latest = requests.begin_attempt("item", "attempt-b");

        assert!(!requests.is_current_attempt("attempt-a", first));
        assert!(!requests.bind_attempt_to_group("attempt-a", "group-a", first));
        assert!(requests.bind_attempt_to_group("attempt-b", "group-b", latest));
        assert!(!requests.is_current_group("group-a", first));
        assert!(requests.is_current_group("group-b", latest));
    }

    #[test]
    fn opening_an_ordinary_agent_owner_cancels_a_historical_load() {
        let backend = Backend::detached();
        let request = backend
            .navigation_requests
            .begin_group("item", "group-a");
        assert!(backend
            .navigation_requests
            .is_current_group("group-a", request));

        backend.view_agent_owner("item");

        assert!(!backend
            .navigation_requests
            .is_current_group("group-a", request));
    }

    #[test]
    fn newer_history_and_close_fence_older_current_navigation() {
        let requests = NavigationRequests::default();
        let current = requests.begin_current();
        let history = requests.begin_group("item", "group");

        assert!(requests.with_current(current, || "stale current").is_none());
        assert_eq!(
            requests.with_current_group("group", history, || "history"),
            Some("history")
        );

        let closed = requests.begin_closed();
        assert!(requests
            .with_current_group("group", history, || "stale history")
            .is_none());
        assert_eq!(requests.with_current(closed, || "closed"), Some("closed"));
    }

    #[test]
    fn failed_history_restores_the_exact_stable_navigation() {
        let requests = NavigationRequests::default();
        let live = requests.begin_item("item");
        let first = requests.begin_group("item", "group-a");
        let repeated = requests.begin_group("item", "group-a");

        assert!(!requests.is_current(first));
        assert!(requests.rollback_current_group("group-a", repeated));
        assert!(requests.is_current(live));

        let stable_history = requests.begin_group("item", "group-a");
        assert_eq!(
            requests.commit_current_group("group-a", stable_history, || Some("installed")),
            Some("installed")
        );
        let failing_history = requests.begin_group("item", "group-b");
        assert!(requests.rollback_current_group("group-b", failing_history));
        assert!(requests.is_current_group("group-a", stable_history));
    }

    #[test]
    fn archive_suspension_synchronously_fences_and_can_restore_same_item_navigation() {
        let requests = NavigationRequests::default();
        let request = requests.begin_item("item");
        let other = requests.suspend_item("other");
        assert!(requests.is_current(request));
        assert!(!requests.restore_suspended(&other));

        let suspended = requests.suspend_item("item");
        assert!(requests.is_current(request));
        assert!(requests.is_current_item("item", request));
        assert!(requests.commit_archive_fence(&suspended));
        assert!(!requests.is_current(request));
        assert!(requests.restore_suspended(&suspended));
        assert!(requests.is_current(request));
        assert!(!requests.restore_suspended(&suspended));
    }

    #[test]
    fn failed_attempt_history_retains_a_newly_opened_owner() {
        let requests = NavigationRequests::default();
        requests.begin_item("old-owner");
        let attempt = requests.begin_attempt("new-owner", "attempt");
        assert!(requests.checkpoint_attempt_owner("new-owner", "attempt", attempt));
        assert!(requests.bind_attempt_to_group("attempt", "group", attempt));
        assert!(requests.rollback_current_group("group", attempt));
        assert!(requests.is_current(attempt));
        let suspended = requests.suspend_item("new-owner");
        assert!(requests.commit_archive_fence(&suspended));
        assert!(!requests.is_current(attempt));
    }

    #[test]
    fn browser_only_notification_preserves_backend_navigation_identity() {
        let backend = Backend::detached();
        let current = backend.open_item("item");
        let mut browser_only = thread("7", "build result");
        browser_only.subject.kind = SubjectKind::CheckSuite;

        assert_eq!(backend.open_notification(&browser_only), None);
        assert!(backend.navigation_requests.is_current(current));
    }

    #[tokio::test]
    async fn a_local_unread_reminder_survives_matching_snapshots_and_clears_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        let last_read_at: Timestamp = "2026-09-17T17:00:00Z".parse().unwrap();
        let mut read = thread("7", "seven");
        read.unread = false;
        read.last_read_at = Some(last_read_at);
        ctx.replace_notifications(vec![read.clone()], Some("first"));

        ctx.mark_notification_unread("7");

        let marked = ctx.notifications.lock().unwrap()[0].clone();
        assert!(marked.unread);
        assert!(marked.local_unread);
        assert_eq!(marked.last_read_at, Some(last_read_at));
        assert_eq!(
            ctx.with_cache(|cache| cache.load_threads().unwrap())
                .unwrap(),
            vec![marked.clone()],
            "the local reminder survives a restart"
        );
        let published = received
            .try_iter()
            .filter_map(|event| match event {
                Event::Notifications(threads) => Some(threads),
                _ => None,
            })
            .last()
            .unwrap();
        assert_eq!(published, vec![marked]);

        ctx.replace_notifications(vec![read.clone()], Some("second"));
        let matching = ctx.notifications.lock().unwrap()[0].clone();
        assert!(matching.unread);
        assert!(matching.local_unread);

        assert!(
            !ctx.prepare_notification_open(&read),
            "the already-read GitHub thread needs no request"
        );
        let opened = ctx.notifications.lock().unwrap()[0].clone();
        assert!(!opened.unread);
        assert!(!opened.local_unread);
        assert_eq!(opened.last_read_at, Some(last_read_at));
    }

    #[tokio::test]
    async fn new_activity_replaces_a_local_unread_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let mut read = thread("7", "seven");
        read.unread = false;
        ctx.replace_notifications(vec![read.clone()], Some("first"));
        ctx.mark_notification_unread("7");

        read.updated_at = "2026-09-18T18:00:00Z".parse().unwrap();
        ctx.replace_notifications(vec![read], Some("second"));

        let newer = ctx.notifications.lock().unwrap()[0].clone();
        assert!(!newer.unread);
        assert!(!newer.local_unread);
    }

    #[tokio::test]
    async fn github_unread_state_replaces_a_local_unread_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let mut read = thread("7", "seven");
        read.unread = false;
        ctx.replace_notifications(vec![read.clone()], Some("first"));
        ctx.mark_notification_unread("7");

        read.unread = true;
        ctx.replace_notifications(vec![read.clone()], Some("second"));
        ctx.mark_notification_unread("7");

        let github_unread = ctx.notifications.lock().unwrap()[0].clone();
        assert!(github_unread.unread);
        assert!(!github_unread.local_unread);
        assert!(ctx.prepare_notification_open(&read));
    }

    #[tokio::test]
    async fn done_stays_hidden_until_newer_activity_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let dismissed = thread("7", "seven");
        let other = thread("8", "eight");
        ctx.replace_notifications(
            vec![dismissed.clone(), other.clone()],
            Some("first"),
        );

        ctx.dismiss_notification(&dismissed.id);
        assert_eq!(*ctx.notifications.lock().unwrap(), vec![other.clone()]);
        assert_eq!(
            ctx.with_cache(|cache| cache.load_threads().unwrap())
                .unwrap(),
            vec![other.clone()]
        );

        let visible = ctx.replace_notifications(
            vec![dismissed.clone(), other.clone()],
            Some("same-revision"),
        );
        assert_eq!(visible, vec![other.clone()]);
        assert_eq!(*ctx.notifications.lock().unwrap(), vec![other.clone()]);

        let mut newer = dismissed;
        newer.updated_at = "2026-09-18T18:00:00Z".parse().unwrap();
        let visible = ctx.replace_notifications(
            vec![newer.clone(), other.clone()],
            Some("newer-revision"),
        );
        assert_eq!(visible, vec![newer.clone(), other.clone()]);
        assert_eq!(*ctx.notifications.lock().unwrap(), visible);
        assert_eq!(
            ctx.with_cache(|cache| cache.load_threads().unwrap())
                .unwrap(),
            visible
        );
    }

    fn pull_request(number: u64) -> PullRequest {
        PullRequest {
            number,
            title: format!("pull {number}"),
            author_login: "u".into(),
            author_avatar_url: String::new(),
            head_sha: format!("h{number}"),
            head_ref: format!("branch-{number}"),
            base_sha: "m".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "a/b".into(),
            html_url: format!("https://github.com/a/b/pull/{number}"),
            head_repo_full_name: Some("a/b".into()),
            updated_at: Some("2026-09-16T11:40:00Z".parse().unwrap()),
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

    fn merge_overview(merge_state: MergeState) -> PullRequestOverview {
        PullRequestOverview {
            head_sha: "h7".into(),
            mergeable: MergeableState::Mergeable,
            merge_state,
            review_decision: None,
            viewer_can_merge_as_admin: false,
            viewer_permission: Some(RepositoryPermission::Write),
            allowed_merge_methods: perch_github::MergeMethods {
                merge: true,
                squash: true,
                rebase: true,
            },
            checks: perch_github::Checks {
                state: perch_github::CheckState::Passing,
                url: None,
                runs: Vec::new(),
            },
        }
    }

    #[test]
    fn a_clean_current_head_can_merge_with_an_enabled_method() {
        assert_eq!(
            validate_merge_attempt(
                &pull_request(7),
                "h7",
                false,
                &merge_overview(MergeState::Clean),
                "h7",
                MergeMethod::Squash,
                false,
            ),
            Ok(())
        );
    }

    #[test]
    fn blocked_merge_requires_explicit_authorized_bypass() {
        let pull = pull_request(7);
        let mut overview = merge_overview(MergeState::Blocked);
        assert!(validate_merge_attempt(
            &pull,
            "h7",
            false,
            &overview,
            "h7",
            MergeMethod::Merge,
            false,
        )
        .unwrap_err()
        .contains("requirements"));

        overview.viewer_can_merge_as_admin = true;
        overview.viewer_permission = Some(RepositoryPermission::Admin);
        assert_eq!(
            validate_merge_attempt(
                &pull,
                "h7",
                false,
                &overview,
                "h7",
                MergeMethod::Merge,
                true,
            ),
            Ok(())
        );
    }

    #[test]
    fn conflicts_and_stale_heads_never_reach_the_merge_endpoint() {
        let pull = pull_request(7);
        let mut overview = merge_overview(MergeState::Dirty);
        overview.viewer_can_merge_as_admin = true;
        overview.viewer_permission = Some(RepositoryPermission::Admin);
        assert!(validate_merge_attempt(
            &pull,
            "h7",
            false,
            &overview,
            "h7",
            MergeMethod::Rebase,
            true,
        )
        .unwrap_err()
        .contains("conflicts"));

        let overview = merge_overview(MergeState::Clean);
        assert!(validate_merge_attempt(
            &pull,
            "old-head",
            false,
            &overview,
            "h7",
            MergeMethod::Rebase,
            false,
        )
        .unwrap_err()
        .contains("moved"));
    }

    #[test]
    fn disabled_methods_and_historical_reviews_cannot_merge() {
        let pull = pull_request(7);
        let mut overview = merge_overview(MergeState::Clean);
        overview.allowed_merge_methods.squash = false;
        assert!(validate_merge_attempt(
            &pull,
            "h7",
            false,
            &overview,
            "h7",
            MergeMethod::Squash,
            false,
        )
        .unwrap_err()
        .contains("disabled"));
        assert!(validate_merge_attempt(
            &pull,
            "h7",
            true,
            &merge_overview(MergeState::Clean),
            "h7",
            MergeMethod::Merge,
            false,
        )
        .unwrap_err()
        .contains("current review"));
    }

    #[test]
    fn pull_request_status_generations_are_monotonic_per_item() {
        let requests = PullRequestStatusRequests::default();

        let first = requests.begin("first");
        let second = requests.begin("first");
        let other = requests.begin("other");

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(other, 1);
        assert!(!requests.is_current("first", first));
        assert!(requests.is_current("first", second));
        assert!(requests.is_current("other", other));
        assert!(!requests.is_current("missing", 1));
    }

    #[test]
    fn merge_attempts_suppress_duplicates_and_only_matching_finishes() {
        let attempts = MergeAttempts::default();

        let first = attempts.begin("item").expect("first attempt starts");
        assert_eq!(attempts.begin("item"), None);

        attempts.finish("item", first.saturating_add(1));
        assert_eq!(attempts.begin("item"), None, "a stale finish is ignored");

        attempts.finish("item", first);
        let second = attempts.begin("item").expect("matching finish releases item");
        assert!(second > first);
    }

    /// A durable row for `a/b#<number>`, as an on-demand open makes it.
    fn store_pull(ctx: &Ctx, number: u64) -> Item {
        let mut item = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        item.subject = Some(ItemSubject::PullRequest(pull_request(number)));
        ctx.with_cache(|c| c.insert_item(&item)).unwrap().unwrap();
        item
    }

    fn with_reason(mut t: Thread, reason: Reason) -> Thread {
        t.reason = reason;
        t
    }

    fn items_sent(received: &Receiver<Event>) -> usize {
        received
            .try_iter()
            .filter(|e| matches!(e, Event::Items(_)))
            .count()
    }

    #[tokio::test]
    async fn notification_changes_leave_the_item_and_its_cached_conversation_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let item = store_pull(&ctx, 7);
        let discussion = perch_github::Discussion {
            opened_at: None,
            body: "kept locally".into(),
            comments: Vec::new(),
            reviews: Vec::new(),
            review_comments: Vec::new(),
            events: Vec::new(),
        };
        ctx.with_cache(|c| c.store_discussion(&item.id, &discussion))
            .unwrap()
            .unwrap();
        ctx.replace_notifications(vec![thread("7", "seven")], Some("last-modified"));

        ctx.edit_notifications(|threads| {
            threads[0].unread = false;
            threads[0].last_read_at = Some(Timestamp::now());
        });
        assert!(!ctx.with_cache(|c| c.load_threads().unwrap()).unwrap()[0].unread);

        ctx.edit_notifications(|threads| threads.clear());
        assert!(ctx
            .with_cache(|c| c.load_threads().unwrap())
            .unwrap()
            .is_empty());
        assert_eq!(ctx.item(&item.id), Some(item.clone()));
        assert_eq!(
            ctx.with_cache(|c| c.load_discussion(&item.id).unwrap().unwrap().1)
                .unwrap(),
            discussion
        );
    }

    #[tokio::test]
    async fn a_notification_does_not_mutate_a_durable_item() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        let item = store_pull(&ctx, 7);

        attend(
            &ctx,
            &[with_reason(thread("7", "seven"), Reason::Subscribed)],
        )
        .await;

        assert_eq!(ctx.item(&item.id), Some(item));
        assert_eq!(items_sent(&received), 0);
    }

    #[tokio::test]
    async fn an_inbox_refresh_only_lists_notifications() {
        let dir = tempfile::tempdir().unwrap();
        let (base, paths) = serve("200 OK", "[]");
        let (ctx, _received) = ctx(dir.path(), &base, &["a/b"]);

        refresh(&ctx, "test", NotificationPollMode::Reconcile).await;

        assert_eq!(
            paths.lock().unwrap().as_slice(),
            ["/notifications?all=true&participating=false&per_page=50&page=1"]
        );
    }

    #[tokio::test]
    async fn a_reconciliation_repairs_a_stale_conditional_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let old = thread("7", "stale");
        let incoming = thread("8", "current");
        let body = serde_json::to_string(&vec![incoming.clone()]).unwrap();
        let (base, _paths) = serve_by_request(move |_path, headers| {
            let conditional = headers.iter().any(|header| {
                header
                    .split_once(':')
                    .is_some_and(|(name, _)| name.eq_ignore_ascii_case("if-modified-since"))
            });
            if conditional {
                ("304 Not Modified", String::new())
            } else {
                ("200 OK", body.clone())
            }
        });
        let (ctx, _received) = ctx(dir.path(), &base, &[]);
        ctx.replace_notifications(
            vec![old.clone()],
            Some("Sat, 19 Sep 2026 12:00:00 GMT"),
        );

        refresh(&ctx, "test", NotificationPollMode::Conditional).await;
        assert_eq!(*ctx.notifications.lock().unwrap(), vec![old]);

        refresh(&ctx, "test", NotificationPollMode::Reconcile).await;
        assert_eq!(*ctx.notifications.lock().unwrap(), vec![incoming]);
    }

    #[tokio::test]
    async fn an_offline_refresh_reports_no_token_without_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let (base, paths) = serve("200 OK", "[]");
        let (mut ctx, received) = ctx(dir.path(), &base, &["a/b"]);
        ctx.github = None;

        refresh(&ctx, "test", NotificationPollMode::Reconcile).await;

        assert!(paths.lock().unwrap().is_empty());
        assert!(received
            .try_iter()
            .any(|event| matches!(event, Event::Status(Status::NoToken { .. }))));
    }

    #[tokio::test]
    async fn cached_agent_history_and_detail_do_not_need_github() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, received) = ctx(dir.path(), &nowhere(), &[]);
        ctx.github = None;
        let run = RunRecord {
            id: "offline-run".into(),
            backend: "codex".into(),
            original_request: "Explain the cache".into(),
            status: RunStatus::Completed,
            queued_at: Timestamp::now(),
            ended_at: Some(Timestamp::now()),
            final_result: Some("The cached answer".into()),
            ..RunRecord::default()
        };
        ctx.with_cache(|cache| cache.insert_run(&run))
            .unwrap()
            .unwrap();

        send_agent_runs(&ctx, 50);
        send_agent_run_detail(&ctx, &run.id);

        let events: Vec<_> = received.try_iter().collect();
        assert!(events.iter().any(|event| matches!(
            event,
            Event::AgentRuns { runs, .. } if runs.iter().any(|stored| stored.id == run.id)
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::AgentRunDetail { run: stored, .. } if stored.id == run.id
        )));
    }

    /// `a/b#7` as `GET /repos/a/b/pulls/7` sends it, so an `attend` that
    /// still fetched and made a row could, and would be seen to.
    const PULL_7: &str = r#"{"number":7,"title":"seven","state":"open","draft":false,
        "user":{"login":"u","avatar_url":""},
        "head":{"sha":"h7","ref":"branch-7","repo":{"full_name":"a/b"}},
        "base":{"sha":"m","ref":"main","repo":{"full_name":"a/b"}},
        "updated_at":"2026-09-16T11:40:00Z"}"#;

    #[tokio::test]
    async fn a_review_request_with_no_item_makes_none() {
        let dir = tempfile::tempdir().unwrap();
        // A GitHub that would answer for the pull request, proving that a
        // background notification poll still does not fetch it or make an
        // item. Opening the notification is the on-demand path that does.
        let (base, paths) = serve("200 OK", PULL_7);
        let (ctx, received) = ctx(dir.path(), &base, &[]);

        attend(
            &ctx,
            &[with_reason(thread("7", "seven"), Reason::ReviewRequested)],
        )
        .await;

        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert!(
            items.is_empty(),
            "a notification does not decide what exists"
        );
        assert_eq!(items_sent(&received), 0);
        assert!(
            paths.lock().unwrap().is_empty(),
            "and nothing was asked to find out"
        );
    }

    const FAILING_RUN: &str = r#"{
        "data": { "repository": {
            "merge_commit_allowed": true,
            "squash_merge_allowed": true,
            "rebase_merge_allowed": true,
            "viewer_permission": "WRITE",
            "pull_request": {
                "head_sha": "h7",
                "mergeable": "MERGEABLE",
                "merge_state": "BLOCKED",
                "review_decision": "REVIEW_REQUIRED",
                "viewer_can_merge_as_admin": false,
                "status_check_rollup": {
                    "state": "FAILURE",
                    "contexts": { "nodes": [{
                        "__typename": "CheckRun",
                        "name": "ci",
                        "status": "COMPLETED",
                        "conclusion": "FAILURE",
                        "details_url": "https://github.com/a/b/actions/runs/1",
                        "started_at": "2026-09-18T11:00:00Z",
                        "completed_at": "2026-09-18T11:05:00Z",
                        "check_suite": { "workflow_run": {
                            "event": "pull_request",
                            "workflow": { "name": "CI" }
                        }}
                    }]}
                }
            }
        }}
    }"#;

    #[tokio::test]
    async fn red_checks_are_cached_without_mutating_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let (base, paths) = serve("200 OK", FAILING_RUN);
        let (ctx, received) = ctx(dir.path(), &base, &[]);
        let item = store_pull(&ctx, 7);

        let pull_request = pull_request(7);
        let status_generation = begin_pull_request_status(&ctx, &item.id, &pull_request);
        fetch_checks(&ctx, &item.id, &pull_request, status_generation).await;

        let checks = ctx
            .with_cache(|c| c.load_checks(&item.id, "h7").unwrap())
            .unwrap();
        assert!(matches!(
            checks,
            Some(perch_github::Checks {
                state: perch_github::CheckState::Failing { .. },
                ..
            })
        ));
        assert_eq!(items_sent(&received), 0);
        assert_eq!(
            paths.lock().unwrap().as_slice(),
            ["/graphql"]
        );
    }

    #[tokio::test]
    async fn a_branch_of_a_local_repository_is_not_looked_up_on_github() {
        let dir = tempfile::tempdir().unwrap();
        let (base, paths) = serve("200 OK", "[]");
        // A GitHub repository is declared too, so the candidates a branch
        // is looked up under would have somewhere to go.
        let (ctx, _received) = ctx_declaring(
            dir.path(),
            &base,
            vec![local_repo("notes"), github_repo("a/b")],
        );
        let mut branch = Item::new("local/notes", "2026-09-10T00:00:00Z".parse().unwrap());
        branch.checkout = Some(root_checkout("feature"));
        ctx.with_cache(|c| c.insert_item(&branch)).unwrap().unwrap();

        let changed = attach_pull_requests(&ctx).await;

        assert!(!changed);
        assert!(
            paths.lock().unwrap().is_empty(),
            "no request for a local branch"
        );
        assert!(ctx.item(&branch.id).unwrap().subject.is_none());

        // A branch of the GitHub repository is looked up under it alone:
        // the local repository is not a candidate.
        let mut hub = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        hub.checkout = Some(ItemCheckout {
            path: "/x/b2".into(),
            ..root_checkout("feature-2")
        });
        ctx.with_cache(|c| c.insert_item(&hub)).unwrap().unwrap();

        attach_pull_requests(&ctx).await;

        let paths = paths.lock().unwrap();
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(
            paths[0].starts_with("/repos/a/b/pulls?head="),
            "{}",
            paths[0]
        );
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository on `main` with one commit at `root`, and a linked
    /// worktree beside it on `branch` with one more commit, one line
    /// added. No remote unless the test adds one, and nothing fetches: a
    /// remote is config the scan reads back as text.
    fn root_with_branch(root: &Path, branch: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(root).unwrap();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("one.txt"), "one\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "one"]);
        let worktree = root.with_file_name(format!(
            "{}-{}",
            root.file_name().unwrap().to_string_lossy(),
            branch
        ));
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                worktree.to_str().unwrap(),
            ],
        );
        std::fs::write(worktree.join("two.txt"), "two\n").unwrap();
        git(&worktree, &["add", "."]);
        git(&worktree, &["commit", "-q", "-m", "two"]);
        worktree
    }

    #[tokio::test]
    async fn archiving_waits_for_active_work_before_retiring_its_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let repo = RepoId::parse("a/b").unwrap();

        // Put a real linked worktree where Perch's store expects it so the
        // assertion covers both the database fence and filesystem cleanup.
        let source = dir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q", "-b", "main"]);
        std::fs::write(source.join("one.txt"), "one\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-q", "-m", "one"]);
        let bare = ctx.preparer.store().bare_path(&repo);
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        git(
            &source,
            &["clone", "-q", "--bare", ".", bare.to_str().unwrap()],
        );
        let branch = "perch/pr-7";
        let worktree = ctx.preparer.store().worktree_path(&repo, branch);
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        let head = git(&worktree, &["rev-parse", "HEAD"]);

        let mut item = store_pull(&ctx, 7);
        item.checkout = Some(ItemCheckout {
            branch: branch.into(),
            base_ref: "main".into(),
            head_sha: head,
            path: worktree.clone(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        ctx.with_cache(|cache| cache.update_item(&item))
            .unwrap()
            .unwrap();

        let run = RunRecord {
            id: "archive-run".into(),
            owner: RunOwner::Review {
                item_id: item.id.clone(),
                reviewer_run_id: "reviewer-run".into(),
            },
            purpose: RunPurpose::WorktreeReview,
            backend: "codex".into(),
            requested_model: "gpt".into(),
            model: "gpt".into(),
            requested_effort: "high".into(),
            effort: "high".into(),
            original_request: "Review this worktree".into(),
            working_directory: Some(worktree.to_string_lossy().into_owned()),
            queued_at: Timestamp::now(),
            ..RunRecord::default()
        };
        let admission = ctx.runs.admit(run).unwrap();
        let lease = ctx.runs.start(admission).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), archive_item(&ctx, &item.id, None))
            .await
            .expect("archive did not wait on the active run")
            .unwrap();
        let archived = ctx.item(&item.id).unwrap();
        assert!(archived.is_archived());
        assert!(archived.checkout.as_ref().unwrap().on_disk);
        assert!(worktree.join(".git").exists());
        assert_eq!(
            ctx.runs.load("archive-run").unwrap().status,
            RunStatus::Starting,
            "archive must not cancel the work it is retaining"
        );

        ctx.runs.transition(
            "archive-run",
            RunStatus::Failed,
            Some(TerminalReason::Other),
            Some("fixture complete".into()),
        );
        ctx.runs.finish(lease);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !worktree.exists()
                    && ctx
                        .item(&item.id)
                        .and_then(|item| item.checkout)
                        .is_some_and(|checkout| !checkout.on_disk)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("deferred archive cleanup did not finish");
        assert_eq!(
            git(
                &bare,
                &["branch", "--format=%(refname:short)", "--list", branch],
            ),
            branch,
            "archiving removes the checkout but retains its branch"
        );
    }

    /// A `[[repo]]` with one root and, when given, a `github`.
    fn rooted(github: Option<&str>, root: &Path) -> perch_github::RepoConfig {
        let mut repo = perch_github::RepoConfig::default();
        repo.github = github.map(str::to_string);
        repo.roots = vec![root.to_string_lossy().into_owned()];
        repo
    }

    fn notices(received: &Receiver<Event>) -> Vec<String> {
        received
            .try_iter()
            .filter_map(|e| match e {
                Event::Notice(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_local_root_s_branches_are_rows_under_local_and_nothing_is_asked_of_github() {
        // The design's scan case: a `git init` repository with no remote,
        // declared by its root alone. Its branch is a row under
        // `local/<name>`, diffed against `main` since there is no remote to
        // have a copy of it, and neither the scan nor the pull request
        // lookup that follows it sends a request.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        root_with_branch(&root, "feature");
        let (base, paths) = serve("200 OK", "[]");
        let mut entry = rooted(None, &root);
        entry.resolve(None);
        let (ctx, received) = ctx_declaring(dir.path(), &base, vec![entry]);
        assert_eq!(ctx.config.declared(), ["local/notes"]);

        assert!(scan_roots(&ctx).await, "a row was made");

        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        let item = &items[0];
        assert_eq!(
            item.repo_full_name, "local/notes",
            "the entry's identity, not the scan's"
        );
        let checkout = item.checkout.as_ref().expect("a checkout");
        assert_eq!(checkout.branch, "feature");
        assert_eq!(
            checkout.base_ref, "main",
            "the base branch, with no remote to prefix it"
        );
        assert_eq!(checkout.churn, Some((1, 0)), "counted against main");
        assert!(!checkout.owned);
        assert!(notices(&received).is_empty(), "nothing to complain about");

        assert!(!attach_pull_requests(&ctx).await);

        assert!(
            paths.lock().unwrap().is_empty(),
            "no GitHub call for a local repository"
        );
        assert!(
            !scan_roots(&ctx).await,
            "a second scan over the same tree changes nothing"
        );
    }

    #[tokio::test]
    async fn a_github_entry_whose_root_has_no_remote_is_told_so_and_gets_no_row() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("b");
        root_with_branch(&root, "feature");
        let (ctx, received) =
            ctx_declaring(dir.path(), &nowhere(), vec![rooted(Some("a/b"), &root)]);

        assert!(!scan_roots(&ctx).await);

        assert!(ctx
            .with_cache(|c| c.load_items().unwrap())
            .unwrap()
            .is_empty());
        let notices = notices(&received);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("no GitHub remote for b"),
            "{}",
            notices[0]
        );
        assert!(
            notices[0].starts_with(&root.display().to_string()),
            "names the root: {}",
            notices[0]
        );
    }

    #[tokio::test]
    async fn a_fork_root_s_branch_is_filed_under_the_upstream_and_takes_its_pull_request_there() {
        // The reader's copy is `origin`, the repository it came from is
        // `upstream`, and the entry declares the upstream. The branch is a
        // row of the upstream's, and its pull request, opened on the
        // upstream from the fork, is looked up there with the fork as its
        // head.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("widgets");
        root_with_branch(&root, "fence");
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/me/widgets.git",
            ],
        );
        git(
            &root,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/acme/widgets.git",
            ],
        );
        let (base, paths) = serve_by_path(|path| {
            if path.starts_with("/repos/acme/widgets/pulls?head=me%3Afence") {
                ("200 OK", format!("[{PULL_FROM_FORK}]"))
            } else {
                ("200 OK", "[]".to_string())
            }
        });
        let (ctx, _received) =
            ctx_declaring(dir.path(), &base, vec![rooted(Some("acme/widgets"), &root)]);

        assert!(scan_roots(&ctx).await);

        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(
            items[0].repo_full_name, "acme/widgets",
            "the entry's identity, which upstream names"
        );
        let checkout = items[0].checkout.as_ref().unwrap();
        assert_eq!(
            checkout.base_ref, "main",
            "the base's name, without the remote"
        );
        assert_eq!(
            checkout.churn,
            Some((1, 0)),
            "upstream was never fetched, so against main itself"
        );

        assert!(attach_pull_requests(&ctx).await);

        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 1, "the same row, not a second: {items:?}");
        assert_eq!(items[0].pull_request().map(|p| p.number), Some(9));
        assert_eq!(
            items[0].checkout.as_ref().map(|c| c.branch.as_str()),
            Some("fence")
        );
        let paths = paths.lock().unwrap();
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(
            paths[0].starts_with("/repos/acme/widgets/pulls?head=me%3Afence"),
            "asked of the upstream, with the fork as the head: {}",
            paths[0]
        );
    }

    /// `acme/widgets#9`, opened from the fork `me/widgets`, as the pulls
    /// list sends it.
    const PULL_FROM_FORK: &str = r#"{"number":9,"title":"fence","state":"open","draft":false,
        "user":{"login":"me","avatar_url":""},
        "head":{"sha":"h9","ref":"fence","repo":{"full_name":"me/widgets"}},
        "base":{"sha":"m","ref":"main","repo":{"full_name":"acme/widgets"}},
        "updated_at":"2026-09-16T11:40:00Z"}"#;

    /// A checkout under a root on `branch`, as the scan records it.
    fn root_checkout(branch: &str) -> ItemCheckout {
        ItemCheckout {
            branch: branch.into(),
            base_ref: "develop".into(),
            head_sha: "h7".into(),
            path: "/x/b".into(),
            owned: false,
            on_disk: true,
            churn: Some((3, 1)),
            last_seen_head: None,
        }
    }

    #[tokio::test]
    async fn a_branch_takes_its_pull_request_from_a_subject_only_row_and_carries_the_marks() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        // An on-demand open made #7 before the scan saw the local branch;
        // the reader then pinned that subject-only row.
        let mut subject_only = store_pull(&ctx, 7);
        subject_only.pinned_at = Some("2026-09-17T09:00:00Z".parse().unwrap());
        ctx.with_cache(|c| c.update_item(&subject_only))
            .unwrap()
            .unwrap();
        let mut branch = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        branch.checkout = Some(root_checkout("branch-7"));
        ctx.with_cache(|c| c.insert_item(&branch)).unwrap().unwrap();

        let taken = ctx
            .with_cache(|c| take_pull_request(c, &mut branch, pull_request(7)))
            .unwrap()
            .unwrap();

        assert!(taken);
        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(
            items.len(),
            1,
            "one row for the branch and its pull request"
        );
        assert_eq!(
            items[0].id, branch.id,
            "the reader's row, with its threads, is the one kept"
        );
        assert_eq!(items[0].pull_request().map(|p| p.number), Some(7));
        assert_eq!(
            items[0].checkout.as_ref().map(|c| c.base_ref.as_str()),
            Some("main"),
            "the pull request's base"
        );
        assert_eq!(
            items[0].pinned_at, subject_only.pinned_at,
            "the pin on the subject-only row came across"
        );
    }

    #[tokio::test]
    async fn a_pull_request_held_by_a_row_with_its_own_checkout_stays_there() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        // Opened in Perch first, so the pull request's row has a checkout
        // Perch cut for it.
        let mut opened = store_pull(&ctx, 7);
        opened.checkout = Some(ItemCheckout {
            branch: "perch/pr-7".into(),
            owned: true,
            ..root_checkout("perch/pr-7")
        });
        ctx.with_cache(|c| c.update_item(&opened)).unwrap().unwrap();
        let mut branch = Item::new("a/b", "2026-09-10T00:00:00Z".parse().unwrap());
        branch.checkout = Some(root_checkout("branch-7"));
        ctx.with_cache(|c| c.insert_item(&branch)).unwrap().unwrap();

        let taken = ctx
            .with_cache(|c| take_pull_request(c, &mut branch, pull_request(7)))
            .unwrap()
            .unwrap();

        assert!(!taken, "the row that has it keeps it");
        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 2, "and the branch stays a branch");
        assert!(ctx.item(&branch.id).unwrap().subject.is_none());
    }

    #[tokio::test]
    async fn an_open_that_found_no_row_writes_onto_one_another_open_made_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        // Nothing when the open looked; then, while it cloned, another open
        // stored #7 with a later snapshot.
        let mut listed = store_pull(&ctx, 7);
        let mut later = pull_request(7);
        later.title = "seven, renamed".into();
        later.updated_at = Some("2026-09-17T09:00:00Z".parse().unwrap());
        listed.subject = Some(ItemSubject::PullRequest(later));
        ctx.with_cache(|c| c.update_item(&listed)).unwrap().unwrap();
        let now: Timestamp = "2026-09-18T12:00:00Z".parse().unwrap();
        let mut opened = Item::new("a/b", now);
        opened.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        opened.checkout = Some(ItemCheckout {
            branch: "perch/pr-7".into(),
            owned: true,
            ..root_checkout("perch/pr-7")
        });

        ctx.with_cache(|c| record_open(c, &mut opened, true))
            .unwrap()
            .expect("no collision on the subject index");

        assert_eq!(
            opened.id, listed.id,
            "the review runs as the row that exists"
        );
        let items = ctx.with_cache(|c| c.load_items().unwrap()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].checkout.as_ref().map(|c| c.branch.as_str()),
            Some("perch/pr-7"),
            "with the open's checkout"
        );
        assert_eq!(items[0].last_opened_at, now);
        assert_eq!(
            items[0].title(),
            "seven, renamed",
            "the later snapshot stands over the one fetched before the clone"
        );
    }

    #[tokio::test]
    async fn an_open_that_found_no_row_and_still_finds_none_inserts_one() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _received) = ctx(dir.path(), &nowhere(), &[]);
        let mut opened = Item::new("a/b", "2026-09-18T12:00:00Z".parse().unwrap());
        opened.subject = Some(ItemSubject::PullRequest(pull_request(7)));
        opened.checkout = Some(ItemCheckout {
            branch: "perch/pr-7".into(),
            owned: true,
            ..root_checkout("perch/pr-7")
        });

        ctx.with_cache(|c| record_open(c, &mut opened, true))
            .unwrap()
            .unwrap();

        assert_eq!(ctx.item(&opened.id).unwrap(), opened);
    }
}
