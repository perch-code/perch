//! Reactive app state.
//!
//! Every field is a signal, so `AppState` is `Copy` and can be handed to any
//! view closure without lifetime plumbing. Derived lists are plain functions
//! over these signals, recomputed by whichever effect reads them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use jiff::Timestamp;
use perch_github::{AgentConfig, DeclaredRepo, Item, Thread};

use crate::backend::{Event, Status};
use crate::ui::agents::model::{
    AgentRunSummary, AgentsTab, BackendFilter, OutcomeFilter, WorktreeFilter,
};

/// Whether a box is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Empty,
    Full,
}

/// A View action waits for its item to finish opening before focusing the
/// discussion.  The token prevents an older, slower open from stealing a
/// newer request's focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAgentNavigation {
    pub token: u64,
    pub navigation_id: u64,
    pub item_id: String,
    pub thread_id: Option<String>,
    pub review_attempt_id: Option<String>,
}

/// Which asynchronous item open is still allowed to replace the workspace.
/// `Any` is used only for operations such as Start work whose item id is not
/// known until the backend creates it. A known target rejects stale Ready
/// events, and `Closed` keeps a late result from reopening a dismissed view.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenExpectation {
    Any,
    Item(String),
    Closed,
}

/// A terminal transition worth surfacing outside the Agents panel. The
/// monotonically increasing token keeps an old dismissal timer from clearing
/// a newer completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCompletionNotice {
    pub run_id: String,
    pub token: u64,
}

#[derive(Clone, Copy)]
pub struct AppState {
    /// Encoded image bytes, shared so a clone is a refcount bump.
    pub avatars: RwSignal<HashMap<String, Arc<Vec<u8>>>>,
    /// Declared repository identities and display names, also offered by
    /// the sidebar's Repository filter when they have no cached items.
    pub repos: RwSignal<Vec<DeclaredRepo>>,
    /// Every item, active and archived.
    pub items: RwSignal<Vec<Item>>,
    /// Perch's visible notification list, in GitHub response order after local
    /// Done watermarks have been applied.
    pub notifications: RwSignal<Vec<Thread>>,
    /// GitHub's wire name for the notification reason currently shown.
    /// Separate from local item filters because notifications are not items.
    pub notification_reason: RwSignal<Option<String>>,
    /// The item whose row is selected in the sidebar: the open one.
    pub open_item: RwSignal<Option<String>>,
    /// The agent section of config.toml: binary, defaults, and the models
    /// the picker offers.
    pub agent: RwSignal<AgentConfig>,
    /// Something a worktree action could not do, for the status line.
    /// Cleared by the next act the reader takes.
    pub notice: RwSignal<Option<String>>,
    /// The review view's state, once the window has made it, so a menu
    /// that ends in something being opened can put the progress card up.
    pub review: RwSignal<Option<crate::review::model::ReviewState>>,
    /// The reader's own login and avatar on GitHub, once known, for the
    /// byline of a thread of their own.
    pub viewer: RwSignal<Option<(String, String)>>,
    /// Files the view should ask the backend to load again, after a scope
    /// change threw the loaded ones away. Drained by the window, which
    /// holds the backend handle.
    pub reload: RwSignal<Vec<String>>,

    /// App-wide run summaries. The durable records live in the cache; this
    /// page is enough to draw Active and the first page of History.
    pub agent_runs: RwSignal<Arc<Vec<AgentRunSummary>>>,
    /// Ordered persisted events for details the reader has opened. The first
    /// history page does not pull every transcript into memory.
    pub agent_run_events:
        RwSignal<HashMap<String, Arc<Vec<perch_github::comments::RunEventRecord>>>>,
    /// Per-detail pagination state. Event pages are retained only for runs
    /// the reader has opened, and fetched from newest toward oldest.
    pub agent_run_events_has_more: RwSignal<HashMap<String, bool>>,
    pub agent_run_events_loading: RwSignal<HashSet<String>>,
    /// The Agents inspector and its remembered geometry.
    pub agents_open: RwSignal<bool>,
    pub agents_width: RwSignal<f64>,
    pub agents_resizing: RwSignal<bool>,
    /// Guards the one shared elapsed-time ticker; rows never make timers of
    /// their own.
    pub agents_clock_armed: RwSignal<bool>,
    pub agents_tab: RwSignal<AgentsTab>,
    pub agents_worktree_filter: RwSignal<WorktreeFilter>,
    pub agents_backend_filter: RwSignal<BackendFilter>,
    pub agents_outcome_filter: RwSignal<OutcomeFilter>,
    pub selected_agent_run: RwSignal<Option<String>>,
    pub agents_control: RwSignal<Option<floem::ViewId>>,
    pub agents_opener: RwSignal<Option<floem::ViewId>>,
    pub agent_history_loading: RwSignal<bool>,
    pub agent_history_has_more: RwSignal<bool>,
    /// A load that received no answer within the UI's bounded wait. A later
    /// authoritative page clears this again.
    pub agent_history_load_failed: RwSignal<bool>,
    pub agent_history_load_token: RwSignal<u64>,
    /// Returning from detail rebuilds the list, so the run id—not the old
    /// row's transient ViewId—is the durable focus target.
    pub pending_agent_row_focus: RwSignal<Option<String>>,
    /// Review group rows are expanded by default; this records the explicit
    /// collapses so newly admitted reviewer children are immediately visible.
    pub collapsed_agent_review_groups: RwSignal<HashSet<String>>,
    pub pending_agent_navigation: RwSignal<Option<PendingAgentNavigation>>,
    pub agent_navigation_token: RwSignal<u64>,
    expected_navigation_id: RwSignal<Option<u64>>,
    open_expectation: RwSignal<OpenExpectation>,
    /// An archived discussion opened from run history is a read-only view.
    /// Keep it open across the normal Items refresh without restoring its
    /// checkout or changing its archived state.
    pub agent_archived_owner: RwSignal<Option<String>>,
    /// Completion notices are queued so simultaneous terminal transitions do
    /// not erase one another before the reader can see them.
    pub agent_completion_notices: RwSignal<VecDeque<AgentCompletionNotice>>,
    pub agent_completion_token: RwSignal<u64>,

    /// Review capability/preferences are global; bases, groups, and launch
    /// errors are keyed by their stable item id so opening another item never
    /// lets a late dashboard response replace the visible one.
    pub review_capabilities: RwSignal<Arc<Vec<perch_agent::ReviewCapability>>>,
    pub review_preferences: RwSignal<Arc<Vec<perch_github::ReviewPreference>>>,
    pub review_base_refs: RwSignal<HashMap<String, Arc<Vec<String>>>>,
    pub review_groups:
        RwSignal<HashMap<String, Arc<Vec<crate::review::model::ReviewGroupSummary>>>>,
    pub review_findings:
        RwSignal<HashMap<String, Arc<Vec<crate::review::model::ReviewFindingSummary>>>>,
    pub review_launch_errors: RwSignal<HashMap<String, String>>,
    pub review_dashboard_loading: RwSignal<HashSet<String>>,

    /// The item the pointer is over, so a row can show its buttons and a
    /// bare keystroke knows what it means.
    pub hovered_item: RwSignal<Option<String>>,
    /// The sidebar's filter menu settings and search.
    pub item_filters: RwSignal<crate::items::ItemFilters>,
    /// The item whose title is a text box right now.
    pub renaming: RwSignal<Option<String>>,
    /// The item with a Delete confirmation open under it.
    pub confirm_delete: RwSignal<Option<String>>,
    /// The sidebar's search box, so Cmd-1 can focus it.
    pub search_box: RwSignal<Option<floem::ViewId>>,
    /// Cmd-1 was pressed with the drawer shut. The box does not exist
    /// until the drawer has been rebuilt, so it takes the keyboard when
    /// it is made and clears this.
    pub focus_search: RwSignal<bool>,
    /// What is typed in the sidebar's search box.
    pub item_query: RwSignal<String>,
    /// A setting the sidebar wants remembered, picked up by the root view
    /// which has the backend handle.
    pub pending_meta: RwSignal<Option<(String, String)>>,

