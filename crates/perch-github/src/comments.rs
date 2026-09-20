//! Comment threads anchored to lines of a worktree, and the agent runs
//! that answer in them.
//!
//! Shaped like GitHub's review threads on purpose: `side` and `start_line`
//! are its names, so review threads fetched later can sit in the same list
//! with the same anchor type. Nothing here fetches or posts one; the
//! `origin` column exists so that milestone needs no migration.
//!
//! `CommentThread` rather than `Thread`: that word is the notification.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

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
    /// Perch's own id for a local thread, GitHub's node id later.
    pub id: String,
    pub worktree_id: String,
    pub origin: ThreadOrigin,
    /// `None` for a thread about the worktree as a whole.
    pub anchor: Option<Anchor>,
    /// The file changed after the anchor was made; the lines may have moved.
    pub outdated: bool,
    pub resolved: bool,
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
        self.messages
            .iter()
            .rev()
            .filter_map(|m| m.run.as_ref())
            .find(|r| r.status == RunStatus::Completed)
            .filter(|r| r.backend == backend)
            .and_then(|r| r.session_id.clone())
    }

    pub fn running_run(&self) -> Option<&RunRecord> {
        self.messages
            .iter()
            .filter_map(|m| m.run.as_ref())
            .find(|r| r.status.is_active())
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
    pub origin: ThreadOrigin,
    pub author: Author,
    pub body: String,
    pub created_at: Timestamp,
    /// An agent's answer carries the run that produced it. A user's
    /// question carries the run that is answering it while it runs, so
    /// the thread has somewhere to show the steps before there is an
    /// answer.
    pub run: Option<RunRecord>,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStep {
    pub summary: String,
    pub ok: bool,
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
            })
            .collect();
        CommentThread {
            id: "t".into(),
            worktree_id: "w".into(),
            origin: ThreadOrigin::Local,
            anchor: None,
            outdated: false,
            resolved: false,
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
}
