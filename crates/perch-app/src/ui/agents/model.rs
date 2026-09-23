//! The small, app-wide projection the Agents panel draws.
//!
//! A durable run remains the backend's record.  These summaries are what the
//! window needs in order to group, filter and inspect one without loading an
//! item's whole discussion.  Keeping the projection here also keeps Floem out
//! of the run manager and the database.

use std::collections::HashSet;
use std::sync::Arc;

use jiff::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsTab {
    #[default]
    Active,
    History,
}

impl AgentsTab {
    pub const ALL: [Self; 2] = [Self::Active, Self::History];

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::History => "History",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorktreeFilter {
    #[default]
    All,
    Current,
}

impl WorktreeFilter {
    pub const ALL: [Self; 2] = [Self::All, Self::Current];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All worktrees",
            Self::Current => "Current worktree",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendFilter {
    #[default]
    All,
    Claude,
    Codex,
}

impl BackendFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Claude, Self::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All agents",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    pub fn includes(self, backend: &str) -> bool {
        match self {
            Self::All => true,
            Self::Claude => {
                backend.eq_ignore_ascii_case("claude-code")
                    || backend.eq_ignore_ascii_case("claude")
            }
            Self::Codex => backend.eq_ignore_ascii_case("codex"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutcomeFilter {
    #[default]
    All,
    Completed,
    Failed,
    Stopped,
    Interrupted,
}

impl OutcomeFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Completed,
        Self::Failed,
        Self::Stopped,
        Self::Interrupted,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All outcomes",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Interrupted => "Interrupted",
        }
    }

    pub fn includes(self, status: AgentRunStatus) -> bool {
        match self {
            Self::All => true,
            Self::Completed => status == AgentRunStatus::Completed,
            Self::Failed => status == AgentRunStatus::Failed,
            Self::Stopped => status == AgentRunStatus::Stopped,
            Self::Interrupted => status == AgentRunStatus::Interrupted,
        }
    }
}

/// The lifecycle names the panel understands.  `parse` accepts both the
/// former `finished` spelling and the manager's `completed` spelling so a
/// schema upgrade never changes what an old run looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AgentRunStatus {
    Queued,
    Starting,
    #[default]
    Running,
    Stopping,
    Completed,
    Failed,
    Stopped,
    Interrupted,
}

impl AgentRunStatus {
    pub fn parse(name: &str) -> Self {
        match name {
            "queued" => Self::Queued,
            "starting" => Self::Starting,
            "stopping" => Self::Stopping,
            "completed" | "finished" => Self::Completed,
            "failed" => Self::Failed,
            "stopped" => Self::Stopped,
            "interrupted" => Self::Interrupted,
            _ => Self::Running,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Interrupted => "Interrupted",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Starting | Self::Running | Self::Stopping
        )
    }

    pub fn is_queued(self) -> bool {
        self == Self::Queued
    }

