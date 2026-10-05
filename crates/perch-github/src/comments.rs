//! Comment threads anchored to lines of a worktree, and the agent runs
//! that answer in them.
//!
//! Shaped like GitHub's review threads on purpose: `side` and `start_line`
//! are its names, and a review thread fetched from GitHub sits in the same
//! list with the same anchor type, told apart by `origin`. Nothing here
//! fetches or posts one; `review_threads_of` turns a fetched discussion
//! into threads, and the cache keeps them.
//!
//! `CommentThread` rather than `Thread`: that word is the notification.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::discussion::{Discussion, ReviewComment};
use crate::reactions::Reactions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadOrigin {
    /// Perch's own. The truth; nothing replaces it.
    Local,
    /// A cache of GitHub's, replaceable wholesale by `github_id`.
    GitHub,
}

impl ThreadOrigin {
    pub fn name(self) -> &'static str {
        match self {
            ThreadOrigin::Local => "local",
            ThreadOrigin::GitHub => "github",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "github" {
            ThreadOrigin::GitHub
        } else {
            ThreadOrigin::Local
        }
    }
}

/// Which file a line number counts in: the old one or the new one.
///
/// Shared with GitHub's review comments, whose `side` is the same two
/// words. Serialised as `left` and `right`, the column values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "left" {
            Side::Left
        } else {
            Side::Right
        }
    }
}

/// Lines a thread is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    /// Inclusive, one-based, in the file on `side`.
    pub start_line: u32,
    /// Inclusive; equal to `start_line` for one line.
    pub line: u32,
    pub side: Side,
    /// The head the lines were read at.
    pub commit_sha: String,
}

