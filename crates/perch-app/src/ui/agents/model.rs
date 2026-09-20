//! The small, app-wide projection the Agents panel draws.
//!
//! A durable run remains the backend's record.  These summaries are what the
//! window needs in order to group, filter and inspect one without loading an
//! item's whole discussion.  Keeping the projection here also keeps Floem out
//! of the run manager and the database.

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
}

impl OutcomeFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Completed, Self::Failed, Self::Stopped];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All outcomes",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
        }
    }

    pub fn includes(self, status: AgentRunStatus) -> bool {
        match self {
            Self::All => true,
            Self::Completed => status == AgentRunStatus::Completed,
            Self::Failed => matches!(status, AgentRunStatus::Failed | AgentRunStatus::Interrupted),
            Self::Stopped => status == AgentRunStatus::Stopped,
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
    pub repository: String,
    pub worktree: String,
    pub purpose: AgentRunPurpose,
    pub request: String,
    pub backend: String,
    pub model: String,
    pub effort: String,
    pub status: AgentRunStatus,
    pub terminal_reason: Option<perch_github::TerminalReason>,
    pub queue_position: Option<usize>,
    pub queued_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub latest_activity: Option<String>,
    pub steps: Arc<Vec<AgentRunStep>>,
    pub response: String,
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

    pub fn from_record(run: &perch_github::RunRecord) -> Self {
        use perch_github::comments::RunPurpose;

        let usage = run.usage.as_ref();
        Self {
            id: run.id.clone(),
            item_id: run.owner.item_id().map(str::to_string),
            thread_id: run.thread_id().map(str::to_string),
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

/// Filter and group active rows. Groups keep the order in which their first
/// run was admitted, except that the open worktree is lifted to the front.
pub fn active_groups(
    runs: &[AgentRunSummary],
    current_item: Option<&str>,
    worktrees: WorktreeFilter,
    backends: BackendFilter,
) -> Vec<AgentRunGroup> {
    let mut groups: Vec<AgentRunGroup> = Vec::new();
    for run in runs.iter().filter(|run| {
        run.status.is_active()
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

pub fn history_runs(
    runs: &[AgentRunSummary],
    current_item: Option<&str>,
    worktrees: WorktreeFilter,
    backends: BackendFilter,
    outcomes: OutcomeFilter,
) -> Vec<AgentRunSummary> {
    let mut rows: Vec<_> = runs
        .iter()
        .filter(|run| {
            !run.status.is_active()
                && backends.includes(&run.backend)
                && outcomes.includes(run.status)
                && (worktrees == WorktreeFilter::All || run.item_id.as_deref() == current_item)
        })
        .cloned()
        .collect();
    rows.sort_by_key(|run| std::cmp::Reverse(run.ended_at.unwrap_or(run.queued_at)));
    rows
}

pub fn elapsed_seconds(run: &AgentRunSummary, now: Timestamp) -> u64 {
    let start = run.started_at.unwrap_or(run.queued_at).as_nanosecond();
    let end = run.ended_at.unwrap_or(now).as_nanosecond();
    ((end - start).max(0) / 1_000_000_000) as u64
}

pub fn elapsed_label(run: &AgentRunSummary, now: Timestamp) -> String {
    let seconds = elapsed_seconds(run, now);
    let time = if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60)
    };
    if run.status == AgentRunStatus::Queued {
        format!("Queued {time}")
    } else {
        time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn run(id: &str, item: &str, status: AgentRunStatus, at: &str) -> AgentRunSummary {
        AgentRunSummary {
            id: id.into(),
            item_id: Some(item.into()),
            thread_id: Some(format!("t-{id}")),
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
            status,
            terminal_reason: None,
            queue_position: None,
            queued_at: self::at(at),
            started_at: Some(self::at(at)),
            ended_at: (!status.is_active()).then(|| self::at(at)),
            latest_activity: None,
            steps: Arc::new(Vec::new()),
            response: String::new(),
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
}