    pub status: RwSignal<Status>,
    /// Ticks forward on refresh, to re-render relative timestamps.
    pub now: RwSignal<Timestamp>,
    /// Whether the left drawer is showing.
    pub sidebar_open: RwSignal<bool>,
    /// How wide the drawer is. Dragged, so a signal rather than a constant.
    pub sidebar_width: RwSignal<f64>,
    /// Whether the drawer's edge is being dragged. The cursor follows what
    /// is under the pointer, and a drag runs over rows that each want their
    /// own, so the window keeps the resize cursor up itself for the duration.
    pub sidebar_resizing: RwSignal<bool>,
    /// True while the search box has the keyboard.
    ///
    /// Bare letters are shortcuts everywhere except inside a text field, and
    /// the root handler cannot see focus on its own.
    pub search_focused: RwSignal<bool>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            avatars: RwSignal::new(HashMap::new()),
            repos: RwSignal::new(Vec::new()),
            items: RwSignal::new(Vec::new()),
            notifications: RwSignal::new(Vec::new()),
            notification_reason: RwSignal::new(None),
            open_item: RwSignal::new(None),
            notice: RwSignal::new(None),
            review: RwSignal::new(None),
            viewer: RwSignal::new(None),
            reload: RwSignal::new(Vec::new()),
            agent_runs: RwSignal::new(Arc::new(Vec::new())),
            agent_run_events: RwSignal::new(HashMap::new()),
            agent_run_events_has_more: RwSignal::new(HashMap::new()),
            agent_run_events_loading: RwSignal::new(HashSet::new()),
            agents_open: RwSignal::new(false),
            agents_width: RwSignal::new(crate::theme::AGENTS_WIDTH),
            agents_resizing: RwSignal::new(false),
            agents_clock_armed: RwSignal::new(false),
            agents_tab: RwSignal::new(AgentsTab::Active),
            agents_worktree_filter: RwSignal::new(WorktreeFilter::All),
            agents_backend_filter: RwSignal::new(BackendFilter::All),
            agents_outcome_filter: RwSignal::new(OutcomeFilter::All),
            selected_agent_run: RwSignal::new(None),
            agents_control: RwSignal::new(None),
            agents_opener: RwSignal::new(None),
            agent_history_loading: RwSignal::new(false),
            agent_history_has_more: RwSignal::new(false),
            agent_history_load_failed: RwSignal::new(false),
            agent_history_load_token: RwSignal::new(0),
            pending_agent_row_focus: RwSignal::new(None),
            collapsed_agent_review_groups: RwSignal::new(HashSet::new()),
            pending_agent_navigation: RwSignal::new(None),
            agent_navigation_token: RwSignal::new(0),
            expected_navigation_id: RwSignal::new(None),
            open_expectation: RwSignal::new(OpenExpectation::Any),
            agent_archived_owner: RwSignal::new(None),
            agent_completion_notices: RwSignal::new(VecDeque::new()),
            agent_completion_token: RwSignal::new(0),
            review_capabilities: RwSignal::new(Arc::new(Vec::new())),
            review_preferences: RwSignal::new(Arc::new(Vec::new())),
            review_base_refs: RwSignal::new(HashMap::new()),
            review_groups: RwSignal::new(HashMap::new()),
            review_findings: RwSignal::new(HashMap::new()),
            review_launch_errors: RwSignal::new(HashMap::new()),
            review_dashboard_loading: RwSignal::new(HashSet::new()),
            agent: RwSignal::new(AgentConfig::default()),
            hovered_item: RwSignal::new(None),
            item_filters: RwSignal::new(crate::items::ItemFilters::default()),
            renaming: RwSignal::new(None),
            confirm_delete: RwSignal::new(None),
            search_box: RwSignal::new(None),
            focus_search: RwSignal::new(false),
            item_query: RwSignal::new(String::new()),
            pending_meta: RwSignal::new(None),
            status: RwSignal::new(Status::Refreshing),
            now: RwSignal::new(Timestamp::now()),
            sidebar_open: RwSignal::new(true),
            sidebar_width: RwSignal::new(crate::theme::SIDEBAR_WIDTH),
            sidebar_resizing: RwSignal::new(false),
            search_focused: RwSignal::new(false),
        }
    }

    /// Replace the summaries loaded at startup or by History pagination.
    /// Active records win by id; repeated pages are harmless.
    pub fn merge_agent_runs(
        &self,
        runs: impl IntoIterator<Item = perch_github::RunSummary>,
        has_more: bool,
    ) {
        self.agent_runs.update(|current| {
            let mut merged = current.as_ref().clone();
            for run in runs {
                let mut summary = AgentRunSummary::from_summary(&run);
                match merged.iter().position(|existing| existing.id == summary.id) {
                    Some(index) if merged[index].can_replace_with(&summary) => {
                        // List pages intentionally omit transcript fields. A
                        // refresh must not erase detail already hydrated for
                        // an open row, even when both projections carry the
                        // same event sequence.
                        summary.steps = merged[index].steps.clone();
                        summary.response = merged[index].response.clone();
                        summary.response_truncated = merged[index].response_truncated;
                        merged[index] = summary
                    }
                    Some(_) => {}
                    None => merged.push(summary),
                }
            }
            *current = Arc::new(merged);
        });
        self.agent_history_has_more.set(has_more);
        self.agent_history_loading.set(false);
        self.agent_history_load_failed.set(false);
    }

    pub fn begin_review_dashboard_load(&self, item_id: &str) {
        self.review_dashboard_loading
            .update(|loading| _ = loading.insert(item_id.to_string()));
        self.review_launch_errors
            .update(|errors| _ = errors.remove(item_id));
    }

    pub fn set_review_dashboard(
        &self,
        item_id: String,
        capabilities: Vec<perch_agent::ReviewCapability>,
        preferences: Vec<perch_github::ReviewPreference>,
        base_refs: Vec<String>,
        mut groups: Vec<crate::review::model::ReviewGroupSummary>,
        findings: Vec<crate::review::model::ReviewFindingSummary>,
    ) {
        groups.sort_by_key(|summary| std::cmp::Reverse(summary.group.number));
        self.review_capabilities.set(Arc::new(capabilities));
        self.review_preferences.set(Arc::new(preferences));
        self.review_base_refs
            .update(|all| _ = all.insert(item_id.clone(), Arc::new(base_refs)));
        self.review_groups
            .update(|all| _ = all.insert(item_id.clone(), Arc::new(groups)));
        self.review_findings
            .update(|all| _ = all.insert(item_id.clone(), Arc::new(findings)));
        self.review_dashboard_loading
            .update(|loading| _ = loading.remove(&item_id));
        self.review_launch_errors
            .update(|errors| _ = errors.remove(&item_id));
    }

    pub fn update_review_group(
        &self,
        item_id: String,
        summary: crate::review::model::ReviewGroupSummary,
    ) {
        self.review_groups.update(|all| {
            let mut groups = all
                .get(&item_id)
                .map(|groups| groups.as_ref().clone())
                .unwrap_or_default();
            match groups
                .iter()
                .position(|existing| existing.group.id == summary.group.id)
            {
                Some(index) => groups[index] = summary,
                None => groups.push(summary),
            }
            groups.sort_by_key(|summary| std::cmp::Reverse(summary.group.number));
            all.insert(item_id, Arc::new(groups));
        });
    }

    pub fn fail_review_launch(&self, item_id: String, message: String) {
        self.review_dashboard_loading
            .update(|loading| _ = loading.remove(&item_id));
        self.review_launch_errors
            .update(|errors| _ = errors.insert(item_id, message));
    }

    pub fn review_groups_for(
        &self,
        item_id: &str,
    ) -> Arc<Vec<crate::review::model::ReviewGroupSummary>> {
        self.review_groups
            .with(|all| all.get(item_id).cloned())
            .unwrap_or_else(|| Arc::new(Vec::new()))
    }

    pub fn review_findings_for(
        &self,
        item_id: &str,
    ) -> Arc<Vec<crate::review::model::ReviewFindingSummary>> {
        self.review_findings
            .with(|all| all.get(item_id).cloned())
            .unwrap_or_else(|| Arc::new(Vec::new()))
    }

    pub fn review_finding_for_thread(
        &self,
        item_id: &str,
        thread_id: &str,
    ) -> Option<crate::review::model::ReviewFindingSummary> {
        self.review_findings_for(item_id)
            .iter()
            .find(|summary| summary.finding.thread_id == thread_id)
            .cloned()
    }

    pub fn update_review_finding(
        &self,
        item_id: String,
        summary: crate::review::model::ReviewFindingSummary,
    ) {
        self.review_findings.update(|all| {
            let mut findings = all
                .get(&item_id)
                .map(|findings| findings.as_ref().clone())
                .unwrap_or_default();
            match findings
                .iter()
                .position(|existing| existing.finding.id == summary.finding.id)
            {
                Some(index) => findings[index] = summary,
                None => findings.push(summary),
            }
            findings.sort_by_key(|summary| std::cmp::Reverse(summary.finding.updated_at));
            all.insert(item_id, Arc::new(findings));
        });
    }

    pub fn review_base_refs_for(&self, item_id: &str) -> Arc<Vec<String>> {
        self.review_base_refs
            .with(|all| all.get(item_id).cloned())
            .unwrap_or_else(|| Arc::new(Vec::new()))
    }

    /// Start a bounded Agents-page request. The token fences an older timeout
    /// from turning a newer successful request into an error.
    pub fn begin_agent_history_load(&self) -> u64 {
        let token = self
            .agent_history_load_token
            .get_untracked()
            .wrapping_add(1);
        self.agent_history_load_token.set(token);
        self.agent_history_loading.set(true);
        self.agent_history_load_failed.set(false);
        token
    }

    /// Mark a still-current request as unanswered. Returns whether this call
    /// changed the visible state, which keeps timeout tests deterministic.
    pub fn time_out_agent_history_load(&self, token: u64) -> bool {
        if self.agent_history_load_token.get_untracked() != token
            || !self.agent_history_loading.get_untracked()
        {
            return false;
        }
        self.agent_history_loading.set(false);
        self.agent_history_load_failed.set(true);
        true
    }

    /// Apply one authoritative record after an event was persisted.
    pub fn update_agent_run(&self, run: &perch_github::RunRecord) {
        let summary = AgentRunSummary::from_record(run);
        if self
            .agent_run(&summary.id)
            .is_some_and(|current| !current.can_replace_with(&summary))
        {
            return;
        }
        let completed_now = self
            .agent_run(&summary.id)
            .is_some_and(|previous| previous.status.is_active() && !summary.status.is_active());
        let completed_id = summary.id.clone();
        self.agent_runs.update(|current| {
            let mut all = current.as_ref().clone();
            match all.iter().position(|existing| existing.id == summary.id) {
                Some(index) => all[index] = summary,
                None => all.push(summary),
            }
            *current = Arc::new(all);
        });
        if completed_now {
            self.show_agent_completion(completed_id);
        }
    }

    fn show_agent_completion(&self, run_id: String) {
        let token = self.agent_completion_token.get_untracked().wrapping_add(1);
        self.agent_completion_token.set(token);
        let was_empty = self
            .agent_completion_notices
            .with_untracked(VecDeque::is_empty);
        self.agent_completion_notices
            .update(|notices| notices.push_back(AgentCompletionNotice { run_id, token }));
        if was_empty {
            self.schedule_agent_completion_dismissal(token);
        }
    }

    pub fn current_agent_completion(&self) -> Option<AgentCompletionNotice> {
        self.agent_completion_notices
            .with(|notices| notices.front().cloned())
    }

    pub fn dismiss_agent_completion(&self, token: u64) {
        let mut next = None;
        self.agent_completion_notices.update(|notices| {
            let Some(index) = notices.iter().position(|notice| notice.token == token) else {
                return;
            };
            let was_front = index == 0;
            notices.remove(index);
            if was_front {
                next = notices.front().map(|notice| notice.token);
            }
        });
        if let Some(next) = next {
            self.schedule_agent_completion_dismissal(next);
        }
    }

    fn schedule_agent_completion_dismissal(&self, token: u64) {
        let state = *self;
        floem::action::exec_after(Duration::from_secs(8), move |_| {
            if state
                .current_agent_completion()
                .is_some_and(|notice| notice.token == token)
            {
                state.dismiss_agent_completion(token);
            }
        });
    }

    pub fn set_agent_run_detail(
        &self,
        run: &perch_github::RunRecord,
        mut events: Vec<perch_github::comments::RunEventRecord>,
        has_more: bool,
    ) {
        self.update_agent_run(run);
        self.agent_run_events.update(|all| {
            if let Some(live) = all.get(&run.id) {
                let loaded: HashSet<_> = events.iter().map(|event| event.sequence).collect();
                events.extend(
                    live.iter()
                        .filter(|event| !loaded.contains(&event.sequence))
                        .cloned(),
                );
            }
            events.sort_by_key(|event| event.sequence);
            all.insert(run.id.clone(), Arc::new(events));
        });
        self.agent_run_events_has_more
            .update(|pages| _ = pages.insert(run.id.clone(), has_more));
        self.agent_run_events_loading
            .update(|loading| _ = loading.remove(&run.id));
    }

    pub fn merge_agent_run_events(
        &self,
        run_id: &str,
        events: Vec<perch_github::comments::RunEventRecord>,
        has_more: bool,
    ) {
        self.agent_run_events.update(|all| {
            let mut merged = all
                .get(run_id)
                .map(|events| events.as_ref().clone())
                .unwrap_or_default();
            for event in events {
                match merged
                    .iter()
                    .position(|existing| existing.sequence == event.sequence)
                {
                    Some(index) => merged[index] = event,
                    None => merged.push(event),
                }
            }
            merged.sort_by_key(|event| event.sequence);
            all.insert(run_id.to_string(), Arc::new(merged));
        });
        self.agent_run_events_has_more
            .update(|pages| _ = pages.insert(run_id.to_string(), has_more));
        self.agent_run_events_loading
            .update(|loading| _ = loading.remove(run_id));
    }

    pub fn set_agent_run_events_loading(&self, run_id: &str) {
        self.agent_run_events_loading
            .update(|loading| _ = loading.insert(run_id.to_string()));
    }

    pub fn agent_run_events_page_state(&self, run_id: &str) -> (bool, bool) {
        let has_more = self
            .agent_run_events_has_more
            .with(|pages| pages.get(run_id).copied().unwrap_or(false));
        let loading = self
            .agent_run_events_loading
            .with(|runs| runs.contains(run_id));
        (has_more, loading)
    }

    /// Keep the open detail live without retaining every transcript in the
    /// app signal. The same event already committed before AgentStep was
    /// published; its authoritative sequence comes from AgentRunUpdated.
    fn append_open_agent_event(&self, run_id: &str, event: &perch_agent::RunEvent) {
        if self
            .selected_agent_run
            .with_untracked(|selected| selected.as_deref() != Some(run_id))
        {
            return;
        }
        let Some(sequence) = self.agent_run(run_id).map(|run| run.last_event_sequence) else {
            return;
        };
        let event = match event {
            perch_agent::RunEvent::Started { session_id, model } => {
                perch_github::RunEvent::Started {
                    session_id: session_id.clone(),
                    model: Some(model.clone()),
                }
            }
            perch_agent::RunEvent::Text(text) => {
                perch_github::RunEvent::Text { text: text.clone() }
            }
            perch_agent::RunEvent::ToolCall {
                id,
                name,
                summary,
                parent_id,
            } => perch_github::RunEvent::ToolCall {
                id: id.clone(),
                name: name.clone(),
                summary: summary.clone(),
                parent_id: parent_id.clone(),
            },
            perch_agent::RunEvent::ToolResult { id, ok, summary } => {
                perch_github::RunEvent::ToolResult {
                    id: id.clone(),
                    ok: *ok,
                    summary: summary.clone(),
                }
            }
            perch_agent::RunEvent::Usage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
            } => perch_github::RunEvent::Usage {
                usage: perch_github::RunUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    cached_input_tokens: *cached_input_tokens,
                },
                cost_usd: None,
                duration_ms: None,
            },
            perch_agent::RunEvent::PermissionDenied { tool, message } => {
                perch_github::RunEvent::PermissionDenied {
                    tool: tool.clone(),
                    message: message.clone(),
                }
            }
            perch_agent::RunEvent::Finished { .. }
            | perch_agent::RunEvent::Failed { .. }
            | perch_agent::RunEvent::Stopped { .. } => return,
        };
        let record = perch_github::RunEventRecord {
            run_id: run_id.to_string(),
            sequence,
            event_id: format!("live-{sequence}"),
            occurred_at: Timestamp::now(),
            event,
            truncated: false,
        };
        self.agent_run_events.update(|all| {
            let mut events = all
                .get(run_id)
                .map(|events| events.as_ref().clone())
                .unwrap_or_default();
            if let Some(index) = events.iter().position(|event| event.sequence == sequence) {
                events[index] = record;
            } else {
                events.push(record);
                events.sort_by_key(|event| event.sequence);
            }
            all.insert(run_id.to_string(), Arc::new(events));
        });
    }

    pub fn agent_run(&self, id: &str) -> Option<AgentRunSummary> {
        self.agent_runs
            .with(|runs| runs.iter().find(|run| run.id == id).cloned())
    }

    /// Remove summaries only when an authoritative owner snapshot proves
    /// their durable record was deleted. History pages remain additive.
    fn retain_agent_runs(&self, keep: impl Fn(&AgentRunSummary) -> bool) {
        let mut removed = HashSet::new();
        self.agent_runs.update(|current| {
            let kept: Vec<_> = current
                .iter()
                .filter_map(|run| {
                    if keep(run) {
                        Some(run.clone())
                    } else {
                        removed.insert(run.id.clone());
                        None
                    }
                })
                .collect();
            if !removed.is_empty() {
                *current = Arc::new(kept);
            }
        });
        if removed.is_empty() {
            return;
        }
        self.agent_run_events
            .update(|events| events.retain(|run_id, _| !removed.contains(run_id)));
        self.agent_run_events_has_more
            .update(|pages| pages.retain(|run_id, _| !removed.contains(run_id)));
        self.agent_run_events_loading
            .update(|runs| runs.retain(|run_id| !removed.contains(run_id)));
        if self
            .selected_agent_run
            .with_untracked(|selected| selected.as_ref().is_some_and(|id| removed.contains(id)))
        {
            self.selected_agent_run.set(None);
        }
        let front = self.current_agent_completion().map(|notice| notice.token);
        self.agent_completion_notices
            .update(|notices| notices.retain(|notice| !removed.contains(&notice.run_id)));
        if let Some(front) = front {
            let next = self.current_agent_completion();
            if next.as_ref().is_some_and(|notice| notice.token != front) {
                self.schedule_agent_completion_dismissal(next.unwrap().token);
            }
        }
    }

    pub fn active_agent_counts(&self) -> (usize, usize) {
        self.agent_runs.with(|runs| {
            let running = runs
                .iter()
                .filter(|run| run.status.is_active() && !run.status.is_queued())
                .count();
            let queued = runs.iter().filter(|run| run.status.is_queued()).count();
            (running, queued)
        })
    }

    pub fn begin_agent_navigation(
        &self,
        item_id: String,
        thread_id: Option<String>,
        navigation_id: u64,
    ) -> u64 {
        self.begin_agent_navigation_to(item_id, thread_id, None, navigation_id)
    }

    pub fn begin_agent_review_navigation(
        &self,
        review: crate::review::model::ReviewState,
        item_id: String,
        review_attempt_id: String,
        navigation_id: u64,
    ) -> u64 {
        review.begin_historical_review_rollback(self.expected_navigation_id.get_untracked());
        self.begin_agent_navigation_to(item_id, None, Some(review_attempt_id), navigation_id)
    }

    fn begin_agent_navigation_to(
        &self,
        item_id: String,
        thread_id: Option<String>,
        review_attempt_id: Option<String>,
        navigation_id: u64,
    ) -> u64 {
        let token = self.agent_navigation_token.get_untracked().wrapping_add(1);
        self.agent_navigation_token.set(token);
        self.expected_navigation_id.set(Some(navigation_id));
        self.open_expectation
            .set(OpenExpectation::Item(item_id.clone()));
        self.open_item.set(Some(item_id.clone()));
        self.agent_archived_owner.set(
            self.item(&item_id)
                .filter(perch_github::Item::is_archived)
                .map(|_| item_id.clone()),
        );
        self.pending_agent_navigation
            .set(Some(PendingAgentNavigation {
                token,
                navigation_id,
                item_id,
                thread_id,
                review_attempt_id,
            }));
        token
    }

    fn accepts_navigation(&self, navigation_id: u64) -> bool {
        self.expected_navigation_id.get_untracked() == Some(navigation_id)
    }

    fn accepts_open_item(&self, navigation_id: u64, item_id: &str) -> bool {
        self.accepts_navigation(navigation_id)
            && self
                .open_expectation
                .with_untracked(|expected| match expected {
                    OpenExpectation::Any => true,
                    OpenExpectation::Item(expected) => expected == item_id,
                    OpenExpectation::Closed => false,
                })
    }

    /// Fold one backend event into the sidebar or the review view.
    pub fn apply_with_review(&self, event: Event, review: crate::review::model::ReviewState) {
        use crate::review::model::{ItemTab, Phase, Preparing};

        match event {
            Event::ReviewProgress {
                navigation_id,
                message,
                percent,
            } => {
                if !self.accepts_navigation(navigation_id) {
                    return;
                }
                review
                    .preparing
                    .set(Preparing::Working { message, percent });
                review.set_phase(Phase::Preparing);
            }
            // The phase goes last, after every field the view will read.
            Event::ReviewReady {
                navigation_id,
                scope_gen,
                item,
                pull_request,
                files,
                tree,
                stale_head,
                offline,
            } => {
                if !self.accepts_open_item(navigation_id, &item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale item-open result");
                    return;
                }
                review.prepare_ready_for(&item.id);
                self.open_expectation
                    .set(OpenExpectation::Item(item.id.clone()));
                review.preparing.set(Preparing::Idle);
                review.scope_gen.set(Some(scope_gen));
                // A normal item open restores the live checkout after a
                // retained review snapshot was being inspected.
                review.clear_historical_review();
                review.set_changed_files(files);
                review.tree.set(tree);
                self.open_item.set(Some(item.id.clone()));
                let overview_loading = pull_request.is_some() && !offline;
                review.item.set(Some(item));
                review.stale_head.set(stale_head);
                review.offline.set(offline);
                review.pull_request.set(pull_request);
                review.pull_request_overview_loading.set(overview_loading);
                review.issue.set(None);
                review.issue_comments.set(Arc::new(Vec::new()));
                // The threads follow on their own event; whatever is here
                // is the last item's.
                review.comment_threads.set(Arc::new(Vec::new()));
                review.restore_tab();
                review.ready.set(true);
                review.set_phase(Phase::Ready);
            }
            Event::IssueReady {
                navigation_id,
                scope_gen,
                item,
                issue,
                comments,
                offline,
            } => {
                if !self.accepts_open_item(navigation_id, &item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale issue-open result");
                    return;
                }
                review.prepare_ready_for(&item.id);
                self.open_expectation
                    .set(OpenExpectation::Item(item.id.clone()));
                review.preparing.set(Preparing::Idle);
                review.scope_gen.set(Some(scope_gen));
                review.changed_files.set(Arc::new(Vec::new()));
                review.tree.set(Arc::new(Vec::new()));
                self.open_item.set(Some(item.id.clone()));
                review.clear_historical_review();
                review.item.set(Some(item));
                review.pull_request.set(None);
                review.stale_head.set(None);
                review.offline.set(offline);
                review.install_issue_reactions(&issue, &comments);
                review.issue.set(Some(issue));
                review.issue_comments.set(comments);
                review.comment_threads.set(Arc::new(Vec::new()));
                review.tab.set(ItemTab::Conversation);
                review.ready.set(true);
                review.set_phase(Phase::Issue);
            }
            Event::CommentThreads {
                item_id,
                threads,
                review_group_id,
                review_request_id,
            } => {
                let expected_historical = review.historical_review.with(|selected| {
                    selected.as_ref().and_then(|summary| {
                        review
                            .historical_review_request
                            .get_untracked()
                            .map(|request_id| (summary.group.id.clone(), request_id))
                    })
                });
                let matching_view = match (
                    expected_historical.as_ref(),
                    review_group_id.as_ref(),
                    review_request_id,
                ) {
                    (None, None, None) => true,
                    (
                        Some((expected_group, expected_request)),
                        Some(group_id),
                        Some(request_id),
                    ) => expected_group == group_id && *expected_request == request_id,
                    _ => false,
                };
                if !matching_view {
                    tracing::debug!(
                        item = %item_id,
                        group = ?review_group_id,
                        request = ?review_request_id,
                        "ignored comment threads for a stale review view"
                    );
                    return;
                }
                let retained_threads: HashSet<_> =
                    threads.iter().map(|thread| thread.id.as_str()).collect();
                self.retain_agent_runs(|run| {
                    run.item_id.as_deref() != Some(item_id.as_str())
                        || run
                            .thread_id
                            .as_deref()
                            .is_none_or(|thread_id| retained_threads.contains(thread_id))
                });
                let open = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
                if open.as_deref() == Some(item_id.as_str()) {
                    review.set_comment_threads(Arc::new(threads));
                    if let Some(pending) = self.pending_agent_navigation.get().filter(|pending| {
                        pending.token == self.agent_navigation_token.get_untracked()
                            && self.expected_navigation_id.get_untracked()
                                == Some(pending.navigation_id)
                            && pending.item_id == item_id
                            && pending.review_attempt_id.is_none()
                    }) {
                        review.show_tab(ItemTab::Conversation);
                        if let Some(thread_id) = pending.thread_id {
                            review
                                .focus_card
                                .set(Some(crate::review::doc::CardId::Thread(thread_id)));
                        }
                        self.pending_agent_navigation.set(None);
                    }
                }
            }
            Event::AgentStep {
                run_id,
                thread_id,
                event,
            } => {
                tracing::debug!(run_id, thread_id, ?event, "run event");
                self.append_open_agent_event(&run_id, &event);
                review.apply_run_event(&run_id, &event);
            }
            Event::AgentRuns { runs, has_more } => self.merge_agent_runs(runs, has_more),
            Event::AgentRunUpdated { run } => self.update_agent_run(&run),
            Event::AgentRunDetail {
                run,
                events,
                has_more,
            } => self.set_agent_run_detail(&run, events, has_more),
            Event::AgentRunEvents {
                run_id,
                events,
                has_more,
            } => self.merge_agent_run_events(&run_id, events, has_more),
            Event::AgentReviewNavigationReady {
                item_id,
                attempt_id,
                request_id,
                owner_changed,
                summary,
            } => {
                self.update_review_group(item_id.clone(), summary.clone());
                let selected = self.pending_agent_navigation.get().is_some_and(|pending| {
                    pending.token == self.agent_navigation_token.get_untracked()
                        && pending.navigation_id == request_id
                        && self.accepts_navigation(request_id)
                        && pending.item_id == item_id
                        && pending.review_attempt_id.as_deref() == Some(attempt_id.as_str())
                });
                let newest_request = review
                    .historical_review_request
                    .get_untracked()
                    .is_none_or(|current| request_id >= current);
                if selected && newest_request && review.is_item(&item_id) {
                    if owner_changed {
                        // ReviewReady installed this owner's live context.
                        // It, not the item visible before Agents navigation,
                        // is the stable view retained when the snapshot fails.
                        review.clear_historical_review();
                    }
                    review.begin_historical_review(
                        owner_changed.then_some(request_id),
                        summary,
                        request_id,
                    );
                    self.pending_agent_navigation.set(None);
                }
            }
            Event::AgentReviewNavigationFailed {
                item_id,
                attempt_id,
                request_id,
                message,
            } => {
                let selected = self.pending_agent_navigation.get().is_some_and(|pending| {
                    pending.token == self.agent_navigation_token.get_untracked()
                        && pending.navigation_id == request_id
                        && pending.item_id == item_id
                        && pending.review_attempt_id.as_deref() == Some(attempt_id.as_str())
                        && self.accepts_navigation(request_id)
                });
                if selected {
                    if let Some(rollback) = review.rollback_historical_review() {
                        if let Some(navigation_id) = rollback.navigation_id {
                            self.expected_navigation_id.set(Some(navigation_id));
                        }
                    }
                    if let Some(owner) = review
                        .item
                        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
                    {
                        self.open_expectation
                            .set(OpenExpectation::Item(owner.clone()));
                        self.open_item.set(Some(owner));
                    }
                    self.pending_agent_navigation.set(None);
                    review.preparing.set(Preparing::Idle);
                    if review.issue.with_untracked(Option::is_some) {
                        review.set_phase(Phase::Issue);
                    } else if review.ready.get_untracked() {
                        review.set_phase(Phase::Ready);
                    }
                    self.notice.set(Some(message));
                }
            }
            Event::NavigationCancelled {
                item_id,
                navigation_id,
            } => {
                if self.accepts_open_item(navigation_id, &item_id) {
                    self.leave_review(review);
                }
            }
            Event::ItemArchived {
                item_id,
                cancelled_navigation_id,
            } => {
                let matches_item = review.is_item(&item_id)
                    || self
                        .open_item
                        .with_untracked(|open| open.as_deref() == Some(item_id.as_str()))
                    || self
                        .open_expectation
                        .with_untracked(|expected| matches!(expected, OpenExpectation::Any));
                if matches_item
                    && cancelled_navigation_id
                        .is_some_and(|navigation_id| self.accepts_navigation(navigation_id))
                {
                    self.leave_review(review);
                }
            }
            Event::ReviewDashboard {
                item_id,
                capabilities,
                preferences,
                base_refs,
                groups,
                findings,
            } => {
                self.set_review_dashboard(
                    item_id.clone(),
                    capabilities,
                    preferences,
                    base_refs,
                    groups,
                    findings.clone(),
                );
                if review.is_item(&item_id) {
                    review.apply_review_finding_filter(
                        review.review_finding_filter.get_untracked(),
                        &findings,
                    );
                }
            }
            Event::ReviewGroupUpdated { item_id, summary } => {
                self.update_review_group(item_id, summary)
            }
            Event::ReviewFindingUpdated { item_id, summary } => {
                self.update_review_finding(item_id.clone(), summary);
                if review.is_item(&item_id) {
                    let findings = self.review_findings_for(&item_id);
                    review.apply_review_finding_filter(
                        review.review_finding_filter.get_untracked(),
                        &findings,
                    );
                }
            }
            Event::ReviewLaunchFailed { item_id, message } => {
                self.fail_review_launch(item_id, message)
            }
            Event::HistoricalReviewReady {
                item_id,
                group_id,
                request_id,
                scope_gen,
                files,
                tree,
                findings,
            } => {
                let selected = review.historical_review.with(|selected| {
                    selected
                        .as_ref()
                        .is_some_and(|summary| summary.group.id == group_id)
                        && review.historical_review_request.get_untracked() == Some(request_id)
                        && self.accepts_navigation(request_id)
                });
                if review.is_item(&item_id) && selected {
                    review.scope_gen.set(Some(scope_gen));
                    review.lsp_status.set(perch_lsp::Status::Absent);
                    // Historical membership is fixed by the selected
                    // group's observations, not by each finding's current
                    // disposition. Do not let today's Open filter hide a
                    // finding that was present in this review.
                    review.hidden_review_threads.set(HashSet::new());
                    review.finish_historical_review(findings);
                    // The backend has already switched its file context to
                    // this retained snapshot. Forget current-snapshot file
                    // buffers and let the normal reload queue ask for the
                    // historical versions of any open tabs.
                    for path in review.change_scope(perch_git::Scope::Branch, files) {
                        self.reload.update(|reload| reload.push(path));
                    }
                    review.tree.set(tree);
                    review.show_tab(ItemTab::Files);
                }
            }
            Event::HistoricalReviewFailed {
                item_id,
                group_id,
                request_id,
                message,
            } => {
                let selected = review.historical_review.with(|selected| {
                    selected.as_ref().is_some_and(|summary| {
                        summary.group.id == group_id
                            && (item_id.is_empty() || summary.group.item_id == item_id)
                            && review.historical_review_request.get_untracked() == Some(request_id)
                            && self.accepts_navigation(request_id)
                    })
                });
                if selected {
                    if let Some(rollback) = review.rollback_historical_review() {
                        if let Some(navigation_id) = rollback.navigation_id {
                            self.expected_navigation_id.set(Some(navigation_id));
                            let item_id = rollback
                                .review
                                .as_ref()
                                .map(|summary| summary.group.item_id.clone())
                                .or_else(|| (!item_id.is_empty()).then(|| item_id.clone()));
                            if let Some(item_id) = item_id {
                                self.open_expectation.set(OpenExpectation::Item(item_id));
                            }
                        }
                    }
                    self.notice.set(Some(message));
                }
            }
            Event::Items(items) => {
                let retained_items: HashSet<_> =
                    items.iter().map(|item| item.id.as_str()).collect();
                self.retain_agent_runs(|run| {
                    run.item_id
                        .as_deref()
                        .is_none_or(|item_id| retained_items.contains(item_id))
                });
                // The open review can be archived from the sidebar, or by
                // the sweep, or deleted. Its files are gone, so the view
                // goes with them. Otherwise the header follows the row, so
                // a branch that gained a pull request fills in.
                let open = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
                if let Some(open) = open {
                    match items.iter().find(|i| i.id == open) {
                        Some(i)
                            if i.is_archived()
                                && self
                                    .agent_archived_owner
                                    .with(|owner| owner.as_deref() != Some(open.as_str())) =>
                        {
                            self.leave_review(review)
                        }
                        Some(i) => {
                            review.item.set(Some(Arc::new(i.clone())));
                            if !i.is_archived() {
                                self.agent_archived_owner.set(None);
                            }
                            // The snapshot, when it changed: a notification
                            // refetches it, and the header reads this.
                            if let Some(pr) = i.pull_request() {
                                if review.pull_request.with(|p| p.as_deref() != Some(pr)) {
                                    review.pull_request.set(Some(Arc::new(pr.clone())));
                                }
                            }
                        }
                        None => self.leave_review(review),
                    }
                }
                self.apply(Event::Items(items));
            }
            Event::ReviewFailed {
                navigation_id,
                message,
            } => {
                if !self.accepts_navigation(navigation_id) {
                    return;
                }
                review.preparing.set(Preparing::Failed { message });
                review.set_phase(Phase::Failed);
            }
            Event::LspStatus {
                navigation_id,
                status,
            } => {
                if self.accepts_navigation(navigation_id) {
                    review.lsp_status.set(status);
                }
            }
            Event::LspHover { token, markdown } => {
                // Kept beside the symbol it answers, and only when it is the
                // answer to the ask still waiting: the pointer may have
                // moved to another symbol and asked about that since.
                review.hover.update(|hover| {
                    if let Some(h) = hover.as_mut().filter(|h| h.token == token) {
                        h.markdown = markdown;
                    }
                });
            }
            Event::LspDefinition { token, locations }
            | Event::LspTypeDefinition { token, locations }
            | Event::LspImplementation { token, locations } => {
                review.finish_goto(token, locations);
            }
            Event::LspDocumentHighlight { token, ranges } => {
                review.finish_highlights(token, ranges);
            }
            Event::LspCallHierarchy { token, items, place } => {
                review.finish_prepare_hierarchy(token, items, place);
            }
            Event::LspCalls { token, calls } => {
                review.finish_calls(token, calls);
            }
            // A load for the last item, still on its way when the next one
            // opened, is nobody's now.
            Event::FileLoaded {
                navigation_id,
                scope_gen,
                item_id,
                path,
                file,
            } => {
                if self.accepts_navigation(navigation_id)
                    && review.is_item(&item_id)
                    && review.scope_gen.get_untracked() == Some(scope_gen)
                {
                    review.finish_load(&path, file);
                }
            }
            Event::IssueComments {
                item_id,
                comments,
                body_reactions,
            } => {
                if review.is_item(&item_id) {
                    let body = review
                        .issue
                        .with_untracked(|i| i.as_ref().map(|i| i.node_id.clone()))
                        .unwrap_or_default();
                    review.install_reactions(
                        std::iter::once((body.as_str(), &body_reactions))
                            .chain(comments.iter().map(|c| (c.node_id.as_str(), &c.reactions))),
                    );
                    review.issue_comments.set(comments);
                }
            }
            Event::ReactionsUpdated {
                item_id,
                node_id,
                kind,
                reactions,
            } => {
                if review.is_item(&item_id) {
                    review.settle_reaction(&node_id, kind, reactions);
                }
            }
            // The complaint itself came as a notice.
            Event::ReactionFailed {
                item_id,
                node_id,
                kind,
                add,
            } => {
                if review.is_item(&item_id) {
                    review.revert_reaction(&node_id, kind, add);
                }
            }
            Event::FileFailed {
                navigation_id,
                scope_gen,
                item_id,
                path,
                message,
            } => {
                if self.accepts_navigation(navigation_id)
                    && review.is_item(&item_id)
                    && review.scope_gen.get_untracked() == Some(scope_gen)
                {
                    tracing::warn!(path, message, "could not open file");
                    review.fail_load(&path);
                }
            }
            // A complaint may be about the send a sent question's card is
            // waiting on, whose thread is then not coming; the card's
            // rows go with it. The notice itself is the status bar's.
            Event::Notice(message) => {
                review.drop_sent_placeholder();
                self.apply(Event::Notice(message));
            }
            // Everything about the open item that arrives after it is
            // ready. Each names its item, so a slow answer for the last
            // one opened cannot land on the next.
            Event::DiscussionReady {
                item_id,
                discussion,
            } => {
                if review.is_item(&item_id) {
                    review.install_reactions(discussion.reactions_by_id());
                    review
                        .discussion
                        .set(Some(review.with_submitted_reviews(discussion)));
                }
            }
            Event::ChecksReady {
                item_id,
                repo_full_name,
                number,
                head_sha,
                status_generation,
                checks,
            } => {
                let is_current_subject = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == head_sha
                    })
                });
                let is_current_generation = status_generation
                    >= review.pull_request_status_generation.get_untracked();
                if review.is_item(&item_id) && is_current_subject && is_current_generation {
                    review
                        .pull_request_status_generation
                        .set(status_generation);
                    review.checks.set(Some(checks));
                }
            }
            Event::PullRequestOverviewLoading {
                item_id,
                repo_full_name,
                number,
                head_sha,
                status_generation,
            } => {
                let is_current_subject = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == head_sha
                    })
                });
                let is_current_generation = status_generation
                    >= review.pull_request_status_generation.get_untracked();
                if review.is_item(&item_id) && is_current_subject && is_current_generation {
                    review
                        .pull_request_status_generation
                        .set(status_generation);
                    review.pull_request_overview.set(None);
                    review.pull_request_overview_error.set(None);
                    review.pull_request_overview_loading.set(true);
                }
            }
            Event::PullRequestOverviewReady {
                item_id,
                repo_full_name,
                number,
                status_generation,
                overview,
            } => {
                let is_current_subject = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == overview.head_sha
                    })
                });
                let is_current_generation = status_generation
                    >= review.pull_request_status_generation.get_untracked();
                if review.is_item(&item_id) && is_current_subject && is_current_generation {
                    review
                        .pull_request_status_generation
                        .set(status_generation);
                    review.pull_request_overview.set(Some(overview));
                    review.pull_request_overview_loading.set(false);
                    review.pull_request_overview_error.set(None);
                }
            }
            Event::PullRequestOverviewFailed {
                item_id,
                repo_full_name,
                number,
                head_sha,
                status_generation,
                message,
            } => {
                let is_current_subject = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == head_sha
                    })
                });
                let is_current_generation = status_generation
                    >= review.pull_request_status_generation.get_untracked();
                if review.is_item(&item_id) && is_current_subject && is_current_generation {
                    review
                        .pull_request_status_generation
                        .set(status_generation);
                    review.pull_request_overview.set(None);
                    review.pull_request_overview_loading.set(false);
                    review.pull_request_overview_error.set(Some(message));
                }
            }
            Event::PullRequestUpdated {
                item_id,
                item,
                pull_request,
                merge_attempt_id,
            } => {
                if review.is_item(&item_id) {
                    review.item.set(Some(item));
                    review.pull_request.set(Some(pull_request.clone()));
                    review.checks.set(None);
                    if let Some(attempt_id) = merge_attempt_id {
                        let is_current_attempt = review
                            .merge_attempt_id
                            .get_untracked()
                            .is_none_or(|current| attempt_id >= current);
                        if is_current_attempt {
                            review.merge_attempt_id.set(Some(attempt_id));
                            review.merge_pending.set(false);
                            review.merge_error.set(None);
                        }
                    }
                    // A REST snapshot can arrive because rules, permissions,
                    // or checks changed without the head moving. Never leave
                    // the old live authorization snapshot enabled while its
                    // replacement is being fetched.
                    review.pull_request_overview.set(None);
                    review.pull_request_overview_error.set(None);
                    review
                        .pull_request_overview_loading
                        .set(pull_request.is_open() && !review.offline.get_untracked());
                }
            }
            Event::PullRequestMergeStarted {
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                attempt_id,
            } => {
                let is_current_subject = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == expected_head_sha
                    })
                });
                let is_current_attempt = review
                    .merge_attempt_id
                    .get_untracked()
                    .is_none_or(|current| attempt_id >= current);
                if review.is_item(&item_id) && is_current_subject && is_current_attempt {
                    review.merge_attempt_id.set(Some(attempt_id));
                    review.merge_pending.set(true);
                    review.merge_error.set(None);
                }
            }
            Event::PullRequestMergeFailed {
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                attempt_id,
                message,
            } => {
                let current_subject_is_open = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == expected_head_sha
                            && pull.is_open()
                    })
                });
                let is_current_attempt = review
                    .merge_attempt_id
                    .get_untracked()
                    .is_none_or(|current| attempt_id >= current);
                if review.is_item(&item_id) && is_current_attempt {
                    review.merge_attempt_id.set(Some(attempt_id));
                    review.merge_pending.set(false);
                    if current_subject_is_open {
                        review.merge_error.set(Some(message));
                    }
                }
            }
            Event::PullRequestReviewStarted {
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                attempt_id,
            } => {
                let is_current_subject = review.pull_request.with(|pull| {
                    pull.as_ref().is_some_and(|pull| {
                        pull.repo_full_name == repo_full_name
                            && pull.number == number
                            && pull.head_sha == expected_head_sha
                    })
                });
                // The UI registers the attempt synchronously before the
                // backend can acknowledge it. A Started event is therefore
                // only an acknowledgement: after navigation has reset the
                // UI, a delayed event must not claim a newly opened draft.
                let is_current_attempt =
                    review.submit_review_pending.get_untracked() == Some(attempt_id);
                if review.is_item(&item_id) && is_current_subject && is_current_attempt {
                    review.begin_submit_review(attempt_id);
                }
            }
            Event::PullRequestReviewSubmitted {
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                attempt_id,
                review: submitted,
            } => {
                // The review is already durable on GitHub. Accept it for the
                // same pull request even if a follow-up refresh has meanwhile
                // observed a newer head; the review itself remains part of
                // that pull request's timeline and names its exact commit.
                let (is_current_subject, is_current_head) = review.pull_request.with(|pull| {
                    pull.as_ref()
                        .map(|pull| {
                            (
                                pull.repo_full_name == repo_full_name && pull.number == number,
                                pull.head_sha == expected_head_sha,
                            )
                        })
                        .unwrap_or_default()
                });
                if review.is_item(&item_id) && is_current_subject {
                    if !is_current_head {
                        tracing::info!(
                            item = %item_id,
                            reviewed_head = %expected_head_sha,
                            "installed a submitted review after the pull request head moved"
                        );
                    }
                    review.finish_submit_review(attempt_id, (*submitted).clone());
                }
            }
            Event::PullRequestReviewFailed {
                item_id,
                repo_full_name,
                number,
                expected_head_sha,
                attempt_id,
                message,
            } => {
                let (is_current_subject, is_current_head) = review.pull_request.with(|pull| {
                    pull.as_ref()
                        .map(|pull| {
                            (
                                pull.repo_full_name == repo_full_name && pull.number == number,
                                pull.head_sha == expected_head_sha,
                            )
                        })
                        .unwrap_or_default()
                });
                if review.is_item(&item_id) && is_current_subject {
                    let message = if is_current_head {
                        message
                    } else {
                        "The pull request moved; refresh and review the new head before submitting"
                            .into()
                    };
                    review.fail_submit_review(attempt_id, message);
                }
            }
            Event::CommitsReady {
                navigation_id,
                scope_gen,
                item_id,
                commits,
                dirt,
                since,
            } => {
                if self.accepts_navigation(navigation_id)
                    && review.is_item(&item_id)
                    && review.scope_gen.get_untracked() == Some(scope_gen)
                {
                    review.commits.set(commits);
                    review.dirty.set(dirt);
                    review.since_head.set(since);
                }
            }
            Event::ScopeReady {
                navigation_id,
                scope_gen,
                item_id,
                scope,
                files,
            } => {
                if self.accepts_navigation(navigation_id) && review.is_item(&item_id) {
                    review.scope_gen.set(Some(scope_gen));
                    for path in review.change_scope(scope, files) {
                        self.reload.update(|r| r.push(path));
                    }
                }
            }
            Event::ViewedReady {
                item_id,
                viewed,
                reveals,
            } => {
                if review.is_item(&item_id) {
                    review.viewed.set(viewed);
                    // The open's answer carries the store's reveals, which
                    // replace what is held for the files it has rows for. A
                    // file with no blob has none there and keeps its own,
                    // under the empty blob. A write's answer carries none,
                    // and what is held stays: it was applied as it was
                    // clicked. The writes are skipped when nothing changed:
                    // each would rebuild the open file's document for
                    // nothing.
                    if let Some(reveals) = reveals {
                        let loaded: HashMap<(String, String), crate::review::doc::Reveals> =
                            reveals
                                .iter()
                                .map(|(key, ranges)| {
                                    (
                                        key.clone(),
                                        ranges
                                            .iter()
                                            .map(|&(a, b)| (a as usize, b as usize))
                                            .collect(),
                                    )
                                })
                                .collect();
                        let changed = review.reveals.with_untracked(|held| {
                            held.iter()
                                .filter(|((_, blob), _)| !blob.is_empty())
                                .count()
                                != loaded.len()
                                || loaded.iter().any(|(key, set)| held.get(key) != Some(set))
                        });
                        if changed {
                            review.reveals.update(|held| {
                                held.retain(|(_, blob), _| blob.is_empty());
                                held.extend(loaded);
                            });
                        }
                    }
                }
            }
            Event::LspReferences { token, locations } => {
                // Matched against the panel's own token: the shared counter
                // moves on with every hover while the answer is on its way.
                review.references.update(|r| {
                    if let Some(r) = r.as_mut().filter(|r| r.token == token) {
                        let mut open = (**r).clone();
                        open.locations = locations;
                        open.pending = false;
                        *r = Arc::new(open);
                    }
                });
            }
            other => self.apply(other),
        }
    }

    /// Fold one event into what the sidebar and the status bar read.
    pub fn apply(&self, event: Event) {
        match event {
            Event::Avatar { url, bytes } => {
                self.avatars.update(|m| {
                    m.insert(url, Arc::new(bytes));
                });
            }
            Event::Items(items) => self.items.set(items),
            Event::Notifications(notifications) => self.notifications.set(notifications),
            Event::Repos(repos) => self.repos.set(repos),
            Event::Meta { key, value } => match key.as_str() {
                "item_filters" => match crate::items::ItemFilters::parse(&value) {
                    Ok(f) => self.item_filters.set(f),
                    Err(e) => tracing::warn!(error = %e, "ignoring saved item filters"),
                },
                "review.layout" => {
                    if let (Some(layout), Some(review)) = (
                        crate::review::model::Layout::parse(&value),
                        self.review.get_untracked(),
                    ) {
                        review.layout.set(layout);
                    }
                }
                "agents.open" => self.agents_open.set(value == "true"),
                "agents.width" => match value.parse::<f64>() {
                    Ok(width) => self.agents_width.set(width.clamp(
                        crate::theme::AGENTS_MIN_WIDTH,
                        crate::theme::AGENTS_MAX_WIDTH,
                    )),
                    Err(e) => tracing::warn!(error = %e, "ignoring saved Agents width"),
                },
                // The two arrive one after the other; whichever comes second
                // completes the pair.
                "viewer.login" => self.viewer.update(|v| {
                    let avatar = v.as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
                    *v = Some((value, avatar));
                }),
                "viewer.avatar_url" => self.viewer.update(|v| {
                    let login = v.as_ref().map(|(l, _)| l.clone()).unwrap_or_default();
                    *v = Some((login, value));
                }),
                _ => {}
            },
            // An empty notice is the backend saying the last one has been
            // read: the reader has moved on to something else.
            Event::Notice(message) => self.notice.set(Some(message).filter(|m| !m.is_empty())),
            Event::AgentConfig(agent) => self.agent.set(agent),
            Event::CommentThreads { .. }
            | Event::AgentStep { .. }
            | Event::AgentRuns { .. }
            | Event::AgentRunUpdated { .. }
            | Event::AgentRunDetail { .. }
            | Event::AgentRunEvents { .. }
            | Event::AgentReviewNavigationReady { .. }
            | Event::AgentReviewNavigationFailed { .. }
            | Event::NavigationCancelled { .. }
            | Event::ItemArchived { .. }
            | Event::ReviewDashboard { .. }
            | Event::ReviewGroupUpdated { .. }
            | Event::ReviewFindingUpdated { .. }
            | Event::ReviewLaunchFailed { .. }
            | Event::HistoricalReviewReady { .. }
            | Event::HistoricalReviewFailed { .. }
            | Event::IssueReady { .. } => {}
            Event::ReviewProgress { .. }
            | Event::ReviewReady { .. }
            | Event::ReviewFailed { .. }
            | Event::FileLoaded { .. }
            | Event::FileFailed { .. }
            | Event::LspStatus { .. }
            | Event::LspHover { .. }
            | Event::LspDefinition { .. }
            | Event::LspTypeDefinition { .. }
            | Event::LspImplementation { .. }
            | Event::LspDocumentHighlight { .. }
            | Event::LspCallHierarchy { .. }
            | Event::LspCalls { .. }
            | Event::DiscussionReady { .. }
            | Event::IssueComments { .. }
            | Event::ReactionsUpdated { .. }
            | Event::ReactionFailed { .. }
            | Event::ChecksReady { .. }
            | Event::PullRequestOverviewLoading { .. }
            | Event::PullRequestOverviewReady { .. }
            | Event::PullRequestOverviewFailed { .. }
            | Event::PullRequestUpdated { .. }
            | Event::PullRequestMergeStarted { .. }
            | Event::PullRequestMergeFailed { .. }
            | Event::PullRequestReviewStarted { .. }
            | Event::PullRequestReviewSubmitted { .. }
            | Event::PullRequestReviewFailed { .. }
            | Event::CommitsReady { .. }
            | Event::ScopeReady { .. }
            | Event::ViewedReady { .. }
            | Event::LspReferences { .. } => {}
            Event::Status(status) => {
                if let Status::Idle { at, .. } = &status {
                    self.now.set(*at);
                }
                self.status.set(status);
            }
        }
    }

    // -----------------------------------------------------------------
    // The review view
    // -----------------------------------------------------------------

    /// Put the progress card up with `message` until the backend says what
    /// it opened.
    pub fn begin_review(&self, review: crate::review::model::ReviewState, message: &str) {
        review.begin(message);
    }

    /// Open a known item. Its id is installed before the asynchronous work so
    /// a slower result from an earlier click cannot replace it.
    pub fn begin_item_review(
        &self,
        review: crate::review::model::ReviewState,
        message: &str,
        item_id: &str,
        navigation_id: u64,
    ) {
        self.expected_navigation_id.set(Some(navigation_id));
        self.open_expectation
            .set(OpenExpectation::Item(item_id.to_string()));
        self.open_item.set(Some(item_id.to_string()));
        review.begin(message);
    }

    /// The progress card, over the open review or the empty main area, for
    /// an act that ends in another item being opened: Start work, Fork.
    pub fn begin_open(&self, message: &str, navigation_id: u64) {
        self.expected_navigation_id.set(Some(navigation_id));
        self.pending_agent_navigation.set(None);
        self.agent_archived_owner.set(None);
        self.open_expectation.set(OpenExpectation::Any);
        self.open_item.set(None);
        if let Some(review) = self.review.get_untracked() {
            review.begin(message);
        }
    }

    /// Register a known item without changing its visible loading state.
    pub fn expect_item_navigation(&self, item_id: &str, navigation_id: u64) {
        self.expected_navigation_id.set(Some(navigation_id));
        self.open_expectation
            .set(OpenExpectation::Item(item_id.to_string()));
    }

    /// Select a retained review after the backend has synchronously assigned
    /// its request identity.
    pub fn begin_historical_review(
        &self,
        review: crate::review::model::ReviewState,
        summary: crate::review::model::ReviewGroupSummary,
        navigation_id: u64,
    ) {
        let item_id = summary.group.item_id.clone();
        review.begin_historical_review(
            self.expected_navigation_id.get_untracked(),
            summary,
            navigation_id,
        );
        self.expect_item_navigation(&item_id, navigation_id);
    }

    /// A user-initiated close has its own backend navigation identity.
    pub fn begin_close(&self, review: crate::review::model::ReviewState, navigation_id: u64) {
        self.expected_navigation_id.set(Some(navigation_id));
        self.leave_review(review);
    }

    /// Leave the review view. The sidebar is the only place to land.
    pub fn leave_review(&self, review: crate::review::model::ReviewState) {
        self.pending_agent_navigation.set(None);
        self.agent_archived_owner.set(None);
        self.open_expectation.set(OpenExpectation::Closed);
        self.open_item.set(None);
        review.close();
    }

    pub fn item(&self, id: &str) -> Option<Item> {
        self.items
            .with(|all| all.iter().find(|i| i.id == id).cloned())
    }

    /// The flat sidebar under the current scope, filters and search.
    pub fn visible_items(&self) -> Vec<Item> {
        let mut filters = self.item_filters.get();
        filters.query = self.item_query.get();
        self.viewer.with(|viewer| {
            self.items.with(|items| {
                crate::items::visible_items(
                    items,
                    &filters,
                    viewer.as_ref().map(|(login, _)| login.as_str()),
                )
            })
        })
    }

    pub fn avatar_for(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        if url.is_empty() {
            return None;
        }
        self.avatars.with(|m| m.get(url).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::ReviewState;
    use perch_github::{
        CheckState, Checks, MergeMethods, MergeState, MergeableState, PullRequest,
        PullRequestOverview, PullReviewEvent, Reason, Repository, RepositoryPermission, Review,
        Subject, SubjectKind,
    };

    /// A review with an item open, by id.
    fn review_of(id: &str) -> ReviewState {
        let review = ReviewState::new();
        let mut item = Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = id.into();
        review.item.set(Some(Arc::new(item)));
        review
    }

    fn pull_request(number: u64, head_sha: &str, state: &str) -> PullRequest {
        PullRequest {
            number,
            title: format!("pull {number}"),
            html_url: format!("https://github.com/slatedb/slatedb/pull/{number}"),
            author_login: "u".into(),
            author_avatar_url: String::new(),
            head_sha: head_sha.into(),
            head_ref: "feature".into(),
            base_sha: "base".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: state.into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
            head_repo_full_name: Some("slatedb/slatedb".into()),
            updated_at: None,
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

    fn pull_request_overview(head_sha: &str) -> Arc<PullRequestOverview> {
        Arc::new(PullRequestOverview {
            head_sha: head_sha.into(),
            mergeable: MergeableState::Mergeable,
            merge_state: MergeState::Clean,
            review_decision: None,
            viewer_can_merge_as_admin: false,
            viewer_permission: Some(RepositoryPermission::Write),
            allowed_merge_methods: MergeMethods {
                merge: true,
                squash: true,
                rebase: true,
            },
            checks: Checks {
                state: CheckState::Passing,
                url: None,
                runs: Vec::new(),
            },
        })
    }

    fn review_of_pull(id: &str, number: u64, head_sha: &str) -> ReviewState {
        let review = review_of(id);
        review
            .pull_request
            .set(Some(Arc::new(pull_request(number, head_sha, "open"))));
        review
    }

    /// What the backend sends for an item on the open: `src/x.rs` at `b1`
    /// viewed, with two runs revealed in it.
    fn viewed_ready(item_id: &str, reveals: Vec<(u32, u32)>) -> Event {
        let key = ("src/x.rs".to_string(), "b1".to_string());
        Event::ViewedReady {
            item_id: item_id.into(),
            viewed: Arc::new([key.clone()].into_iter().collect()),
            reveals: Some(Arc::new(if reveals.is_empty() {
                HashMap::new()
            } else {
                [(key, reveals)].into_iter().collect()
            })),
        }
    }

    fn notification(id: &str, unread: bool) -> Thread {
        Thread {
            id: id.into(),
            unread,
            local_unread: false,
            reason: Reason::Mention,
            updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            last_read_at: (!unread).then(|| "2026-09-19T11:00:00Z".parse().unwrap()),
            subject: Subject {
                title: format!("Notification {id}"),
                kind: SubjectKind::Issue,
                url: Some(format!("https://api.github.com/repos/org/repo/issues/{id}")),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "org/repo".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
        }
    }

    fn review_ready(item_id: &str, navigation_id: u64) -> Event {
        review_ready_at_scope(item_id, navigation_id, navigation_id)
    }

    fn review_ready_at_scope(item_id: &str, navigation_id: u64, scope_gen: u64) -> Event {
        let mut item = Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = item_id.into();
        Event::ReviewReady {
            navigation_id,
            scope_gen,
            item: Arc::new(item),
            pull_request: None,
            files: Arc::new(Vec::new()),
            tree: Arc::new(Vec::new()),
            stale_head: None,
            offline: true,
        }
    }

    fn historical_summary(group_id: &str) -> crate::review::model::ReviewGroupSummary {
        let created_at = "2026-09-19T12:00:00Z".parse().unwrap();
        crate::review::model::ReviewGroupSummary {
            group: perch_github::ReviewGroup {
                id: group_id.into(),
                item_id: "wt".into(),
                snapshot_id: "snapshot".into(),
                number: 2,
                status: perch_github::ReviewGroupStatus::Completed,
                new_findings: 0,
                still_present: 0,
                verified_fixed: 0,
                not_verified: 0,
                created_at,
                started_at: Some(created_at),
                ended_at: Some(created_at),
            },
            snapshot: perch_github::ReviewSnapshot {
                id: "snapshot".into(),
                item_id: "wt".into(),
                repository: "slatedb/slatedb".into(),
                branch_label: Some("feature".into()),
                requested_base_ref: "main".into(),
                base_tip_sha: "base".into(),
                merge_base_sha: "merge-base".into(),
                head_sha: "head".into(),
                dirty_edits_excluded: false,
                retention_key: "snapshot".into(),
                retained_ref: None,
                manifest_path: None,
                diff_path: None,
                created_at,
            },
            attempts: Vec::new(),
        }
    }

    #[test]
    fn a_notifications_event_replaces_the_inbox_in_github_order_and_keeps_read_threads() {
        let app = AppState::new();
        app.apply(Event::Notifications(vec![
            notification("2", false),
            notification("1", true),
        ]));
        let notifications = app.notifications.get();
        assert_eq!(
            notifications
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "1"]
        );
        assert!(!notifications[0].unread);
        assert!(notifications[1].unread);
    }

    #[test]
    fn a_late_agent_view_cannot_replace_or_focus_over_the_latest_target() {
        let app = AppState::new();
        let review = ReviewState::new();

        let first = app.begin_agent_navigation("older".into(), Some("thread-old".into()), 11);
        let latest = app.begin_agent_navigation("latest".into(), Some("thread-new".into()), 12);
        assert!(latest > first);
        app.begin_review(review, "Reading the discussion");

        app.apply_with_review(review_ready("older", 11), review);
        assert_eq!(app.open_item.get().as_deref(), Some("latest"));
        assert!(!review.is_item("older"));

        app.apply_with_review(review_ready("latest", 12), review);
        app.apply_with_review(
            Event::CommentThreads {
                item_id: "latest".into(),
                threads: Vec::new(),
                review_group_id: None,
                review_request_id: None,
            },
            review,
        );
        assert!(review.is_item("latest"));
        assert_eq!(
            review.focus_card.get(),
            Some(crate::review::doc::CardId::Thread("thread-new".into()))
        );

        // The first load can still finish after the latest one. It remains
        // inert, and closing the workspace also fences delayed Ready events.
        app.apply_with_review(review_ready("older", 11), review);
        assert!(review.is_item("latest"));
        app.leave_review(review);
        app.apply_with_review(review_ready("latest", 12), review);
        assert!(!review.is_showing());
    }

    #[test]
    fn a_queued_same_item_event_requires_the_exact_backend_navigation_id() {
        let app = AppState::new();
        let review = ReviewState::new();

        app.begin_item_review(review, "First open", "same", 31);
        app.begin_item_review(review, "Second open", "same", 32);

        app.apply_with_review(review_ready("same", 31), review);
        app.apply_with_review(
            Event::ReviewFailed {
                navigation_id: 31,
                message: "stale failure".into(),
            },
            review,
        );
        assert!(!review.ready.get());
        assert_eq!(review.phase.get(), crate::review::model::Phase::Preparing);

        app.apply_with_review(review_ready("same", 32), review);
        assert!(review.ready.get());
        assert!(review.is_item("same"));
    }

    #[test]
    fn an_accepted_cross_item_ready_clears_the_retained_checkout() {
        let app = AppState::new();
        let review = review_of("old");
        review.tabs.set(vec![crate::review::model::Tab {
            path: "src/old.rs".into(),
            origin: crate::review::model::Origin::Worktree,
        }]);
        review
            .loading
            .update(|loading| _ = loading.insert("src/old.rs".into(), ()));
        review.lsp_status.set(perch_lsp::Status::Ready);
        app.expect_item_navigation("new", 41);

        app.apply_with_review(review_ready("new", 41), review);

        assert!(review.is_item("new"));
        assert!(review.tabs.with(Vec::is_empty));
        assert!(review.loading.with(HashMap::is_empty));
        assert_eq!(review.lsp_status.get(), perch_lsp::Status::Absent);
    }

    #[test]
    fn an_accepted_same_item_ready_fences_the_old_lsp_status() {
        let app = AppState::new();
        let review = review_of("same");
        review.lsp_status.set(perch_lsp::Status::Ready);
        app.expect_item_navigation("same", 42);

        app.apply_with_review(review_ready("same", 42), review);

        assert_eq!(review.lsp_status.get(), perch_lsp::Status::Absent);
    }

    #[test]
    fn file_results_require_the_current_scope_generation() {
        let app = AppState::new();
        let review = ReviewState::new();

        app.begin_item_review(review, "Open", "same", 61);
        app.apply_with_review(review_ready_at_scope("same", 61, 610), review);
        review.loading.update(|loading| {
            loading.insert("src/loaded.rs".into(), ());
            loading.insert("src/failed.rs".into(), ());
        });
        let file = Arc::new(crate::backend::build_loaded_file(
            "src/loaded.rs".into(),
            "new\n".into(),
            "old\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        ));

        app.apply_with_review(
            Event::FileLoaded {
                navigation_id: 61,
                scope_gen: 609,
                item_id: "same".into(),
                path: "src/loaded.rs".into(),
                file: file.clone(),
            },
            review,
        );
        app.apply_with_review(
            Event::FileFailed {
                navigation_id: 61,
                scope_gen: 609,
                item_id: "same".into(),
                path: "src/failed.rs".into(),
                message: "stale".into(),
            },
            review,
        );
        assert!(!review.files.with(|files| files.contains_key("src/loaded.rs")));
        assert!(review
            .loading
            .with(|loading| loading.contains_key("src/loaded.rs")));
        assert!(review
            .loading
            .with(|loading| loading.contains_key("src/failed.rs")));

        app.apply_with_review(
            Event::FileLoaded {
                navigation_id: 61,
                scope_gen: 610,
                item_id: "same".into(),
                path: "src/loaded.rs".into(),
                file,
            },
            review,
        );
        app.apply_with_review(
            Event::FileFailed {
                navigation_id: 61,
                scope_gen: 610,
                item_id: "same".into(),
                path: "src/failed.rs".into(),
                message: "current".into(),
            },
            review,
        );
        assert!(review.files.with(|files| files.contains_key("src/loaded.rs")));
        assert!(!review
            .loading
            .with(|loading| loading.contains_key("src/loaded.rs")));
        assert!(!review
            .loading
            .with(|loading| loading.contains_key("src/failed.rs")));
    }

    #[test]
    fn scope_and_commit_results_require_the_current_navigation_and_generation() {
        let app = AppState::new();
        let review = ReviewState::new();

        app.begin_item_review(review, "Open", "same", 71);
        app.apply_with_review(review_ready_at_scope("same", 71, 710), review);
        let changed = Arc::new(vec![perch_git::ChangedFile {
            path: "src/current.rs".into(),
            old_path: None,
            status: perch_git::ChangeStatus::Modified,
            additions: 1,
            deletions: 0,
            binary: false,
            head_blob: Some("current".into()),
        }]);

        app.apply_with_review(
            Event::ScopeReady {
                navigation_id: 70,
                scope_gen: 711,
                item_id: "same".into(),
                scope: perch_git::Scope::Branch,
                files: changed.clone(),
            },
            review,
        );
        assert_eq!(review.scope_gen.get(), Some(710));
        assert!(review.changed_files.get().is_empty());

        app.apply_with_review(
            Event::ScopeReady {
                navigation_id: 71,
                scope_gen: 712,
                item_id: "same".into(),
                scope: perch_git::Scope::Branch,
                files: changed,
            },
            review,
        );
        assert_eq!(review.scope_gen.get(), Some(712));
        assert_eq!(review.changed_files.get()[0].path, "src/current.rs");

        app.apply_with_review(
            Event::CommitsReady {
                navigation_id: 71,
                scope_gen: 710,
                item_id: "same".into(),
                commits: Arc::new(Vec::new()),
                dirt: perch_git::Dirt::default(),
                since: Some("stale-generation".into()),
            },
            review,
        );
        app.apply_with_review(
            Event::CommitsReady {
                navigation_id: 70,
                scope_gen: 712,
                item_id: "same".into(),
                commits: Arc::new(Vec::new()),
                dirt: perch_git::Dirt::default(),
                since: Some("stale-navigation".into()),
            },
            review,
        );
        assert!(review.since_head.with(Option::is_none));

        app.apply_with_review(
            Event::CommitsReady {
                navigation_id: 71,
                scope_gen: 712,
                item_id: "same".into(),
                commits: Arc::new(Vec::new()),
                dirt: perch_git::Dirt::default(),
                since: Some("current".into()),
            },
            review,
        );
        assert_eq!(review.since_head.get().as_deref(), Some("current"));
    }

    #[test]
    fn review_navigation_selects_only_the_latest_attempts_exact_group() {
        let app = AppState::new();
        let review = review_of("wt");

        let first = app.begin_agent_review_navigation(
            review,
            "wt".into(),
            "attempt-old".into(),
            41,
        );
        let latest = app.begin_agent_review_navigation(
            review,
            "wt".into(),
            "attempt-new".into(),
            42,
        );
        assert!(latest > first);

        app.apply_with_review(
            Event::AgentReviewNavigationReady {
                item_id: "wt".into(),
                attempt_id: "attempt-old".into(),
                request_id: 41,
                owner_changed: false,
                summary: historical_summary("group-old"),
            },
            review,
        );
        assert!(review.historical_review.with(Option::is_none));

        app.apply_with_review(
            Event::AgentReviewNavigationReady {
                item_id: "wt".into(),
                attempt_id: "attempt-new".into(),
                request_id: 42,
                owner_changed: false,
                summary: historical_summary("group-new"),
            },
            review,
        );
        assert_eq!(
            review
                .historical_review
                .with(|selected| selected.as_ref().map(|summary| summary.group.id.clone())),
            Some("group-new".to_string())
        );
        assert_eq!(review.historical_review_request.get(), Some(42));
        assert!(app.pending_agent_navigation.with(Option::is_none));
    }

    #[test]
    fn cross_item_attempt_failure_retains_the_new_live_owner() {
        let app = AppState::new();
        let review = review_of("old-owner");
        app.expect_item_navigation("old-owner", 50);
        app.begin_agent_review_navigation(review, "wt".into(), "attempt".into(), 51);

        app.apply_with_review(review_ready("wt", 51), review);
        app.apply_with_review(
            Event::AgentReviewNavigationReady {
                item_id: "wt".into(),
                attempt_id: "attempt".into(),
                request_id: 51,
                owner_changed: true,
                summary: historical_summary("group"),
            },
            review,
        );
        app.apply_with_review(
            Event::HistoricalReviewFailed {
                item_id: "wt".into(),
                group_id: "group".into(),
                request_id: 51,
                message: "missing snapshot".into(),
            },
            review,
        );

        assert!(review.is_item("wt"));
        assert!(review.historical_review.with(Option::is_none));
        assert!(app.accepts_navigation(51));
        assert!(!app.accepts_navigation(50));
    }

    #[test]
    fn an_older_same_head_status_generation_cannot_replace_or_clear_the_latest_overview() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        let latest = pull_request_overview("head");

        app.apply_with_review(
            Event::PullRequestOverviewLoading {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                head_sha: "head".into(),
                status_generation: 2,
            },
            review,
        );
        assert_eq!(review.pull_request_status_generation.get(), 2);
        assert!(review.pull_request_overview_loading.get());

        app.apply_with_review(
            Event::PullRequestOverviewReady {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                status_generation: 1,
                overview: pull_request_overview("head"),
            },
            review,
        );
        assert!(review.pull_request_overview.with(Option::is_none));
        assert!(review.pull_request_overview_loading.get());

        app.apply_with_review(
            Event::PullRequestOverviewReady {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                status_generation: 2,
                overview: latest.clone(),
            },
            review,
        );
        app.apply_with_review(
            Event::PullRequestOverviewFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                head_sha: "head".into(),
                status_generation: 1,
                message: "stale failure".into(),
            },
            review,
        );

        assert_eq!(review.pull_request_overview.get(), Some(latest));
        assert!(!review.pull_request_overview_loading.get());
        assert!(review.pull_request_overview_error.with(Option::is_none));
        assert_eq!(review.pull_request_status_generation.get(), 2);
    }

    #[test]
    fn a_current_status_failure_clears_the_merge_authorization_snapshot() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");

        app.apply_with_review(
            Event::PullRequestOverviewReady {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                status_generation: 4,
                overview: pull_request_overview("head"),
            },
            review,
        );
        assert!(review.pull_request_overview.with(Option::is_some));

        app.apply_with_review(
            Event::PullRequestOverviewFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                head_sha: "head".into(),
                status_generation: 4,
                message: "status unavailable".into(),
            },
            review,
        );

        assert!(review.pull_request_overview.with(Option::is_none));
        assert!(!review.pull_request_overview_loading.get());
        assert_eq!(
            review.pull_request_overview_error.get().as_deref(),
            Some("status unavailable")
        );
        assert_eq!(review.pull_request_status_generation.get(), 4);
    }

    #[test]
    fn only_the_current_merge_attempt_can_be_settled_by_refresh_or_failure() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        let item = review.item.get().unwrap();

        app.apply_with_review(
            Event::PullRequestMergeStarted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 10,
            },
            review,
        );
        assert!(review.merge_pending.get());
        assert_eq!(review.merge_attempt_id.get(), Some(10));

        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "wt".into(),
                item: item.clone(),
                pull_request: Arc::new(pull_request(7, "head", "open")),
                merge_attempt_id: None,
            },
            review,
        );
        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "wt".into(),
                item: item.clone(),
                pull_request: Arc::new(pull_request(7, "head", "open")),
                merge_attempt_id: Some(9),
            },
            review,
        );
        app.apply_with_review(
            Event::PullRequestMergeFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 9,
                message: "stale failure".into(),
            },
            review,
        );
        assert!(review.merge_pending.get());
        assert_eq!(review.merge_attempt_id.get(), Some(10));
        assert!(review.merge_error.with(Option::is_none));

        app.apply_with_review(
            Event::PullRequestMergeFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 10,
                message: "merge failed".into(),
            },
            review,
        );
        assert!(!review.merge_pending.get());
        assert_eq!(review.merge_attempt_id.get(), Some(10));
        assert_eq!(review.merge_error.get().as_deref(), Some("merge failed"));

        app.apply_with_review(
            Event::PullRequestMergeStarted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 11,
            },
            review,
        );
        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "wt".into(),
                item: item.clone(),
                pull_request: Arc::new(pull_request(7, "head", "open")),
                merge_attempt_id: Some(10),
            },
            review,
        );
        assert!(review.merge_pending.get());
        assert_eq!(review.merge_attempt_id.get(), Some(11));

        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "wt".into(),
                item,
                pull_request: Arc::new(pull_request(7, "head", "closed")),
                merge_attempt_id: Some(11),
            },
            review,
        );
        assert!(!review.merge_pending.get());
        assert_eq!(review.merge_attempt_id.get(), Some(11));
        assert!(review.merge_error.with(Option::is_none));
    }

    /// A discussion with one comment, `IC_1`, carrying `reactions`.
    fn discussion_with_reactions(reactions: perch_github::Reactions) -> perch_github::Discussion {
        perch_github::Discussion {
            node_id: "PR_7".into(),
            comments: vec![perch_github::IssueComment {
                id: 1,
                node_id: "IC_1".into(),
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "q".into(),
                created_at: "2026-09-15T09:14:00Z".parse().unwrap(),
                updated_at: None,
                html_url: String::new(),
                reactions,
            }],
            ..Default::default()
        }
    }

    /// Somebody else's reaction, `count` of them, none the reader's.
    fn theirs(kind: perch_github::ReactionKind, count: u32) -> perch_github::Reactions {
        perch_github::Reactions {
            groups: vec![perch_github::ReactionGroup {
                kind,
                count,
                viewer_reacted: false,
            }],
        }
    }

    #[test]
    fn a_reaction_is_toggled_where_it_is_shown_and_github_settles_or_reverts_it() {
        use perch_github::{ReactionKind, Reactions};
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        app.apply_with_review(
            Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(discussion_with_reactions(theirs(ReactionKind::Heart, 1))),
            },
            review,
        );
        // Landed by node id: the comment's, and the description's, empty.
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Heart), 1);
        assert!(review.reactions_of("PR_7").is_empty());

        // The pill toggles before GitHub is asked, and asks to add.
        assert_eq!(review.toggle_reaction("IC_1", ReactionKind::Heart), Some(true));
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Heart), 2);
        assert!(review.reactions_of("IC_1").viewer_reacted(ReactionKind::Heart));
        assert!(review.is_reacting("IC_1", ReactionKind::Heart));
        // A second click on the same pill waits for the answer; another
        // pill does not.
        assert_eq!(review.toggle_reaction("IC_1", ReactionKind::Heart), None);
        assert_eq!(review.toggle_reaction("IC_1", ReactionKind::Eyes), Some(true));
        assert_eq!(review.toggle_reaction("", ReactionKind::Eyes), None, "no id, nothing to ask");

        // A read that lands meanwhile, with the older counts, does not
        // put them over the toggle; the description's it may set.
        let mut read = discussion_with_reactions(theirs(ReactionKind::Heart, 1));
        read.reactions = theirs(ReactionKind::Rocket, 1);
        app.apply_with_review(
            Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(read),
            },
            review,
        );
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Heart), 2);
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Eyes), 1);
        assert_eq!(review.reactions_of("PR_7").count(ReactionKind::Rocket), 1);

        // GitHub's answer is what is shown from then on.
        let mut settled = theirs(ReactionKind::Heart, 2);
        settled.toggle(ReactionKind::Heart, true);
        settled.toggle(ReactionKind::Eyes, true);
        app.apply_with_review(
            Event::ReactionsUpdated {
                item_id: "wt".into(),
                node_id: "IC_1".into(),
                kind: ReactionKind::Heart,
                reactions: settled.clone(),
            },
            review,
        );
        assert!(!review.is_reacting("IC_1", ReactionKind::Heart));
        assert!(review.is_reacting("IC_1", ReactionKind::Eyes), "the other ask is still out");
        assert_eq!(review.reactions_of("IC_1"), settled);

        // A refusal puts the pill back as it was before the click.
        app.apply_with_review(
            Event::ReactionFailed {
                item_id: "wt".into(),
                node_id: "IC_1".into(),
                kind: ReactionKind::Eyes,
                add: true,
            },
            review,
        );
        assert!(!review.is_reacting("IC_1", ReactionKind::Eyes));
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Eyes), 0);
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Heart), 3);

        // Another item's answer is not this one's.
        app.apply_with_review(
            Event::ReactionsUpdated {
                item_id: "other".into(),
                node_id: "IC_1".into(),
                kind: ReactionKind::Heart,
                reactions: Reactions::default(),
            },
            review,
        );
        assert_eq!(review.reactions_of("IC_1").count(ReactionKind::Heart), 3);

        // Leaving the item forgets them, in flight or not.
        review.close();
        assert!(review.reactions_of("IC_1").is_empty());
        assert!(!review.is_reacting("IC_1", ReactionKind::Heart));
    }

    #[test]
    fn an_issue_lands_with_the_reactions_on_its_body_and_comments() {
        use perch_github::{ReactionKind, Reactions};
        let app = AppState::new();
        let review = review_of("wt");
        let thumbs = theirs(ReactionKind::ThumbsUp, 4);
        let issue = perch_github::Issue {
            number: 42,
            node_id: "I_42".into(),
            title: "Fence the manifest".into(),
            body: String::new(),
            reactions: thumbs.clone(),
            state: "open".into(),
            state_reason: None,
            author_login: "a".into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            comments: 1,
            created_at: None,
            updated_at: "2026-09-18T10:00:00Z".parse().unwrap(),
            repo_full_name: "slatedb/slatedb".into(),
        };
        let comments = discussion_with_reactions(thumbs.clone()).comments;
        let item = review.item.get_untracked().unwrap();
        app.expect_item_navigation("wt", 7);
        app.apply_with_review(
            Event::IssueReady {
                navigation_id: 7,
                scope_gen: 1,
                item,
                issue: Arc::new(issue),
                comments: Arc::new(comments),
                offline: false,
            },
            review,
        );
        assert_eq!(review.reactions_of("I_42"), thumbs);
        assert_eq!(review.reactions_of("IC_1"), thumbs);

        // Fetched again: the comments' and the body's, read together.
        let hooray = theirs(ReactionKind::Hooray, 1);
        app.apply_with_review(
            Event::IssueComments {
                item_id: "wt".into(),
                comments: Arc::new(discussion_with_reactions(hooray.clone()).comments),
                body_reactions: Reactions::default(),
            },
            review,
        );
        assert_eq!(review.reactions_of("IC_1"), hooray);
        assert!(review.reactions_of("I_42").is_empty());
    }

    #[test]
    fn only_the_current_review_attempt_settles_and_survives_stale_discussion_reads() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        review.ready.set(true);
        review.submit_review_open.set(true);
        review
            .submit_review_draft
            .set("The summary I wrote".into());
        review
            .submit_review_event
            .set(PullReviewEvent::RequestChanges);
        review
            .discussion
            .set(Some(Arc::new(perch_github::Discussion::default())));
        // Submission registration is synchronous in the UI; Started only
        // acknowledges the attempt already owning this surface.
        review.begin_submit_review(10);

        app.apply_with_review(
            Event::PullRequestReviewStarted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 10,
            },
            review,
        );
        assert_eq!(review.submit_review_pending.get(), Some(10));

        let accepted = Review {
            node_id: String::new(),
            reactions: Default::default(),
            id: 99,
            user: "reviewer".into(),
            body: "Please cover the failure path".into(),
            state: "CHANGES_REQUESTED".into(),
            submitted_at: Some("2026-09-20T18:00:00Z".parse().unwrap()),
            html_url: "https://github.com/slatedb/slatedb/pull/7#pullrequestreview-99".into(),
        };
        app.apply_with_review(
            Event::PullRequestReviewSubmitted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 9,
                review: Arc::new(accepted.clone()),
            },
            review,
        );
        assert_eq!(review.submit_review_pending.get(), Some(10));
        assert!(review.submit_review_open.get());
        assert_eq!(
            review.discussion.get().unwrap().reviews,
            vec![accepted.clone()],
            "an accepted older review enters the timeline without settling the newer attempt"
        );

        app.apply_with_review(
            Event::PullRequestReviewSubmitted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 10,
                review: Arc::new(accepted.clone()),
            },
            review,
        );
        assert!(review.submit_review_pending.with(Option::is_none));
        assert!(!review.submit_review_open.get());
        assert!(review.submit_review_draft.get().is_empty());
        assert_eq!(
            review.submit_review_event.get(),
            PullReviewEvent::Comment
        );

        // A GET that began before the POST must not temporarily erase the
        // review GitHub already accepted.
        app.apply_with_review(
            Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(perch_github::Discussion::default()),
            },
            review,
        );
        let discussion = review.discussion.get().unwrap();
        assert_eq!(discussion.reviews, vec![accepted]);
    }

    #[test]
    fn review_submission_failure_keeps_the_draft_and_selected_outcome() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        review.submit_review_open.set(true);
        review.submit_review_draft.set("Needs a test".into());
        review
            .submit_review_event
            .set(PullReviewEvent::RequestChanges);
        review.begin_submit_review(4);

        app.apply_with_review(
            Event::PullRequestReviewFailed {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 4,
                message: "GitHub refused the review".into(),
            },
            review,
        );

        assert!(review.submit_review_pending.with(Option::is_none));
        assert!(review.submit_review_open.get());
        assert_eq!(review.submit_review_draft.get(), "Needs a test");
        assert_eq!(
            review.submit_review_event.get(),
            PullReviewEvent::RequestChanges
        );
        assert_eq!(
            review.submit_review_error.get().as_deref(),
            Some("GitHub refused the review")
        );
    }

    #[test]
    fn returning_to_an_item_accepts_its_inflight_review_without_dropping_a_new_draft() {
        let app = AppState::new();
        let review = review_of_pull("wt", 7, "head");
        review.ready.set(true);
        review.submit_review_open.set(true);
        review.submit_review_draft.set("Original submission".into());
        review.begin_submit_review(12);

        // Leave after sending, then reopen the same pull request while the
        // backend still owns the item-scoped mutation.
        let item = review.item.get_untracked();
        let pull_request = review.pull_request.get_untracked();
        review.begin("Reopening pull request");
        review.item.set(item);
        review.pull_request.set(pull_request);
        review.ready.set(true);
        review.submit_review_open.set(true);
        review.submit_review_draft.set("A second thought".into());
        review
            .discussion
            .set(Some(Arc::new(perch_github::Discussion::default())));

        // The delayed acknowledgement belongs to the surface that navigation
        // reset. It must not claim the new draft.
        app.apply_with_review(
            Event::PullRequestReviewStarted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 12,
            },
            review,
        );
        assert!(review.submit_review_pending.with(Option::is_none));
        assert!(review.submit_review_open.get());
        assert_eq!(review.submit_review_draft.get(), "A second thought");

        // The durable result is still relevant to this pull request and
        // enters its timeline without settling the new surface.
        app.apply_with_review(
            Event::PullRequestReviewSubmitted {
                item_id: "wt".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 7,
                expected_head_sha: "head".into(),
                attempt_id: 12,
                review: Arc::new(Review {
                    node_id: String::new(),
                    reactions: Default::default(),
                    id: 101,
                    user: "reviewer".into(),
                    body: "Approved before navigation".into(),
                    state: "APPROVED".into(),
                    submitted_at: Some("2026-09-20T18:05:00Z".parse().unwrap()),
                    html_url:
                        "https://github.com/slatedb/slatedb/pull/7#pullrequestreview-101".into(),
                }),
            },
            review,
        );

        assert!(review.submit_review_open.get());
        assert_eq!(review.submit_review_draft.get(), "A second thought");
        assert_eq!(
            review.discussion.get().unwrap().reviews[0].id,
            101,
            "the successful original attempt still enters the timeline"
        );
    }

    #[test]
    fn only_the_selected_historical_snapshot_can_replace_the_open_diff() {
        let app = AppState::new();
        let review = review_of("wt");
        review
            .historical_review
            .set(Some(historical_summary("selected")));
        review.historical_review_request.set(Some(7));
        app.expect_item_navigation("wt", 7);
        review
            .changed_files
            .set(Arc::new(vec![perch_git::ChangedFile {
                path: "src/current.rs".into(),
                old_path: None,
                status: perch_git::ChangeStatus::Modified,
                additions: 1,
                deletions: 0,
                binary: false,
                head_blob: Some("current".into()),
            }]));

        let event = |item_id: &str, group_id: &str, request_id: u64, path: &str| {
            Event::HistoricalReviewReady {
                item_id: item_id.into(),
                group_id: group_id.into(),
                request_id,
                scope_gen: request_id,
                files: Arc::new(vec![perch_git::ChangedFile {
                    path: path.into(),
                    old_path: None,
                    status: perch_git::ChangeStatus::Added,
                    additions: 2,
                    deletions: 0,
                    binary: false,
                    head_blob: Some("historical".into()),
                }]),
                tree: Arc::new(vec![perch_git::TreeEntry::new(path, false)]),
                findings: Arc::new(Vec::new()),
            }
        };

        app.apply_with_review(event("wt", "stale", 7, "src/stale.rs"), review);
        app.apply_with_review(event("wt", "selected", 6, "src/repeated.rs"), review);
        app.apply_with_review(event("other", "selected", 7, "src/other.rs"), review);
        assert_eq!(review.changed_files.get()[0].path, "src/current.rs");

        app.apply_with_review(event("wt", "selected", 7, "src/historical.rs"), review);
        assert_eq!(review.scope_gen.get(), Some(7));
        assert_eq!(review.changed_files.get()[0].path, "src/historical.rs");
        assert_eq!(review.tree.get()[0].path, "src/historical.rs");
        assert_eq!(review.tab.get(), crate::review::model::ItemTab::Files);
        assert!(review.historical_findings.with(Option::is_some));
        assert_eq!(
            review
                .historical_review
                .get()
                .as_ref()
                .map(|summary| summary.group.id.as_str()),
            Some("selected")
        );
    }

    #[test]
    fn a_failed_historical_load_rolls_back_only_its_optimistic_selection() {
        let app = AppState::new();
        let review = review_of("wt");
        review
            .historical_review
            .set(Some(historical_summary("stable")));
        review.historical_review_request.set(Some(7));
        review.historical_findings.set(Some(Arc::new(Vec::new())));
        app.expect_item_navigation("wt", 7);

        app.begin_historical_review(review, historical_summary("selected"), 8);
        app.begin_historical_review(review, historical_summary("latest"), 9);

        app.apply_with_review(
            Event::HistoricalReviewFailed {
                item_id: "wt".into(),
                group_id: "selected".into(),
                request_id: 8,
                message: "stale failed".into(),
            },
            review,
        );
        assert_eq!(
            review
                .historical_review
                .get()
                .as_ref()
                .map(|summary| summary.group.id.as_str()),
            Some("latest")
        );

        app.apply_with_review(
            Event::HistoricalReviewFailed {
                item_id: "wt".into(),
                group_id: "latest".into(),
                request_id: 9,
                message: "retained source missing".into(),
            },
            review,
        );
        assert_eq!(
            review
                .historical_review
                .get()
                .as_ref()
                .map(|summary| summary.group.id.as_str()),
            Some("stable")
        );
        assert_eq!(review.historical_review_request.get(), Some(7));
        assert!(review.historical_findings.with(Option::is_some));
        assert_eq!(app.expected_navigation_id.get(), Some(7));
        assert_eq!(app.notice.get().as_deref(), Some("retained source missing"));
    }

    #[test]
    fn an_older_historical_load_cannot_replace_threads_or_files_before_the_latest_fails() {
        let app = AppState::new();
        let review = review_of("wt");
        review
            .historical_review
            .set(Some(historical_summary("group-b")));
        review.historical_review_request.set(Some(22));
        app.expect_item_navigation("wt", 22);
        review
            .changed_files
            .set(Arc::new(vec![perch_git::ChangedFile {
                path: "src/current.rs".into(),
                old_path: None,
                status: perch_git::ChangeStatus::Modified,
                additions: 1,
                deletions: 0,
                binary: false,
                head_blob: Some("current".into()),
            }]));
        review.set_comment_threads(Arc::new(vec![perch_github::CommentThread {
            id: "current-thread".into(),
            worktree_id: "wt".into(),
            origin: perch_github::ThreadOrigin::Local,
            anchor: None,
            outdated: false,
            resolved: false,
            created_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            messages: Vec::new(),
        }]));

        // Request A finishes after B is selected. Both its projected threads
        // and its file payload carry A's request identity and remain inert.
        app.apply_with_review(
            Event::CommentThreads {
                item_id: "wt".into(),
                threads: Vec::new(),
                review_group_id: Some("group-a".into()),
                review_request_id: Some(21),
            },
            review,
        );
        app.apply_with_review(
            Event::HistoricalReviewReady {
                item_id: "wt".into(),
                group_id: "group-a".into(),
                request_id: 21,
                scope_gen: 21,
                files: Arc::new(vec![perch_git::ChangedFile {
                    path: "src/group-a.rs".into(),
                    old_path: None,
                    status: perch_git::ChangeStatus::Added,
                    additions: 2,
                    deletions: 0,
                    binary: false,
                    head_blob: Some("a".into()),
                }]),
                tree: Arc::new(vec![perch_git::TreeEntry::new("src/group-a.rs", false)]),
                findings: Arc::new(Vec::new()),
            },
            review,
        );
        assert_eq!(review.comment_threads.get()[0].id, "current-thread");
        assert_eq!(review.changed_files.get()[0].path, "src/current.rs");

        app.apply_with_review(
            Event::HistoricalReviewFailed {
                item_id: "wt".into(),
                group_id: "group-b".into(),
                request_id: 22,
                message: "group B failed".into(),
            },
            review,
        );
        assert!(review.historical_review.with(Option::is_none));
        assert!(review.historical_review_request.with(Option::is_none));
        assert_eq!(review.comment_threads.get()[0].id, "current-thread");
        assert_eq!(review.changed_files.get()[0].path, "src/current.rs");
        assert_eq!(app.notice.get().as_deref(), Some("group B failed"));
    }

    #[test]
    fn viewed_ready_carries_the_reveals_into_the_open_item() {
        let app = AppState::new();
        let review = review_of("wt");
        // A file with no blob has reveals the store never sees.
        review.reveal("untracked.rs", "", 0, 9);

        // Another item's answer, arriving late, lands nowhere.
        app.apply_with_review(viewed_ready("other", vec![(4, 23)]), review);
        assert!(review.viewed.get().is_empty());
        assert!(review.reveals_for("src/x.rs", "b1").is_empty());

        app.apply_with_review(viewed_ready("wt", vec![(4, 23), (40, 59)]), review);
        assert!(review
            .viewed
            .get()
            .contains(&("src/x.rs".into(), "b1".into())));
        assert_eq!(
            review
                .reveals_for("src/x.rs", "b1")
                .iter()
                .collect::<Vec<_>>(),
            vec![(4, 23), (40, 59)]
        );
        assert!(
            review.reveals_for("untracked.rs", "").overlaps(0, 9),
            "the store has no row to replace it with"
        );

        // Opened again with fewer rows in the store, the state follows.
        app.apply_with_review(viewed_ready("wt", vec![(40, 59)]), review);
        assert_eq!(
            review
                .reveals_for("src/x.rs", "b1")
                .iter()
                .collect::<Vec<_>>(),
            vec![(40, 59)]
        );
        app.apply_with_review(viewed_ready("wt", Vec::new()), review);
        assert!(review.reveals_for("src/x.rs", "b1").is_empty());
        assert!(
            review.reveals_for("untracked.rs", "").overlaps(0, 9),
            "still kept"
        );
    }

    #[test]
    fn a_writes_answer_carries_only_the_viewed_set_and_leaves_the_reveals_alone() {
        let app = AppState::new();
        let review = review_of("wt");
        // Down, applied as it was clicked.
        review.reveal("src/x.rs", "b1", 4, 23);

        // A viewed write's answer, read from the store before the reveal
        // reached it, does not undo it.
        app.apply_with_review(
            Event::ViewedReady {
                item_id: "wt".into(),
                viewed: Arc::new(
                    [("src/x.rs".to_string(), "b1".to_string())]
                        .into_iter()
                        .collect(),
                ),
                reveals: None,
            },
            review,
        );
        assert!(review
            .viewed
            .get()
            .contains(&("src/x.rs".into(), "b1".into())));
        assert_eq!(
            review
                .reveals_for("src/x.rs", "b1")
                .iter()
                .collect::<Vec<_>>(),
            vec![(4, 23)]
        );
    }

    #[test]
    fn a_notice_reaches_the_status_bar_and_gives_up_a_sent_questions_rows() {
        let app = AppState::new();
        let review = review_of("wt");
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nB\nc\n".into(),
            "a\nb\nc\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        review.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        review.active.set(Some("src/x.rs".into()));
        let anchor = perch_github::Anchor {
            path: "src/x.rs".into(),
            start_line: 3,
            line: 3,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        review.open_composer(
            None,
            Some(anchor.clone()),
            String::new(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        review.composer_sent();
        assert_eq!(review.sent_composer.get(), Some(anchor));

        // The send failed: the complaint shows, and the blank rows under
        // the lines go.
        app.apply_with_review(
            Event::Notice("Could not save the question: disk full".into()),
            review,
        );
        assert_eq!(
            app.notice.get().as_deref(),
            Some("Could not save the question: disk full")
        );
        assert!(review.sent_composer.get().is_none());
        assert!(review.cards_in("src/x.rs").is_empty());
    }

    #[test]
    fn simultaneous_agent_completions_wait_their_turn() {
        let _root = floem::headless::TestRoot::new();
        let app = AppState::new();
        app.show_agent_completion("first".into());
        app.show_agent_completion("second".into());

        let first = app.current_agent_completion().unwrap();
        assert_eq!(first.run_id, "first");
        app.dismiss_agent_completion(first.token);
        assert_eq!(
            app.current_agent_completion().map(|notice| notice.run_id),
            Some("second".into())
        );
    }

    #[test]
    fn only_the_current_agent_history_request_can_time_out() {
        let app = AppState::new();
        let stale = app.begin_agent_history_load();
        let current = app.begin_agent_history_load();
        assert!(!app.time_out_agent_history_load(stale));
        assert!(app.agent_history_loading.get());
        assert!(app.time_out_agent_history_load(current));
        assert!(!app.agent_history_loading.get());
        assert!(app.agent_history_load_failed.get());
    }

    #[test]
    fn a_summary_refresh_keeps_already_hydrated_detail() {
        let app = AppState::new();
        let run = perch_github::RunRecord {
            id: "run".into(),
            status: perch_github::RunStatus::Completed,
            terminal_reason: Some(perch_github::TerminalReason::Completed),
            queued_at: "2026-09-19T10:00:00Z".parse().unwrap(),
            ended_at: Some("2026-09-19T10:00:01Z".parse().unwrap()),
            final_result: Some("hydrated answer".into()),
            final_result_truncated: true,
            steps: vec![perch_github::RunStep {
                summary: "Read src/lib.rs".into(),
                ok: true,
            }],
            last_event_sequence: 3,
            ..perch_github::RunRecord::default()
        };
        app.update_agent_run(&run);

        app.merge_agent_runs([perch_github::RunSummary::from(&run)], false);

        let retained = app.agent_run("run").unwrap();
        assert_eq!(retained.response, "hydrated answer");
        assert!(retained.response_truncated);
        assert_eq!(retained.steps.len(), 1);
    }
}