impl Anchor {
    /// `src/x.rs:41–58`, or `src/x.rs:41`.
    pub fn label(&self) -> String {
        if self.start_line == self.line {
            format!("{}:{}", self.path, self.line)
        } else {
            format!("{}:{}\u{2013}{}", self.path, self.start_line, self.line)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommentThread {
    /// Perch's own id for a local thread; `gh-{root comment id}` for one
    /// from GitHub, so ingesting the same thread twice finds the same row.
    pub id: String,
    pub worktree_id: String,
    pub origin: ThreadOrigin,
    /// The id of the root review comment on GitHub. `None` for Perch's own
    /// and for a thread on its way to GitHub.
    pub github_id: Option<u64>,
    /// `None` for a thread about the worktree as a whole.
    pub anchor: Option<Anchor>,
    /// The file changed after the anchor was made; the lines may have moved.
    pub outdated: bool,
    pub resolved: bool,
    /// The thread's GraphQL id on GitHub, which resolving it goes by.
    /// `None` for Perch's own, for one on its way, and for one whose
    /// state GitHub did not say.
    pub node_id: Option<String>,
    /// Whether GitHub lets the reader resolve the thread and unresolve it.
    pub can_resolve: bool,
    pub created_at: Timestamp,
    pub messages: Vec<CommentMessage>,
}

impl CommentThread {
    /// The session id of the last run that finished, which a follow-up
    /// resumes. `None` means start over with the history folded in.
    ///
    /// A session belongs to the backend that made it, so the last finished
    /// run only counts when it ran on `backend`. Switching backends
    /// mid-thread starts over rather than looking for an older run of the
    /// right kind, because the history since that run would be lost.
    pub fn resumable_session(&self, backend: &str) -> Option<String> {
        self.resumable_run(backend)
            .and_then(|r| r.session_id.clone())
    }

    /// The run [`resumable_session`](Self::resumable_session) continues,
    /// by the same rule, so what it recorded comes with its session.
    pub fn resumable_run(&self, backend: &str) -> Option<&RunRecord> {
        self.messages
            .iter()
            .rev()
            .filter_map(|m| m.run.as_ref())
            .find(|r| r.status == RunStatus::Completed)
            .filter(|r| r.backend == backend && r.session_id.is_some())
    }

    /// [`resumable_run`](Self::resumable_run), when it was of `mode`: a
    /// session begun as a question was told not to change a file, and
    /// one begun as work was told it may, so neither is continued as the
    /// other.
    pub fn resumable_run_in(&self, backend: &str, mode: RunMode) -> Option<&RunRecord> {
        self.resumable_run(backend).filter(|r| r.mode == mode)
    }

    /// The mode of the thread's latest run, which a follow-up starts on.
    pub fn last_mode(&self) -> Option<RunMode> {
        self.messages
            .iter()
            .rev()
            .find_map(|m| m.run.as_ref())
            .map(|r| r.mode)
    }

    pub fn running_run(&self) -> Option<&RunRecord> {
        self.messages
            .iter()
            .filter_map(|m| m.run.as_ref())
            .find(|r| r.status.is_active())
    }

    /// Whether the thread is GitHub's, posted or not yet.
    pub fn is_github_bound(&self) -> bool {
        self.origin == ThreadOrigin::GitHub
    }

    /// Whether anything in the thread went to GitHub, or is going.
    pub fn has_github_message(&self) -> bool {
        self.messages
            .iter()
            .any(|m| m.origin == ThreadOrigin::GitHub)
    }

    /// Whether the thread is on GitHub, so that resolving it is GitHub's.
    /// One on its way there with a draft is not yet.
    pub fn on_github(&self) -> bool {
        self.github_id.is_some()
    }

    /// Whether the reader can resolve the thread, or unresolve it. One on
    /// GitHub resolves there, where GitHub said they may and named the
    /// thread to ask it by; any other resolves in Perch.
    pub fn resolvable(&self) -> bool {
        !self.on_github() || (self.can_resolve && self.node_id.is_some())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Author {
    User,
    Agent { backend: String, model: String },
    GitHubUser(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommentMessage {
    pub id: String,
    pub thread_id: String,
    /// Where the message went: to GitHub, or to an agent and so to no one
    /// else.
    pub origin: ThreadOrigin,
    pub author: Author,
    pub body: String,
    pub created_at: Timestamp,
    /// An agent's answer carries the run that produced it. A user's
    /// question carries the run that is answering it while it runs, so
    /// the thread has somewhere to show the steps before there is an
    /// answer.
    pub run: Option<RunRecord>,
    /// The comment's id on GitHub; `None` until it is posted.
    pub github_id: Option<u64>,
    /// The comment's GraphQL node id, which reactions are left by.
    pub node_id: Option<String>,
    /// Where the comment is on github.com.
    pub html_url: Option<String>,
    /// The hunk a review comment was left on, as GitHub shows it, for
    /// when the lines are not in the checkout.
    pub diff_hunk: Option<String>,
    /// The reactions on a comment from GitHub. Empty on Perch's own.
    pub reactions: Reactions,
}

impl CommentMessage {
    /// The reader's message to GitHub that has not been posted yet.
    pub fn is_draft(&self) -> bool {
        self.origin == ThreadOrigin::GitHub
            && self.author == Author::User
            && self.github_id.is_none()
    }

    /// A message no one on GitHub can see.
    pub fn is_private(&self) -> bool {
        self.origin == ThreadOrigin::Local
    }

    /// A comment that is on GitHub.
    pub fn from_github(&self) -> bool {
        self.origin == ThreadOrigin::GitHub && self.github_id.is_some()
    }
}

/// The review threads of a discussion as `CommentThread`s: each root
/// comment with its replies under it, in time order, threads in the order
/// their roots were left. A reply names the first comment of its thread,
/// but a chain is followed all the same, and a reply whose parent is
/// missing starts a thread of its own. Comments of a review still pending
/// on github.com are the viewer's own unfinished review and are left out.
///
/// The thread's anchor is the root's line, or the line it was left on when
/// the head no longer has it, and the commit likewise; a root that names
/// no commit is anchored at `fallback_head_sha`. What GitHub says of the
/// thread's state wins; without it, a root with no head line is outdated
/// and nothing is resolved.
pub fn review_threads_of(
    discussion: &Discussion,
    item_id: &str,
    fallback_head_sha: &str,
) -> Vec<CommentThread> {
    let pending: Vec<u64> = discussion
        .reviews
        .iter()
        .filter(|r| r.state == "PENDING")
        .map(|r| r.id)
        .collect();
    let comments: Vec<&ReviewComment> = discussion
        .review_comments
        .iter()
        .filter(|c| {
            !c.pull_request_review_id
                .is_some_and(|id| pending.contains(&id))
        })
        .collect();

    let root_of = |c: &ReviewComment| -> u64 {
        let mut at = c;
        // Bounded, in case the ids loop.
        for _ in 0..comments.len() {
            let Some(parent) = at
                .in_reply_to_id
                .and_then(|id| comments.iter().find(|p| p.id == id))
            else {
                break;
            };
            at = parent;
        }
        at.id
    };

    let mut threads: Vec<CommentThread> = comments
        .iter()
        .filter(|c| root_of(c) == c.id)
        .map(|root| {
            let state = discussion
                .review_threads
                .iter()
                .find(|s| s.root_comment_id == root.id);
            let at_head = root.line.is_some();
            let line = root.line.or(root.original_line);
            let on_file = match line {
                None => file_comment_anchor(root, fallback_head_sha),
                Some(_) => None,
            };
            let anchor = line.map(|line| {
                let start_line = if at_head {
                    root.start_line
                } else {
                    root.original_start_line
                };
                let commit = if at_head {
                    &root.commit_id
                } else {
                    &root.original_commit_id
                };
                Anchor {
                    path: root.path.clone(),
                    start_line: start_line.unwrap_or(line) as u32,
                    line: line as u32,
                    side: root.side,
                    commit_sha: if commit.is_empty() {
                        fallback_head_sha.to_string()
                    } else {
                        commit.clone()
                    },
                }
            });
            let mut message = github_message(root, root.id);
            if let Some((_, body)) = &on_file {
                message.body = body.to_string();
            }
            CommentThread {
                id: github_thread_id(root.id),
                worktree_id: item_id.to_string(),
                origin: ThreadOrigin::GitHub,
                github_id: Some(root.id),
                outdated: state.map_or(!at_head && on_file.is_none(), |s| s.outdated),
                anchor: anchor.or(on_file.map(|(anchor, _)| anchor)),
                resolved: state.is_some_and(|s| s.resolved),
                node_id: state.map(|s| s.node_id.clone()).filter(|id| !id.is_empty()),
                can_resolve: state.is_some_and(|s| s.can_resolve),
                created_at: root.created_at,
                messages: vec![message],
            }
        })
        .collect();
    for c in comments.iter().filter(|c| root_of(c) != c.id) {
        let root = root_of(c);
        if let Some(t) = threads.iter_mut().find(|t| t.github_id == Some(root)) {
            t.messages.push(github_message(c, root));
        }
    }
    for t in &mut threads {
        t.messages[1..].sort_by_key(|m| m.created_at);
    }
    threads.sort_by_key(|t| t.created_at);
    threads
}

/// What a comment on lines outside a pull request's diff says when it
/// goes on the lines' file, which GitHub takes where it refuses the
/// lines: a permalink to the lines on a line of its own, which GitHub
/// draws as the code, then `body`. `repo_url` is the repository's page;
/// `commit` and `path` name the blob the lines count in, the head's for
/// the head side and the merge base's for the base side. Perch reads the
/// lines back from the link, `file_comment_anchor`.
pub fn file_comment_body(
    repo_url: &str,
    commit: &str,
    path: &str,
    start_line: u32,
    line: u32,
    body: &str,
) -> String {
    let lines = if start_line == line {
        format!("L{line}")
    } else {
        format!("L{start_line}-L{line}")
    };
    format!(
        "{}/blob/{commit}/{}#{lines}\n\n{body}",
        repo_url.trim_end_matches('/'),
        url_path(path)
    )
}

/// The lines a comment on a file is about and its text without them,
/// when it opens with a permalink to lines of its own file, as
/// `file_comment_body` writes one. A link at the commit the comment was
/// left on is on the head side; at any other, the base side. `None` for a
/// comment on lines, or one that opens with anything else.
pub fn file_comment_anchor<'a>(
    c: &'a ReviewComment,
    fallback_head_sha: &str,
) -> Option<(Anchor, &'a str)> {
    if c.line.is_some() || c.original_line.is_some() {
        return None;
    }
    let repo_url = &c.html_url[..c.html_url.find("/pull/")?];
    let body = c.body.trim_start();
    let (link, rest) = body.split_once('\n').unwrap_or((body, ""));
    let link = link
        .trim_end()
        .strip_prefix(repo_url)?
        .strip_prefix("/blob/")?;
    let (commit, link) = link.split_once('/')?;
    let (path, lines) = link.split_once("#L")?;
    if path != url_path(&c.path) || commit.is_empty() {
        return None;
    }
    let (start_line, line) = match lines.split_once("-L") {
        Some((start, end)) => (start.parse().ok()?, end.parse().ok()?),
        None => {
            let line = lines.parse().ok()?;
            (line, line)
        }
    };
    if start_line == 0 || start_line > line {
        return None;
    }
    let head = [c.original_commit_id.as_str(), c.commit_id.as_str()]
        .into_iter()
        .find(|sha| !sha.is_empty());
    let side = match head {
        Some(head) if head != commit => Side::Left,
        _ => Side::Right,
    };
    let anchor = Anchor {
        path: c.path.clone(),
        start_line,
        line,
        side,
        commit_sha: head.unwrap_or(fallback_head_sha).to_string(),
    };
    Some((anchor, rest.trim_start_matches(['\r', '\n'])))
}

/// `path` as a URL's path: each part percent-encoded, the slashes kept.
fn url_path(path: &str) -> String {
    const PART: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    path.split('/')
        .map(|part| percent_encoding::utf8_percent_encode(part, PART).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// The thread id a GitHub review thread has in Perch, from its root
/// comment's id.
pub fn github_thread_id(root_comment_id: u64) -> String {
    format!("gh-{root_comment_id}")
}

/// The message id a GitHub comment has in Perch.
pub fn github_message_id(comment_id: u64) -> String {
    format!("gh-{comment_id}")
}

fn github_message(c: &ReviewComment, root_id: u64) -> CommentMessage {
    CommentMessage {
        id: github_message_id(c.id),
        thread_id: github_thread_id(root_id),
        origin: ThreadOrigin::GitHub,
        author: Author::GitHubUser(c.user.clone()),
        body: c.body.clone(),
        created_at: c.created_at,
        run: None,
        github_id: Some(c.id),
        node_id: Some(c.node_id.clone()).filter(|id| !id.is_empty()),
        html_url: Some(c.html_url.clone()).filter(|url| !url.is_empty()),
        diff_hunk: Some(c.diff_hunk.clone()).filter(|hunk| !hunk.is_empty()),
        reactions: c.reactions.clone(),
    }
}

/// The durable thing an agent run belongs to. Labels and paths are display
/// snapshots; these ids are what an action validates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunOwner {
    Discussion {
        item_id: String,
        thread_id: String,
        request_id: String,
    },
    Review {
        item_id: String,
        reviewer_run_id: String,
    },
    /// Retained schema-13 history whose discussion owner could not be
    /// recovered. It remains readable, but cannot be acted on.
    Unavailable { legacy_thread_id: Option<String> },
}

impl RunOwner {
    pub fn item_id(&self) -> Option<&str> {
        match self {
            Self::Discussion { item_id, .. } | Self::Review { item_id, .. } => Some(item_id),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn thread_id(&self) -> Option<&str> {
        match self {
            Self::Discussion { thread_id, .. } => Some(thread_id),
            Self::Unavailable { legacy_thread_id } => legacy_thread_id.as_deref(),
            Self::Review { .. } => None,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Discussion { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPurpose {
    Question,
    FollowUp,
    WorktreeReview,
}

impl RunPurpose {
    pub fn name(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::FollowUp => "follow_up",
            Self::WorktreeReview => "worktree_review",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "question" => Some(Self::Question),
            "follow_up" => Some(Self::FollowUp),
            "worktree_review" => Some(Self::WorktreeReview),
            _ => None,
        }
    }
}

/// What a run was allowed to do: answer a question, or change the
/// worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Ask,
    Work,
}

impl RunMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Work => "work",
        }
    }

    /// What the card and the inspector call it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Work => "Work",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "ask" => Some(Self::Ask),
            "work" => Some(Self::Work),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Starting,
    Running,
    Stopping,
    Completed,
    Failed,
    Stopped,
    Interrupted,
}

impl RunStatus {
    pub const ACTIVE: [Self; 4] = [Self::Queued, Self::Starting, Self::Running, Self::Stopping];

    pub fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Starting | Self::Running | Self::Stopping
        )
    }

    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Queued => matches!(
                next,
                Self::Starting | Self::Stopped | Self::Interrupted | Self::Failed
            ),
            Self::Starting => matches!(
                next,
                Self::Running | Self::Stopping | Self::Failed | Self::Stopped | Self::Interrupted
            ),
            Self::Running => matches!(
                next,
                Self::Stopping | Self::Completed | Self::Failed | Self::Stopped | Self::Interrupted
            ),
            Self::Stopping => matches!(
                next,
                Self::Completed | Self::Failed | Self::Stopped | Self::Interrupted
            ),
            Self::Completed | Self::Failed | Self::Stopped | Self::Interrupted => false,
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "stopping" => Some(Self::Stopping),
            "completed" | "finished" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "stopped" => Some(Self::Stopped),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReason {
    Completed,
    QueueCancelled,
    UserStopped,
    SpawnFailed,
    PermissionDenied,
    MalformedStream,
    UnexpectedExit,
    AppShutdown,
    AppRestart,
    UnavailableConfiguration,
    Other,
}

impl TerminalReason {
    pub fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::QueueCancelled => "queue_cancelled",
            Self::UserStopped => "user_stopped",
            Self::SpawnFailed => "spawn_failed",
            Self::PermissionDenied => "permission_denied",
            Self::MalformedStream => "malformed_stream",
            Self::UnexpectedExit => "unexpected_exit",
            Self::AppShutdown => "app_shutdown",
            Self::AppRestart => "app_restart",
            Self::UnavailableConfiguration => "unavailable_configuration",
            Self::Other => "other",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "queue_cancelled" => Some(Self::QueueCancelled),
            "user_stopped" => Some(Self::UserStopped),
            "spawn_failed" => Some(Self::SpawnFailed),
            "permission_denied" => Some(Self::PermissionDenied),
            "malformed_stream" => Some(Self::MalformedStream),
            "unexpected_exit" => Some(Self::UnexpectedExit),
            "app_shutdown" => Some(Self::AppShutdown),
            "app_restart" => Some(Self::AppRestart),
            "unavailable_configuration" => Some(Self::UnavailableConfiguration),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevisionContext {
    #[default]
    Unknown,
    LiveCheckout {
        head_sha: Option<String>,
        base_ref: Option<String>,
        dirty: Option<bool>,
    },
    Snapshot {
        base_sha: String,
        head_sha: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunLabels {
    pub repository: String,
    pub item: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub owner: RunOwner,
    pub purpose: RunPurpose,
    pub backend: String,
    pub executable: Option<String>,
    pub executable_version: Option<String>,
    pub requested_model: String,
    /// The model actually reported or used by the CLI.
    pub model: String,
    pub requested_effort: String,
    /// The effort actually used after backend capability normalization.
    pub effort: String,
    pub session_id: Option<String>,
    pub original_request: String,
    /// Full prompt captured when the run was admitted. Unlike
    /// `original_request`, this is execution context rather than display
    /// text and is what a retry should replay when it is available.
    pub prompt_snapshot: Option<String>,
    pub working_directory: Option<String>,
    pub revision: RevisionContext,
    pub labels: RunLabels,
    pub status: RunStatus,
    pub terminal_reason: Option<TerminalReason>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub usage: Option<RunUsage>,
    pub retry_of: Option<String>,
    /// Compatibility projection of tool events for the existing cards.
    pub steps: Vec<RunStep>,
    pub error: Option<String>,
    pub queued_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub final_result: Option<String>,
    /// Whether `final_result` is only a bounded prefix. The complete stored
    /// stream remains available through paginated run events.
    pub final_result_truncated: bool,
    pub latest_activity: Option<String>,
    pub last_event_sequence: u64,
    pub mode: RunMode,
    /// A Work run's worktree recorded as a commit as it left the queue,
    /// and as it ended; `None` until then, and for Ask.
    pub work_before: Option<String>,
    pub work_after: Option<String>,
}

impl RunRecord {
    pub fn thread_id(&self) -> Option<&str> {
        self.owner.thread_id()
    }
}

impl Default for RunRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            owner: RunOwner::Unavailable {
                legacy_thread_id: None,
            },
            purpose: RunPurpose::Question,
            backend: String::new(),
            executable: None,
            executable_version: None,
            requested_model: String::new(),
            model: String::new(),
            requested_effort: String::new(),
            effort: String::new(),
            session_id: None,
            original_request: String::new(),
            prompt_snapshot: None,
            working_directory: None,
            revision: RevisionContext::Unknown,
            labels: RunLabels::default(),
            status: RunStatus::Queued,
            terminal_reason: None,
            cost_usd: None,
            duration_ms: None,
            usage: None,
            retry_of: None,
            steps: Vec::new(),
            error: None,
            queued_at: Timestamp::UNIX_EPOCH,
            started_at: None,
            ended_at: None,
            final_result: None,
            final_result_truncated: false,
            latest_activity: None,
            last_event_sequence: 0,
            mode: crate::comments::RunMode::Ask,
            work_before: None,
            work_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunStep {
    pub summary: String,
    pub ok: bool,
    /// What the agent said between its tool calls, rather than a call:
    /// `summary` holds the words.
    #[serde(default)]
    pub note: bool,
    /// A tool call whose result has not come back: what the agent is
    /// doing now, while its run goes. For a `user` step, a message the
    /// agent has not read yet.
    #[serde(default)]
    pub pending: bool,
    /// What the reader told the agent while it worked, rather than
    /// something the agent did: `summary` holds the words. One not yet
    /// read waits at the end of the steps; one read sits where it landed.
    #[serde(default)]
    pub user: bool,
}

/// One durable, ordered observation from a process or the scheduler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    State {
        status: RunStatus,
    },
    Started {
        session_id: String,
        model: Option<String>,
    },
    Text {
        text: String,
    },
    /// The reader told the agent something while it worked.
    Steer {
        text: String,
    },
    /// The agent read the oldest unread steer with this text.
    SteerLanded {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        summary: String,
        /// The delegation tool call that owns this activity. Older stored
        /// events predate hierarchy and deserialize as top-level calls.
        #[serde(default)]
        parent_id: Option<String>,
    },
    ToolResult {
        id: String,
        ok: bool,
        summary: String,
    },
    PermissionDenied {
        tool: Option<String>,
        message: String,
    },
    Usage {
        usage: RunUsage,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
    },
    Completed {
        session_id: Option<String>,
        result: String,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        usage: Option<RunUsage>,
    },
    Terminated {
        status: RunStatus,
        reason: TerminalReason,
        message: String,
    },
    /// Schema 13 retained only a rendered tool line. Migration keeps it
    /// exactly, without inventing a tool id or name.
    LegacyStep {
        summary: String,
        ok: bool,
    },
}

impl RunEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::State { .. } => "state",
            Self::Started { .. } => "started",
            Self::Text { .. } => "text",
            Self::Steer { .. } => "steer",
            Self::SteerLanded { .. } => "steer_landed",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::Usage { .. } => "usage",
            Self::Completed { .. } => "completed",
            Self::Terminated { .. } => "terminated",
            Self::LegacyStep { .. } => "legacy_step",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunEventRecord {
    pub run_id: String,
    pub sequence: u64,
    pub event_id: String,
    pub occurred_at: Timestamp,
    pub event: RunEvent,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewRunEvent {
    pub event_id: String,
    pub occurred_at: Timestamp,
    pub event: RunEvent,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunHistoryFilter {
    pub item_id: Option<String>,
    pub backend: Option<String>,
    pub status: Option<RunStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunHistoryCursor {
    pub ended_at: Timestamp,
    pub id: String,
}

/// The scalar projection used by app-wide run lists. It deliberately omits
/// steps, event text, the final result, and the retry prompt; those belong to
/// the selected run's paginated detail query.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub id: String,
    pub owner: RunOwner,
    pub purpose: RunPurpose,
    pub backend: String,
    pub model: String,
    pub effort: String,
    pub original_request: String,
    pub labels: RunLabels,
    pub status: RunStatus,
    pub terminal_reason: Option<TerminalReason>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub usage: Option<RunUsage>,
    pub retry_of: Option<String>,
    pub error: Option<String>,
    pub queued_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub latest_activity: Option<String>,
    pub last_event_sequence: u64,
    pub mode: RunMode,
    pub work_before: Option<String>,
    pub work_after: Option<String>,
}

impl RunSummary {
    pub fn thread_id(&self) -> Option<&str> {
        self.owner.thread_id()
    }
}

impl From<&RunRecord> for RunSummary {
    fn from(run: &RunRecord) -> Self {
        Self {
            id: run.id.clone(),
            owner: run.owner.clone(),
            purpose: run.purpose,
            backend: run.backend.clone(),
            model: run.model.clone(),
            effort: run.effort.clone(),
            original_request: run.original_request.clone(),
            labels: run.labels.clone(),
            status: run.status,
            terminal_reason: run.terminal_reason,
            cost_usd: run.cost_usd,
            duration_ms: run.duration_ms,
            usage: run.usage.clone(),
            retry_of: run.retry_of.clone(),
            error: run.error.clone(),
            queued_at: run.queued_at,
            started_at: run.started_at,
            ended_at: run.ended_at,
            latest_activity: run.latest_activity.clone(),
            last_event_sequence: run.last_event_sequence,
            mode: run.mode,
            work_before: run.work_before.clone(),
            work_after: run.work_after.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunHistoryPage {
    pub runs: Vec<RunSummary>,
    pub next_cursor: Option<RunHistoryCursor>,
}

/// A fresh id: time-ordered, unique on this machine, safe in a URL.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = Timestamp::now().as_nanosecond();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}-{:x}", nanos, std::process::id(), n)
}

pub(crate) mod sql {
    //! The rows, so the cache module stays about SQL and not about parsing.

    use super::*;

    pub(crate) fn origin(s: &str) -> ThreadOrigin {
        ThreadOrigin::parse(s)
    }

    pub(crate) fn side(s: &str) -> Side {
        Side::parse(s)
    }

    pub(crate) fn author(kind: &str, name: Option<String>, model: Option<String>) -> Author {
        match kind {
            "agent" => Author::Agent {
                backend: name.unwrap_or_default(),
                model: model.unwrap_or_default(),
            },
            "github_user" => Author::GitHubUser(name.unwrap_or_default()),
            _ => Author::User,
        }
    }

    pub(crate) fn author_columns(a: &Author) -> (&'static str, Option<String>) {
        match a {
            Author::User => ("user", None),
            Author::Agent { backend, .. } => ("agent", Some(backend.clone())),
            Author::GitHubUser(login) => ("github_user", Some(login.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_label_a_line_or_a_range() {
        let one = Anchor {
            path: "src/x.rs".into(),
            start_line: 41,
            line: 41,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        assert_eq!(one.label(), "src/x.rs:41");
        let range = Anchor { line: 58, ..one };
        assert_eq!(range.label(), "src/x.rs:41–58");
    }

    #[test]
    fn ids_are_unique_and_ordered() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(!a.contains('/'));
    }

    fn run(backend: &str, session: &str, status: RunStatus) -> RunRecord {
        RunRecord {
            id: new_id(),
            owner: RunOwner::Discussion {
                item_id: "w".into(),
                thread_id: "t".into(),
                request_id: "q".into(),
            },
            purpose: RunPurpose::Question,
            backend: backend.into(),
            executable: None,
            executable_version: None,
            requested_model: "m".into(),
            model: "m".into(),
            requested_effort: "medium".into(),
            effort: "medium".into(),
            session_id: Some(session.into()),
            original_request: "why?".into(),
            prompt_snapshot: Some("full prompt: why?".into()),
            working_directory: None,
            revision: RevisionContext::Unknown,
            labels: RunLabels::default(),
            status,
            terminal_reason: status.is_terminal().then_some(TerminalReason::Other),
            cost_usd: None,
            duration_ms: None,
            usage: None,
            retry_of: None,
            steps: Vec::new(),
            error: None,
            queued_at: Timestamp::UNIX_EPOCH,
            started_at: Some(Timestamp::UNIX_EPOCH),
            ended_at: status.is_terminal().then_some(Timestamp::UNIX_EPOCH),
            final_result: None,
            final_result_truncated: false,
            latest_activity: None,
            last_event_sequence: 0,
            mode: crate::comments::RunMode::Ask,
            work_before: None,
            work_after: None,
        }
    }

    fn thread_of(runs: Vec<RunRecord>) -> CommentThread {
        let messages = runs
            .into_iter()
            .map(|r| CommentMessage {
                id: new_id(),
                thread_id: "t".into(),
                origin: ThreadOrigin::Local,
                author: Author::Agent {
                    backend: r.backend.clone(),
                    model: r.model.clone(),
                },
                body: String::new(),
                created_at: Timestamp::UNIX_EPOCH,
                run: Some(r),
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Reactions::default(),
            })
            .collect();
        CommentThread {
            id: "t".into(),
            worktree_id: "w".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: None,
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: Timestamp::UNIX_EPOCH,
            messages,
        }
    }

    #[test]
    fn a_follow_up_resumes_only_a_session_of_its_own_backend() {
        let t = thread_of(vec![run("codex", "thread-1", RunStatus::Completed)]);
        assert_eq!(t.resumable_session("codex").as_deref(), Some("thread-1"));
        assert_eq!(t.resumable_session("claude-code"), None);
    }

    #[test]
    fn switching_backends_does_not_reach_past_the_last_finished_run() {
        let t = thread_of(vec![
            run("claude-code", "sess-1", RunStatus::Completed),
            run("codex", "thread-2", RunStatus::Completed),
        ]);
        assert_eq!(t.resumable_session("codex").as_deref(), Some("thread-2"));
        assert_eq!(
            t.resumable_session("claude-code"),
            None,
            "the newer Codex run made its history stale"
        );
    }

    #[test]
    fn a_newer_failed_run_does_not_hide_an_older_finished_one() {
        let t = thread_of(vec![
            run("claude-code", "sess-1", RunStatus::Completed),
            run("claude-code", "sess-2", RunStatus::Failed),
        ]);
        assert_eq!(
            t.resumable_session("claude-code").as_deref(),
            Some("sess-1")
        );
        let none = thread_of(vec![run("claude-code", "sess-1", RunStatus::Failed)]);
        assert_eq!(none.resumable_session("claude-code"), None);
    }

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn comment(id: u64, reply_to: Option<u64>, at_s: &str) -> ReviewComment {
        ReviewComment {
            id,
            node_id: format!("PRRC_{id}"),
            pull_request_review_id: Some(9),
            in_reply_to_id: reply_to,
            user: format!("u{id}"),
            user_avatar_url: String::new(),
            body: format!("c{id}"),
            path: "a.rs".into(),
            line: Some(41),
            original_line: Some(40),
            start_line: None,
            original_start_line: None,
            diff_hunk: "@@ -40,3 +40,3 @@".into(),
            side: Side::Right,
            start_side: None,
            commit_id: "head".into(),
            original_commit_id: "orig".into(),
            created_at: at(at_s),
            updated_at: None,
            html_url: format!("https://github.com/a/b/pull/1#discussion_r{id}"),
            reactions: Reactions::default(),
            viewer_can_delete: false,
            viewer_can_update: false,
        }
    }

    fn discussion(comments: Vec<ReviewComment>) -> Discussion {
        Discussion {
            review_comments: comments,
            ..Discussion::default()
        }
    }

    #[test]
    fn replies_nest_under_their_root_and_a_reply_whose_root_is_missing_is_its_own_root() {
        let d = discussion(vec![
            comment(3, Some(1), "2026-09-16T11:40:00Z"),
            comment(2, Some(1), "2026-09-16T11:39:00Z"),
            comment(1, None, "2026-09-16T11:38:00Z"),
            comment(7, Some(6), "2026-09-16T11:30:00Z"),
        ]);
        let threads = review_threads_of(&d, "w", "fallback");
        let ids: Vec<_> = threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["gh-7", "gh-1"],
            "threads sort by their root's time"
        );
        let first = &threads[1];
        assert_eq!(first.github_id, Some(1));
        assert_eq!(first.origin, ThreadOrigin::GitHub);
        assert_eq!(first.worktree_id, "w");
        assert_eq!(first.created_at, at("2026-09-16T11:38:00Z"));
        let messages: Vec<_> = first.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(messages, vec!["gh-1", "gh-2", "gh-3"]);
        let root = &first.messages[0];
        assert_eq!(root.author, Author::GitHubUser("u1".into()));
        assert_eq!(root.thread_id, "gh-1");
        assert_eq!(root.github_id, Some(1));
        assert_eq!(root.node_id.as_deref(), Some("PRRC_1"));
        assert_eq!(
            root.html_url.as_deref(),
            Some("https://github.com/a/b/pull/1#discussion_r1")
        );
        assert_eq!(root.diff_hunk.as_deref(), Some("@@ -40,3 +40,3 @@"));
        assert!(root.from_github() && !root.is_draft() && !root.is_private());
        assert_eq!(first.messages[2].thread_id, "gh-1");
        assert!(first.is_github_bound() && first.has_github_message());
        assert_eq!(threads[0].messages.len(), 1);
    }

    #[test]
    fn a_comment_on_a_file_is_on_the_lines_its_permalink_names() {
        let body = file_comment_body(
            "https://github.com/a/b",
            "orig",
            "src/my file.rs",
            861,
            863,
            "Unsigned types are untested.",
        );
        assert_eq!(
            body,
            "https://github.com/a/b/blob/orig/src/my%20file.rs#L861-L863\n\n\
             Unsigned types are untested."
        );
        let mut root = comment(1, None, "2026-09-16T11:38:00Z");
        root.path = "src/my file.rs".into();
        root.line = None;
        root.original_line = None;
        root.body = body;
        let threads = review_threads_of(&discussion(vec![root.clone()]), "w", "fallback");
        let thread = &threads[0];
        assert_eq!(
            thread.anchor,
            Some(Anchor {
                path: "src/my file.rs".into(),
                start_line: 861,
                line: 863,
                side: Side::Right,
                commit_sha: "orig".into(),
            })
        );
        assert!(!thread.outdated, "a comment on a file stays on it");
        assert_eq!(thread.messages[0].body, "Unsigned types are untested.");

        // A link at another commit is to the base side's lines.
        root.body = file_comment_body(
            "https://github.com/a/b",
            "base",
            "src/my file.rs",
            7,
            7,
            "x",
        );
        let (anchor, body) = file_comment_anchor(&root, "fallback").unwrap();
        assert_eq!(
            (anchor.side, anchor.start_line, anchor.line),
            (Side::Left, 7, 7)
        );
        assert_eq!(anchor.commit_sha, "orig");
        assert_eq!(body, "x");

        // Anything else on a file has no lines.
        for other in [
            "Looks fine.".to_string(),
            file_comment_body(
                "https://github.com/o/other",
                "orig",
                "src/my file.rs",
                7,
                7,
                "x",
            ),
            file_comment_body("https://github.com/a/b", "orig", "src/other.rs", 7, 7, "x"),
            "https://github.com/a/b/blob/orig/src/my%20file.rs#L9-L3".to_string(),
        ] {
            root.body = other;
            assert_eq!(
                file_comment_anchor(&root, "fallback"),
                None,
                "{}",
                root.body
            );
        }
        let threads = review_threads_of(&discussion(vec![root.clone()]), "w", "fallback");
        assert_eq!(threads[0].anchor, None);
        assert!(threads[0].outdated);

        // A comment on lines is on them, whatever it opens with.
        let mut on_lines = comment(2, None, "2026-09-16T11:38:00Z");
        on_lines.body = file_comment_body("https://github.com/a/b", "orig", "a.rs", 7, 7, "x");
        assert_eq!(file_comment_anchor(&on_lines, "fallback"), None);
    }

    #[test]
    fn comments_of_a_pending_review_are_left_out() {
        let mut d = discussion(vec![
            comment(1, None, "2026-09-16T11:38:00Z"),
            comment(2, None, "2026-09-16T11:39:00Z"),
        ]);
        d.review_comments[1].pull_request_review_id = Some(10);
        d.reviews = vec![crate::discussion::Review {
            viewer_can_update: false,
            id: 10,
            node_id: String::new(),
            user: "me".into(),
            body: String::new(),
            state: "PENDING".into(),
            submitted_at: None,
            html_url: String::new(),
            reactions: Reactions::default(),
        }];
        let threads = review_threads_of(&d, "w", "");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "gh-1");
    }

    #[test]
    fn the_anchor_is_the_head_line_and_commit_when_the_head_has_it() {
        let mut root = comment(1, None, "2026-09-16T11:38:00Z");
        root.start_line = Some(39);
        root.original_start_line = Some(30);
        let threads = review_threads_of(&discussion(vec![root]), "w", "fallback");
        assert_eq!(
            threads[0].anchor,
            Some(Anchor {
                path: "a.rs".into(),
                start_line: 39,
                line: 41,
                side: Side::Right,
                commit_sha: "head".into(),
            })
        );
        assert!(!threads[0].outdated);
        assert!(!threads[0].resolved);
    }

    #[test]
    fn a_comment_the_head_no_longer_has_anchors_where_it_was_left_and_is_outdated() {
        let mut root = comment(1, None, "2026-09-16T11:38:00Z");
        root.line = None;
        root.start_line = None;
        root.original_start_line = Some(30);
        let threads = review_threads_of(&discussion(vec![root]), "w", "fallback");
        assert_eq!(
            threads[0].anchor,
            Some(Anchor {
                path: "a.rs".into(),
                start_line: 30,
                line: 40,
                side: Side::Right,
                commit_sha: "orig".into(),
            })
        );
        assert!(
            threads[0].outdated,
            "no head line means outdated without GitHub's word"
        );
    }

    #[test]
    fn github_says_when_a_thread_is_resolved_or_outdated() {
        let mut root = comment(1, None, "2026-09-16T11:38:00Z");
        root.line = None;
        let mut d = discussion(vec![root, comment(2, None, "2026-09-16T11:39:00Z")]);
        d.review_threads = vec![
            crate::discussion::ReviewThreadState {
                node_id: "PRRT_1".into(),
                root_comment_id: 1,
                resolved: true,
                outdated: false,
                can_resolve: true,
            },
            crate::discussion::ReviewThreadState {
                node_id: "PRRT_2".into(),
                root_comment_id: 2,
                resolved: false,
                outdated: true,
                can_resolve: false,
            },
        ];
        let threads = review_threads_of(&d, "w", "");
        assert!(
            (threads[0].resolved, threads[0].outdated) == (true, false),
            "GitHub's word wins over the missing line"
        );
        assert!((threads[1].resolved, threads[1].outdated) == (false, true));
        // Resolving goes by the thread's own id, where GitHub allows it.
        assert_eq!(threads[0].node_id.as_deref(), Some("PRRT_1"));
        assert!(threads[0].resolvable());
        assert_eq!(threads[1].node_id.as_deref(), Some("PRRT_2"));
        assert!(!threads[1].resolvable(), "GitHub does not let the reader");
    }

    #[test]
    fn a_thread_github_said_nothing_about_cannot_be_resolved() {
        let threads = review_threads_of(
            &discussion(vec![comment(1, None, "2026-09-16T11:38:00Z")]),
            "w",
            "",
        );
        assert_eq!(threads[0].node_id, None);
        assert!(!threads[0].can_resolve);
        assert!(!threads[0].resolvable());
    }

    #[test]
    fn a_thread_not_on_github_resolves_in_perch() {
        let mut t = thread_of(Vec::new());
        assert!(!t.on_github());
        assert!(t.resolvable(), "Perch's own");
        t.origin = ThreadOrigin::GitHub;
        assert!(t.resolvable(), "on its way with a draft, and not there yet");
        t.github_id = Some(3);
        assert!(
            !t.resolvable(),
            "on GitHub, and GitHub has not said the reader may"
        );
    }

    #[test]
    fn a_root_without_a_commit_is_anchored_at_the_head_passed_in_and_without_a_line_at_nothing() {
        let mut root = comment(1, None, "2026-09-16T11:38:00Z");
        root.commit_id = String::new();
        let mut nowhere = comment(2, None, "2026-09-16T11:39:00Z");
        nowhere.line = None;
        nowhere.original_line = None;
        let threads = review_threads_of(&discussion(vec![root, nowhere]), "w", "fallback");
        assert_eq!(threads[0].anchor.as_ref().unwrap().commit_sha, "fallback");
        assert_eq!(threads[1].anchor, None);
        assert!(threads[1].outdated);
    }
}