    pub fn may_stop(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    pub fn may_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped | Self::Interrupted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentRunPurpose {
    #[default]
    Question,
    FollowUp,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRetryTarget {
    Run(String),
    ReviewAttempt(String),
}

impl AgentRunPurpose {
    pub fn label(self) -> &'static str {
        match self {
            Self::Question => "Question",
            Self::FollowUp => "Follow-up",
            Self::Review => "Worktree review",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunStep {
    pub sequence: u64,
    pub summary: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunSummary {
    pub id: String,
    pub item_id: Option<String>,
    pub thread_id: Option<String>,
    /// Stable reviewer-attempt identity for native review children. The
    /// ordinary run id is deliberately different: retry and navigation must
    /// act on the review attempt/group, not replay the generic run request.
    pub review_attempt_id: Option<String>,
    pub repository: String,
    pub worktree: String,
    pub purpose: AgentRunPurpose,
    pub request: String,
    pub backend: String,
    pub model: String,
    pub effort: String,
    pub mode: perch_github::RunMode,
    pub status: AgentRunStatus,
    pub terminal_reason: Option<perch_github::TerminalReason>,
    pub queue_position: Option<usize>,
    pub queued_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub latest_activity: Option<String>,
    pub steps: Arc<Vec<AgentRunStep>>,
    pub response: String,
    pub response_truncated: bool,
    pub error: Option<String>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub retry_of: Option<String>,
    pub source_available: bool,
    pub last_event_sequence: u64,
}

impl AgentRunSummary {
    /// Durable snapshots can arrive out of order while a run is streaming.
    /// Sequence is the projection's revision; once terminal, a run never
    /// returns to an active lifecycle even if an older page arrives late.
    pub fn can_replace_with(&self, candidate: &Self) -> bool {
        !(!self.status.is_active() && candidate.status.is_active())
            && candidate.last_event_sequence >= self.last_event_sequence
    }

    pub fn backend_label(&self) -> &str {
        if self.backend.eq_ignore_ascii_case("codex") {
            "Codex"
        } else if self.backend.eq_ignore_ascii_case("claude-code")
            || self.backend.eq_ignore_ascii_case("claude")
        {
            "Claude"
        } else {
            &self.backend
        }
    }

    pub fn retry_target(&self) -> AgentRetryTarget {
        self.review_attempt_id
            .clone()
            .map(AgentRetryTarget::ReviewAttempt)
            .unwrap_or_else(|| AgentRetryTarget::Run(self.id.clone()))
    }

    pub fn subject(&self) -> String {
        let first = self.request.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            self.purpose.label().to_string()
        } else {
            first.to_string()
        }
    }

    pub fn group_label(&self) -> String {
        match (self.repository.is_empty(), self.worktree.is_empty()) {
            (false, false) => format!("{}  /  {}", self.repository, self.worktree),
            (false, true) => self.repository.clone(),
            (true, false) => self.worktree.clone(),
            (true, true) => "Unavailable worktree".to_string(),
        }
    }

    pub fn terminal_summary(&self) -> Option<&'static str> {
        use perch_github::TerminalReason;
        Some(match self.terminal_reason? {
            TerminalReason::Completed => "Completed",
            TerminalReason::QueueCancelled => "Canceled before starting",
            TerminalReason::UserStopped => "Stopped by the user",
            TerminalReason::SpawnFailed => "The agent could not be started",
            TerminalReason::PermissionDenied => "The agent was denied permission",
            TerminalReason::MalformedStream => "The agent returned malformed output",
            TerminalReason::UnexpectedExit => "The agent exited unexpectedly",
            TerminalReason::AppShutdown => "Interrupted while Perch was closing",
            TerminalReason::AppRestart => "Interrupted by an app restart",
            TerminalReason::UnavailableConfiguration => {
                "The saved agent configuration is unavailable"
            }
            TerminalReason::Other => "The run did not complete",
        })
    }

    /// Put the typed outcome first, then retain any process diagnostic that
    /// adds information. UI code must not infer permission or spawn failures
    /// from arbitrary CLI prose.
    pub fn terminal_display(&self) -> Option<String> {
        let summary = self.terminal_summary().map(str::to_string);
        let detail = self
            .error
            .as_deref()
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
            .map(str::to_string);
        match (summary, detail) {
            (Some(summary), Some(detail)) if !detail.eq_ignore_ascii_case(&summary) => {
                Some(format!("{summary} — {detail}"))
            }
            (Some(summary), _) => Some(summary),
            (None, detail) => detail,
        }
    }

    pub fn from_record(run: &perch_github::RunRecord) -> Self {
        use perch_github::comments::RunPurpose;

        let usage = run.usage.as_ref();
        let review_attempt_id = match &run.owner {
            perch_github::RunOwner::Review {
                reviewer_run_id, ..
            } => Some(reviewer_run_id.clone()),
            _ => None,
        };
        Self {
            id: run.id.clone(),
            item_id: run.owner.item_id().map(str::to_string),
            thread_id: run.thread_id().map(str::to_string),
            review_attempt_id,
            repository: run.labels.repository.clone(),
            worktree: run
                .labels
                .branch
                .clone()
                .unwrap_or_else(|| run.labels.item.clone()),
            purpose: match run.purpose {
                RunPurpose::Question => AgentRunPurpose::Question,
                RunPurpose::FollowUp => AgentRunPurpose::FollowUp,
                RunPurpose::WorktreeReview => AgentRunPurpose::Review,
            },
            request: run.original_request.clone(),
            backend: run.backend.clone(),
            model: run.model.clone(),
            effort: run.effort.clone(),
            mode: run.mode,
            status: AgentRunStatus::parse(run.status.name()),
            terminal_reason: run.terminal_reason,
            queue_position: None,
            queued_at: run.queued_at,
            started_at: run.started_at,
            ended_at: run.ended_at,
            latest_activity: run
                .latest_activity
                .clone()
                .or_else(|| run.steps.last().map(|step| step.summary.clone())),
            steps: Arc::new(
                run.steps
                    .iter()
                    .enumerate()
                    .map(|(sequence, step)| AgentRunStep {
                        sequence: sequence as u64,
                        summary: step.summary.clone(),
                        ok: step.ok,
                    })
                    .collect(),
            ),
            response: run.final_result.clone().unwrap_or_default(),
            response_truncated: run.final_result_truncated,
            error: run.error.clone(),
            cost_usd: run.cost_usd,
            input_tokens: usage.and_then(|usage| usage.input_tokens),
            output_tokens: usage.and_then(|usage| usage.output_tokens),
            cached_input_tokens: usage.and_then(|usage| usage.cached_input_tokens),
            retry_of: run.retry_of.clone(),
            source_available: run.owner.item_id().is_some(),
            last_event_sequence: run.last_event_sequence,
        }
    }

    /// Build the list projection without hydrating transcript-only fields.
    /// Opening a row replaces it with a full `RunRecord` through
    /// `AppState::update_agent_run` and loads its persisted events separately.
    pub fn from_summary(run: &perch_github::RunSummary) -> Self {
        use perch_github::comments::RunPurpose;

        let usage = run.usage.as_ref();
        let review_attempt_id = match &run.owner {
            perch_github::RunOwner::Review {
                reviewer_run_id, ..
            } => Some(reviewer_run_id.clone()),
            _ => None,
        };
        Self {
            id: run.id.clone(),
            item_id: run.owner.item_id().map(str::to_string),
            thread_id: run.thread_id().map(str::to_string),
            review_attempt_id,
            repository: run.labels.repository.clone(),
            worktree: run
                .labels
                .branch
                .clone()
                .unwrap_or_else(|| run.labels.item.clone()),
            purpose: match run.purpose {
                RunPurpose::Question => AgentRunPurpose::Question,
                RunPurpose::FollowUp => AgentRunPurpose::FollowUp,
                RunPurpose::WorktreeReview => AgentRunPurpose::Review,
            },
            request: run.original_request.clone(),
            backend: run.backend.clone(),
            model: run.model.clone(),
            effort: run.effort.clone(),
            mode: run.mode,
            status: AgentRunStatus::parse(run.status.name()),
            terminal_reason: run.terminal_reason,
            queue_position: None,
            queued_at: run.queued_at,
            started_at: run.started_at,
            ended_at: run.ended_at,
            latest_activity: run.latest_activity.clone(),
            steps: Arc::new(Vec::new()),
            response: String::new(),
            response_truncated: false,
            error: run.error.clone(),
            cost_usd: run.cost_usd,
            input_tokens: usage.and_then(|usage| usage.input_tokens),
            output_tokens: usage.and_then(|usage| usage.output_tokens),
            cached_input_tokens: usage.and_then(|usage| usage.cached_input_tokens),
            retry_of: run.retry_of.clone(),
            source_available: run.owner.item_id().is_some(),
            last_event_sequence: run.last_event_sequence,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunGroup {
    pub item_id: Option<String>,
    pub label: String,
    pub current: bool,
    pub runs: Vec<AgentRunSummary>,
}

/// One row in a worktree/history run list. Native reviewer children stay
/// beneath their durable review group; ordinary question and follow-up rows
/// retain their existing flat presentation.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentRunEntry {
    Run(AgentRunSummary),
    ReviewGroup(AgentReviewRunGroup),
}

impl AgentRunEntry {
    pub fn key(&self) -> String {
        match self {
            Self::Run(run) => format!("run:{}", run.id),
            Self::ReviewGroup(group) => format!("review:{}", group.key),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentReviewRunGroup {
    /// The durable group id when its metadata has arrived. A just-admitted
    /// attempt may briefly use its attempt id as a stable placeholder key.
    pub key: String,
    pub summary: Option<crate::review::model::ReviewGroupSummary>,
    pub runs: Vec<AgentRunSummary>,
}

/// Find the durable group which owns one reviewer attempt.
pub fn review_group_for_attempt<'a>(
    groups: &'a [crate::review::model::ReviewGroupSummary],
    attempt_id: &str,
) -> Option<&'a crate::review::model::ReviewGroupSummary> {
    groups.iter().find(|summary| {
        summary
            .attempts
            .iter()
            .any(|attempt| attempt.id == attempt_id)
    })
}

/// Preserve the caller's established ordering while replacing reviewer rows
/// with one expandable group entry at the position of its first child.
pub fn group_review_runs(
    runs: &[AgentRunSummary],
    groups: &[crate::review::model::ReviewGroupSummary],
) -> Vec<AgentRunEntry> {
    let mut entries = Vec::new();
    let mut emitted = HashSet::new();
    for run in runs {
        let Some(attempt_id) = run.review_attempt_id.as_deref() else {
            entries.push(AgentRunEntry::Run(run.clone()));
            continue;
        };
        let summary = review_group_for_attempt(groups, attempt_id).cloned();
        let key = summary
            .as_ref()
            .map(|summary| summary.group.id.clone())
            .unwrap_or_else(|| format!("attempt:{attempt_id}"));
        if !emitted.insert(key.clone()) {
            continue;
        }
        let children = match summary.as_ref() {
            Some(summary) => {
                let attempts = summary
                    .attempts
                    .iter()
                    .map(|attempt| attempt.id.as_str())
                    .collect::<HashSet<_>>();
                runs.iter()
                    .filter(|candidate| {
                        candidate
                            .review_attempt_id
                            .as_deref()
                            .is_some_and(|attempt| attempts.contains(attempt))
                    })
                    .cloned()
                    .collect()
            }
            None => vec![run.clone()],
        };
        entries.push(AgentRunEntry::ReviewGroup(AgentReviewRunGroup {
            key,
            summary,
            runs: children,
        }));
    }
    entries
}

/// Filter and group active rows. Groups keep the order in which their first
/// run was admitted, except that the open worktree is lifted to the front.
#[cfg(test)]
pub fn active_groups(
    runs: &[AgentRunSummary],
    current_item: Option<&str>,
    worktrees: WorktreeFilter,
    backends: BackendFilter,
) -> Vec<AgentRunGroup> {
    active_groups_with_reviews(runs, current_item, worktrees, backends, &[])
}

pub fn active_groups_with_reviews(
    runs: &[AgentRunSummary],
    current_item: Option<&str>,
    worktrees: WorktreeFilter,
    backends: BackendFilter,
    review_groups: &[crate::review::model::ReviewGroupSummary],
) -> Vec<AgentRunGroup> {
    let active_review_attempts = review_groups
        .iter()
        .filter(|summary| summary.group.status.is_active())
        .flat_map(|summary| summary.attempts.iter().map(|attempt| attempt.id.as_str()))
        .collect::<HashSet<_>>();
    let mut groups: Vec<AgentRunGroup> = Vec::new();
    for run in runs.iter().filter(|run| {
        (run.status.is_active()
            || run
                .review_attempt_id
                .as_deref()
                .is_some_and(|attempt| active_review_attempts.contains(attempt)))
            && backends.includes(&run.backend)
            && (worktrees == WorktreeFilter::All || run.item_id.as_deref() == current_item)
    }) {
        let index = groups
            .iter()
            .position(|group| group.item_id == run.item_id)
            .unwrap_or_else(|| {
                groups.push(AgentRunGroup {
                    item_id: run.item_id.clone(),
                    label: run.group_label(),
                    current: run.item_id.as_deref() == current_item,
                    runs: Vec::new(),
                });
                groups.len() - 1
            });
        groups[index].runs.push(run.clone());
    }
    for group in &mut groups {
        group
            .runs
            .sort_by_key(|run| (run.status.is_queued(), run.queued_at));
    }
    groups.sort_by_key(|group| {
        (
            !group.current,
            group
                .runs
                .iter()
                .map(|run| run.queued_at)
                .min()
                .unwrap_or(Timestamp::MAX),
        )
    });
    groups
}

#[cfg(test)]
pub fn history_runs(
    runs: &[AgentRunSummary],
    current_item: Option<&str>,
    worktrees: WorktreeFilter,
    backends: BackendFilter,
    outcomes: OutcomeFilter,
) -> Vec<AgentRunSummary> {
    history_runs_with_reviews(runs, current_item, worktrees, backends, outcomes, &[])
}

pub fn history_runs_with_reviews(
    runs: &[AgentRunSummary],
    current_item: Option<&str>,
    worktrees: WorktreeFilter,
    backends: BackendFilter,
    outcomes: OutcomeFilter,
    review_groups: &[crate::review::model::ReviewGroupSummary],
) -> Vec<AgentRunSummary> {
    let active_review_attempts = review_groups
        .iter()
        .filter(|summary| summary.group.status.is_active())
        .flat_map(|summary| summary.attempts.iter().map(|attempt| attempt.id.as_str()))
        .collect::<HashSet<_>>();
    let mut rows: Vec<_> = runs
        .iter()
        .filter(|run| {
            !run.status.is_active()
                && !run
                    .review_attempt_id
                    .as_deref()
                    .is_some_and(|attempt| active_review_attempts.contains(attempt))
                && backends.includes(&run.backend)
                && outcomes.includes(run.status)
                && (worktrees == WorktreeFilter::All || run.item_id.as_deref() == current_item)
        })
        .cloned()
        .collect();
    rows.sort_by_key(|run| std::cmp::Reverse(run.ended_at.unwrap_or(run.queued_at)));
    rows
}

/// One item's runs as its Agents row lists them: what is working first,
/// in the order it was admitted with queued work last, then what has
/// finished, newest first. A reviewer attempt whose review is still going
/// counts as working, so a review's attempts stay together.
pub fn item_runs(
    runs: &[AgentRunSummary],
    item_id: &str,
    review_groups: &[crate::review::model::ReviewGroupSummary],
) -> Vec<AgentRunSummary> {
    let active_review_attempts = review_groups
        .iter()
        .filter(|summary| summary.group.status.is_active())
        .flat_map(|summary| summary.attempts.iter().map(|attempt| attempt.id.as_str()))
        .collect::<HashSet<_>>();
    let working = |run: &AgentRunSummary| {
        run.status.is_active()
            || run
                .review_attempt_id
                .as_deref()
                .is_some_and(|attempt| active_review_attempts.contains(attempt))
    };
    let (mut active, mut finished): (Vec<_>, Vec<_>) = runs
        .iter()
        .filter(|run| run.item_id.as_deref() == Some(item_id))
        .cloned()
        .partition(|run| working(run));
    active.sort_by_key(|run| (run.status.is_queued(), run.queued_at));
    finished.sort_by_key(|run| std::cmp::Reverse(run.ended_at.unwrap_or(run.queued_at)));
    active.extend(finished);
    active
}

/// What an item's runs are doing, for the pill on its Agents row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunCounts {
    pub running: usize,
    pub queued: usize,
    /// Running, and waiting on the reader to answer a request.
    pub waiting: usize,
    pub done: usize,
    pub failed: usize,
    /// Whether the run that finished last failed.
    pub last_failed: bool,
}

impl RunCounts {
    /// Counted over `runs`, one item's; `waiting` is the ids of runs
    /// with a request pending.
    pub fn of(runs: &[AgentRunSummary], waiting: &HashSet<String>) -> Self {
        let mut counts = Self::default();
        for run in runs {
            match run.status {
                _ if run.status.is_active() && waiting.contains(&run.id) => counts.waiting += 1,
                AgentRunStatus::Queued => counts.queued += 1,
                AgentRunStatus::Starting | AgentRunStatus::Running | AgentRunStatus::Stopping => {
                    counts.running += 1
                }
                AgentRunStatus::Completed | AgentRunStatus::Stopped => counts.done += 1,
                AgentRunStatus::Failed | AgentRunStatus::Interrupted => counts.failed += 1,
            }
        }
        counts.last_failed = runs
            .iter()
            .filter(|run| !run.status.is_active())
            .max_by_key(|run| run.ended_at.unwrap_or(run.queued_at))
            .is_some_and(|run| matches!(run.status, AgentRunStatus::Failed | AgentRunStatus::Interrupted));
        counts
    }

    pub fn total(&self) -> usize {
        self.running + self.queued + self.waiting + self.done + self.failed
    }

    pub fn active(&self) -> bool {
        self.running + self.queued + self.waiting > 0
    }

    /// The pill's words: while anything works, each state that has runs,
    /// the busiest first; after, how many there were, or that the last
    /// one failed.
    pub fn label(&self) -> String {
        if self.active() {
            [
                (self.running, "running"),
                (self.queued, "queued"),
                (self.waiting, "waiting"),
                (self.done, "done"),
                (self.failed, "failed"),
            ]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, word)| format!("{n} {word}"))
            .collect::<Vec<_>>()
            .join(" · ")
        } else if self.last_failed {
            "Last run failed".to_string()
        } else if self.total() == 1 {
            "1 run".to_string()
        } else {
            format!("{} runs", self.total())
        }
    }
}

fn seconds_between(start: Timestamp, end: Timestamp) -> u64 {
    let start = start.as_nanosecond();
    let end = end.as_nanosecond();
    ((end - start).max(0) / 1_000_000_000) as u64
}

fn duration_label(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60)
    }
}

pub fn elapsed_seconds(run: &AgentRunSummary, now: Timestamp) -> u64 {
    let start = run.started_at.unwrap_or(run.queued_at);
    seconds_between(start, run.ended_at.unwrap_or(now))
}

pub fn elapsed_label(run: &AgentRunSummary, now: Timestamp) -> String {
    if run.status == AgentRunStatus::Queued {
        return format!(
            "Queued {}",
            duration_label(seconds_between(run.queued_at, run.ended_at.unwrap_or(now)))
        );
    }
    let execution = duration_label(elapsed_seconds(run, now));
    let Some(started_at) = run.started_at else {
        return execution;
    };
    let queued = seconds_between(run.queued_at, started_at);
    if queued == 0 {
        execution
    } else {
        format!("{execution} · queued {}", duration_label(queued))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A finished review of item `pr`, `Review #3` of `head` against
    /// `main`, with an attempt for each of `attempts`.
    pub(crate) fn review_summary(
        group_id: &str,
        attempts: &[&str],
    ) -> crate::review::model::ReviewGroupSummary {
        let created_at: Timestamp = "2026-09-19T10:00:00Z".parse().unwrap();
        let attempt = |id: &str| perch_github::ReviewerAttempt {
            id: id.into(),
            group_id: group_id.into(),
            run_id: None,
            backend: "claude-code".into(),
            executable: None,
            executable_version: None,
            model: "model".into(),
            effort: "high".into(),
            adapter_version: "1".into(),
            output_format_version: "1".into(),
            attempt_number: 1,
            retry_of: None,
            phase: perch_github::ReviewerPhase::Completed,
            ingestion_state: perch_github::ReviewIngestionState::Complete,
            result_kind: Some(perch_github::ReviewResultKind::Clean),
            output_id: None,
            raw_result: None,
            diagnostics: None,
            verification_input: perch_github::ReviewVerificationInput::default(),
            output_saved_at: Some(created_at),
            created_at,
            started_at: Some(created_at),
            ended_at: Some(created_at),
        };
        crate::review::model::ReviewGroupSummary {
            group: perch_github::ReviewGroup {
                id: group_id.into(),
                item_id: "pr".into(),
                snapshot_id: "snapshot".into(),
                number: 3,
                status: perch_github::ReviewGroupStatus::Completed,
                new_findings: 0,
                still_present: 0,
                verified_fixed: 0,
                not_verified: 0,
                created_at,
                started_at: Some(created_at),
                ended_at: Some(created_at),
                instructions: None,
            },
            snapshot: perch_github::ReviewSnapshot {
                id: "snapshot".into(),
                item_id: "pr".into(),
                repository: "slatedb/slatedb".into(),
                branch_label: Some("feature".into()),
                requested_base_ref: "main".into(),
                base_tip_sha: "base".into(),
                merge_base_sha: "merge".into(),
                head_sha: "head".into(),
                dirty_edits_excluded: false,
                retention_key: "retained".into(),
                retained_ref: None,
                manifest_path: Some("manifest".into()),
                diff_path: Some("diff".into()),
                created_at,
            },
            attempts: attempts.iter().map(|id| attempt(id)).collect(),
        }
    }

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn run(id: &str, item: &str, status: AgentRunStatus, at: &str) -> AgentRunSummary {
        AgentRunSummary {
            id: id.into(),
            item_id: Some(item.into()),
            thread_id: Some(format!("t-{id}")),
            review_attempt_id: None,
            repository: "perch".into(),
            worktree: item.into(),
            purpose: AgentRunPurpose::Question,
            request: id.into(),
            backend: if id.starts_with('c') {
                "codex".into()
            } else {
                "claude-code".into()
            },
            model: "model".into(),
            effort: "high".into(),
            mode: perch_github::RunMode::Ask,
            status,
            terminal_reason: None,
            queue_position: None,
            queued_at: self::at(at),
            started_at: Some(self::at(at)),
            ended_at: (!status.is_active()).then(|| self::at(at)),
            latest_activity: None,
            steps: Arc::new(Vec::new()),
            response: String::new(),
            response_truncated: false,
            error: None,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            retry_of: None,
            source_available: true,
            last_event_sequence: 0,
        }
    }

    #[test]
    fn active_groups_lift_the_current_item_and_put_queued_work_last() {
        let runs = vec![
            run(
                "a",
                "elsewhere",
                AgentRunStatus::Running,
                "2026-09-19T10:00:00Z",
            ),
            run(
                "b",
                "current",
                AgentRunStatus::Queued,
                "2026-09-19T10:01:00Z",
            ),
            run(
                "c",
                "current",
                AgentRunStatus::Running,
                "2026-09-19T10:02:00Z",
            ),
        ];
        let groups = active_groups(
            &runs,
            Some("current"),
            WorktreeFilter::All,
            BackendFilter::All,
        );
        assert_eq!(
            groups
                .iter()
                .map(|g| g.item_id.as_deref())
                .collect::<Vec<_>>(),
            [Some("current"), Some("elsewhere")]
        );
        assert_eq!(
            groups[0]
                .runs
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "b"]
        );
    }

    #[test]
    fn filters_do_not_change_the_global_active_count() {
        let runs = vec![
            run("a", "one", AgentRunStatus::Running, "2026-09-19T10:00:00Z"),
            run("c", "two", AgentRunStatus::Running, "2026-09-19T10:01:00Z"),
        ];
        let visible = active_groups(
            &runs,
            Some("one"),
            WorktreeFilter::Current,
            BackendFilter::Claude,
        );
        assert_eq!(visible.iter().map(|g| g.runs.len()).sum::<usize>(), 1);
        assert_eq!(runs.iter().filter(|r| r.status.is_active()).count(), 2);
    }

    #[test]
    fn history_is_newest_first_and_accepts_the_old_finished_spelling() {
        assert_eq!(AgentRunStatus::parse("finished"), AgentRunStatus::Completed);
        let runs = vec![
            run(
                "a",
                "one",
                AgentRunStatus::Completed,
                "2026-09-19T10:00:00Z",
            ),
            run("c", "two", AgentRunStatus::Failed, "2026-09-19T11:00:00Z"),
        ];
        let rows = history_runs(
            &runs,
            None,
            WorktreeFilter::All,
            BackendFilter::All,
            OutcomeFilter::All,
        );
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c", "a"]
        );

        let interrupted = run(
            "i",
            "two",
            AgentRunStatus::Interrupted,
            "2026-09-19T12:00:00Z",
        );
        assert!(OutcomeFilter::Interrupted.includes(interrupted.status));
        assert!(!OutcomeFilter::Failed.includes(interrupted.status));
    }

    /// An item's runs: its own only, working first with queued work
    /// last, then finished, newest first.
    #[test]
    fn an_items_runs_are_its_own_working_first_then_newest_finished() {
        let runs = vec![
            run("a", "one", AgentRunStatus::Completed, "2026-09-19T10:00:00Z"),
            run("b", "one", AgentRunStatus::Queued, "2026-09-19T10:01:00Z"),
            run("c", "two", AgentRunStatus::Running, "2026-09-19T10:02:00Z"),
            run("d", "one", AgentRunStatus::Running, "2026-09-19T10:03:00Z"),
            run("e", "one", AgentRunStatus::Failed, "2026-09-19T10:04:00Z"),
        ];
        let ids = item_runs(&runs, "one", &[])
            .into_iter()
            .map(|run| run.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["d", "b", "e", "a"]);
    }

    /// The pill says what is working, the busiest first, and after, how
    /// many there were or that the last one failed.
    #[test]
    fn the_pill_names_each_state_while_work_goes_and_the_total_after() {
        let none = HashSet::new();
        let at = "2026-09-19T10:00:00Z";
        let mut runs = vec![
            run("a", "one", AgentRunStatus::Running, at),
            run("b", "one", AgentRunStatus::Completed, "2026-09-19T09:00:00Z"),
            run("c", "one", AgentRunStatus::Completed, "2026-09-19T09:10:00Z"),
            run("d", "one", AgentRunStatus::Stopped, "2026-09-19T09:20:00Z"),
            run("e", "one", AgentRunStatus::Completed, "2026-09-19T09:30:00Z"),
        ];
        assert_eq!(RunCounts::of(&runs, &none).label(), "1 running · 4 done");

        let waiting = HashSet::from(["a".to_string()]);
        runs.push(run("q", "one", AgentRunStatus::Queued, at));
        runs.push(run("f", "one", AgentRunStatus::Failed, "2026-09-19T08:00:00Z"));
        assert_eq!(
            RunCounts::of(&runs, &waiting).label(),
            "1 queued · 1 waiting · 4 done · 1 failed"
        );

        runs.retain(|run| !run.status.is_active());
        assert_eq!(RunCounts::of(&runs, &none).label(), "5 runs");
        runs.truncate(1);
        assert_eq!(RunCounts::of(&runs, &none).label(), "1 run");

        // The one that finished last failed.
        runs.push(run("g", "one", AgentRunStatus::Interrupted, "2026-09-19T11:00:00Z"));
        let counts = RunCounts::of(&runs, &none);
        assert!(counts.last_failed);
        assert_eq!(counts.label(), "Last run failed");
    }

    #[test]
    fn elapsed_distinguishes_queue_time_from_execution_time() {
        let mut queued = run("a", "one", AgentRunStatus::Queued, "2026-09-19T10:00:00Z");
        queued.started_at = None;
        assert_eq!(
            elapsed_label(&queued, at("2026-09-19T10:01:05Z")),
            "Queued 1m 05s"
        );
        let running = run("a", "one", AgentRunStatus::Running, "2026-09-19T10:00:00Z");
        assert_eq!(elapsed_label(&running, at("2026-09-19T10:00:42Z")), "42s");

        let mut waited = running;
        waited.queued_at = at("2026-09-19T09:58:55Z");
        assert_eq!(
            elapsed_label(&waited, at("2026-09-19T10:00:42Z")),
            "42s · queued 1m 05s"
        );
    }

    #[test]
    fn typed_terminal_reason_precedes_but_does_not_discard_the_diagnostic() {
        let mut failed = run("a", "one", AgentRunStatus::Failed, "2026-09-19T10:00:00Z");
        failed.terminal_reason = Some(perch_github::TerminalReason::PermissionDenied);
        failed.error = Some("Read was rejected by policy".into());
        assert_eq!(
            failed.terminal_display().as_deref(),
            Some("The agent was denied permission — Read was rejected by policy")
        );
    }

    #[test]
    fn run_snapshots_never_regress_sequence_or_terminal_state() {
        let mut current = run("a", "one", AgentRunStatus::Running, "2026-09-19T10:00:00Z");
        current.last_event_sequence = 5;

        let mut older = current.clone();
        older.last_event_sequence = 4;
        assert!(!current.can_replace_with(&older));

        let mut finished = current.clone();
        finished.status = AgentRunStatus::Completed;
        assert!(current.can_replace_with(&finished));

        let mut late_active = finished.clone();
        late_active.status = AgentRunStatus::Running;
        late_active.last_event_sequence = 6;
        assert!(!finished.can_replace_with(&late_active));

        let mut richer = finished.clone();
        richer.response = "durable result".into();
        assert!(finished.can_replace_with(&richer));
    }

    #[test]
    fn native_review_owner_retains_the_attempt_identity() {
        let record = perch_github::RunRecord {
            id: "process-run".into(),
            owner: perch_github::RunOwner::Review {
                item_id: "item".into(),
                reviewer_run_id: "attempt-7".into(),
            },
            ..perch_github::RunRecord::default()
        };

        let projected = AgentRunSummary::from_record(&record);
        assert_eq!(projected.review_attempt_id.as_deref(), Some("attempt-7"));
        assert!(projected.thread_id.is_none());
        assert_eq!(
            projected.retry_target(),
            AgentRetryTarget::ReviewAttempt("attempt-7".into())
        );

        let ordinary = run(
            "ordinary-run",
            "item",
            AgentRunStatus::Failed,
            "2026-09-19T10:00:00Z",
        );
        assert_eq!(
            ordinary.retry_target(),
            AgentRetryTarget::Run("ordinary-run".into())
        );
    }

    #[test]
    fn reviewer_children_collapse_under_their_durable_group() {
        let created_at = at("2026-09-19T10:00:00Z");
        let mut claude = run(
            "claude-run",
            "item",
            AgentRunStatus::Failed,
            "2026-09-19T10:00:00Z",
        );
        claude.purpose = AgentRunPurpose::Review;
        claude.review_attempt_id = Some("attempt-claude".into());
        let mut codex = run(
            "codex-run",
            "item",
            AgentRunStatus::Completed,
            "2026-09-19T10:01:00Z",
        );
        codex.purpose = AgentRunPurpose::Review;
        codex.review_attempt_id = Some("attempt-codex".into());
        let ordinary = run(
            "question",
            "item",
            AgentRunStatus::Completed,
            "2026-09-19T10:02:00Z",
        );
        let attempt = |id: &str, backend: &str| perch_github::ReviewerAttempt {
            id: id.into(),
            group_id: "group-3".into(),
            run_id: None,
            backend: backend.into(),
            executable: None,
            executable_version: None,
            model: "model".into(),
            effort: "high".into(),
            adapter_version: "1".into(),
            output_format_version: "1".into(),
            attempt_number: 1,
            retry_of: None,
            phase: perch_github::ReviewerPhase::Completed,
            ingestion_state: perch_github::ReviewIngestionState::Complete,
            result_kind: Some(perch_github::ReviewResultKind::Clean),
            output_id: None,
            raw_result: None,
            diagnostics: None,
            verification_input: perch_github::ReviewVerificationInput::default(),
            output_saved_at: Some(created_at),
            created_at,
            started_at: Some(created_at),
            ended_at: Some(created_at),
        };
        let summary = crate::review::model::ReviewGroupSummary {
            group: perch_github::ReviewGroup {
                id: "group-3".into(),
                item_id: "item".into(),
                snapshot_id: "snapshot-3".into(),
                number: 3,
                status: perch_github::ReviewGroupStatus::Completed,
                new_findings: 0,
                still_present: 0,
                verified_fixed: 0,
                not_verified: 0,
                created_at,
                started_at: Some(created_at),
                ended_at: Some(created_at),
                instructions: None,
            },
            snapshot: perch_github::ReviewSnapshot {
                id: "snapshot-3".into(),
                item_id: "item".into(),
                repository: "local/perch".into(),
                branch_label: Some("feature".into()),
                requested_base_ref: "main".into(),
                base_tip_sha: "base".into(),
                merge_base_sha: "merge".into(),
                head_sha: "head".into(),
                dirty_edits_excluded: false,
                retention_key: "retained".into(),
                retained_ref: None,
                manifest_path: Some("manifest".into()),
                diff_path: Some("diff".into()),
                created_at,
            },
            attempts: vec![
                attempt("attempt-claude", "claude-code"),
                attempt("attempt-codex", "codex"),
            ],
        };

        let mut active_summary = summary.clone();
        active_summary.group.status = perch_github::ReviewGroupStatus::Reconciling;
        let active = active_groups_with_reviews(
            &[claude.clone(), codex.clone(), ordinary.clone()],
            None,
            WorktreeFilter::All,
            BackendFilter::All,
            &[active_summary.clone()],
        );
        assert_eq!(
            active.iter().map(|group| group.runs.len()).sum::<usize>(),
            2
        );
        let history = history_runs_with_reviews(
            &[claude.clone(), codex.clone(), ordinary.clone()],
            None,
            WorktreeFilter::All,
            BackendFilter::All,
            OutcomeFilter::All,
            &[active_summary],
        );
        assert_eq!(
            history
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            ["question"]
        );

        let entries = group_review_runs(&[claude, codex, ordinary], &[summary]);
        assert_eq!(entries.len(), 2);
        let AgentRunEntry::ReviewGroup(group) = &entries[0] else {
            panic!("review children were not grouped");
        };
        assert_eq!(group.key, "group-3");
        assert_eq!(
            group
                .runs
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            ["claude-run", "codex-run"]
        );
        assert!(matches!(entries[1], AgentRunEntry::Run(_)));
    }
}
