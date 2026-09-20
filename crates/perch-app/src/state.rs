//! Reactive app state.
//!
//! Every field is a signal, so `AppState` is `Copy` and can be handed to any
//! view closure without lifetime plumbing. Derived lists are plain functions
//! over these signals, recomputed by whichever effect reads them.

use std::collections::{HashMap, HashSet};
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
    pub item_id: String,
    pub thread_id: Option<String>,
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
    /// GitHub's notification inbox, in the order GitHub returned it.
    /// Read threads stay here until GitHub removes them from the inbox.
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
    pub agents_opener: RwSignal<Option<floem::ViewId>>,
    pub agent_history_loading: RwSignal<bool>,
    pub agent_history_has_more: RwSignal<bool>,
    pub pending_agent_navigation: RwSignal<Option<PendingAgentNavigation>>,
    pub agent_navigation_token: RwSignal<u64>,
    open_expectation: RwSignal<OpenExpectation>,
    /// An archived discussion opened from run history is a read-only view.
    /// Keep it open across the normal Items refresh without restoring its
    /// checkout or changing its archived state.
    pub agent_archived_owner: RwSignal<Option<String>>,
    pub agent_completion_notice: RwSignal<Option<AgentCompletionNotice>>,
    pub agent_completion_token: RwSignal<u64>,

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
            agents_opener: RwSignal::new(None),
            agent_history_loading: RwSignal::new(false),
            agent_history_has_more: RwSignal::new(false),
            pending_agent_navigation: RwSignal::new(None),
            agent_navigation_token: RwSignal::new(0),
            open_expectation: RwSignal::new(OpenExpectation::Any),
            agent_archived_owner: RwSignal::new(None),
            agent_completion_notice: RwSignal::new(None),
            agent_completion_token: RwSignal::new(0),
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
        runs: impl IntoIterator<Item = perch_github::RunRecord>,
        has_more: bool,
    ) {
        self.agent_runs.update(|current| {
            let mut merged = current.as_ref().clone();
            for run in runs {
                let summary = AgentRunSummary::from_record(&run);
                match merged.iter().position(|existing| existing.id == summary.id) {
                    Some(index) if merged[index].can_replace_with(&summary) => {
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
        self.agent_completion_notice
            .set(Some(AgentCompletionNotice { run_id, token }));
        let state = *self;
        floem::action::exec_after(Duration::from_secs(8), move |_| {
            state.agent_completion_notice.update(|notice| {
                if notice.as_ref().is_some_and(|notice| notice.token == token) {
                    *notice = None;
                }
            });
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
            perch_agent::RunEvent::ToolCall { id, name, summary } => {
                perch_github::RunEvent::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    summary: summary.clone(),
                }
            }
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
        if self.agent_completion_notice.with_untracked(|notice| {
            notice
                .as_ref()
                .is_some_and(|notice| removed.contains(&notice.run_id))
        }) {
            self.agent_completion_notice.set(None);
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

    pub fn begin_agent_navigation(&self, item_id: String, thread_id: Option<String>) -> u64 {
        let token = self.agent_navigation_token.get_untracked().wrapping_add(1);
        self.agent_navigation_token.set(token);
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
                item_id,
                thread_id,
            }));
        token
    }

    fn accepts_open_item(&self, item_id: &str) -> bool {
        self.open_expectation
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
            Event::ReviewProgress { message, percent } => {
                review
                    .preparing
                    .set(Preparing::Working { message, percent });
                review.set_phase(Phase::Preparing);
            }
            // The phase goes last, after every field the view will read.
            Event::ReviewReady {
                item,
                pull_request,
                files,
                tree,
                stale_head,
                offline,
            } => {
                if !self.accepts_open_item(&item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale item-open result");
                    return;
                }
                self.open_expectation
                    .set(OpenExpectation::Item(item.id.clone()));
                review.preparing.set(Preparing::Idle);
                review.set_changed_files(files);
                review.tree.set(tree);
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.stale_head.set(stale_head);
                review.offline.set(offline);
                review.pull_request.set(pull_request);
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
                item,
                issue,
                comments,
                offline,
            } => {
                if !self.accepts_open_item(&item.id) {
                    tracing::debug!(item = %item.id, "ignored a stale issue-open result");
                    return;
                }
                self.open_expectation
                    .set(OpenExpectation::Item(item.id.clone()));
                review.preparing.set(Preparing::Idle);
                review.changed_files.set(Arc::new(Vec::new()));
                review.tree.set(Arc::new(Vec::new()));
                self.open_item.set(Some(item.id.clone()));
                review.item.set(Some(item));
                review.pull_request.set(None);
                review.stale_head.set(None);
                review.offline.set(offline);
                review.issue.set(Some(issue));
                review.issue_comments.set(comments);
                review.comment_threads.set(Arc::new(Vec::new()));
                review.tab.set(ItemTab::Conversation);
                review.ready.set(true);
                review.set_phase(Phase::Issue);
            }
            Event::CommentThreads { item_id, threads } => {
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
                            && pending.item_id == item_id
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
            Event::ReviewFailed { message } => {
                review.preparing.set(Preparing::Failed { message });
                review.set_phase(Phase::Failed);
            }
            Event::LspStatus(status) => review.lsp_status.set(status),
            Event::LspHover { token, text } => {
                // Drop an answer the pointer has already moved past.
                if review.request_token.get() == token {
                    review.hover_text.set(Some(text));
                }
            }
            Event::LspDefinition { token, path, line } => {
                if review.request_token.get() == token {
                    if let Some(path) = path {
                        review.pending_jump.set(Some((path, line as usize)));
                    }
                }
            }
            // A load for the last item, still on its way when the next one
            // opened, is nobody's now.
            Event::FileLoaded {
                item_id,
                path,
                file,
            } => {
                if review.is_item(&item_id) {
                    review.finish_load(&path, file);
                }
            }
            Event::IssueComments { item_id, comments } => {
                if review.is_item(&item_id) {
                    review.issue_comments.set(comments);
                }
            }
            Event::FileFailed { path, message } => {
                tracing::warn!(path, message, "could not open file");
                review.fail_load(&path);
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
                    review.discussion.set(Some(discussion));
                }
            }
            Event::ChecksReady { item_id, checks } => {
                if review.is_item(&item_id) {
                    review.checks.set(Some(checks));
                }
            }
            Event::CommitsReady {
                item_id,
                commits,
                dirt,
                since,
            } => {
                if review.is_item(&item_id) {
                    review.commits.set(commits);
                    review.dirty.set(dirt);
                    review.since_head.set(since);
                }
            }
            Event::ScopeReady {
                item_id,
                scope,
                files,
            } => {
                if review.is_item(&item_id) {
                    for path in review.change_scope(scope, files) {
                        self.reload.update(|r| r.push(path));
                    }
                }
            }
            Event::ViewedReady {
                item_id,
                viewed,
                reveals,
                folded,
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
                    if let Some(folded) = folded {
                        if review.folded.get_untracked() != folded {
                            review.folded.set(folded);
                        }
                    }
                }
            }
            Event::LspReferences {
                token,
                references,
                elsewhere,
            } => {
                // Matched against the panel's own token: the shared counter
                // moves on with every hover while the answer is on its way.
                review.references.update(|r| {
                    if let Some(r) = r.as_mut().filter(|r| r.token == token) {
                        let mut open = (**r).clone();
                        open.locations = references;
                        open.elsewhere = elsewhere;
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
            | Event::IssueReady { .. } => {}
            Event::ReviewProgress { .. }
            | Event::ReviewReady { .. }
            | Event::ReviewFailed { .. }
            | Event::FileLoaded { .. }
            | Event::FileFailed { .. }
            | Event::LspStatus(_)
            | Event::LspHover { .. }
            | Event::LspDefinition { .. }
            | Event::DiscussionReady { .. }
            | Event::IssueComments { .. }
            | Event::ChecksReady { .. }
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
    ) {
        self.open_expectation
            .set(OpenExpectation::Item(item_id.to_string()));
        self.open_item.set(Some(item_id.to_string()));
        review.begin(message);
    }

    /// The progress card, over the open review or the empty main area, for
    /// an act that ends in another item being opened: Start work, Fork.
    pub fn begin_open(&self, message: &str) {
        let Some(review) = self.review.get_untracked() else {
            return;
        };
        self.pending_agent_navigation.set(None);
        self.agent_archived_owner.set(None);
        self.open_expectation.set(OpenExpectation::Any);
        self.open_item.set(None);
        review.begin(message);
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
    use perch_github::{Reason, Repository, Subject, SubjectKind};

    /// A review with an item open, by id.
    fn review_of(id: &str) -> ReviewState {
        let review = ReviewState::new();
        let mut item = Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = id.into();
        review.item.set(Some(Arc::new(item)));
        review
    }

    /// What the backend sends for an item on the open: `src/x.rs` at `b1`
    /// viewed, with two runs revealed in it.
    fn viewed_ready(item_id: &str, reveals: Vec<(u32, u32)>, folded: bool) -> Event {
        let key = ("src/x.rs".to_string(), "b1".to_string());
        Event::ViewedReady {
            item_id: item_id.into(),
            viewed: Arc::new([key.clone()].into_iter().collect()),
            reveals: Some(Arc::new(if reveals.is_empty() {
                HashMap::new()
            } else {
                [(key, reveals)].into_iter().collect()
            })),
            folded: Some(folded),
        }
    }

    fn notification(id: &str, unread: bool) -> Thread {
        Thread {
            id: id.into(),
            unread,
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
        }
    }

    fn review_ready(item_id: &str) -> Event {
        let mut item = Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = item_id.into();
        Event::ReviewReady {
            item: Arc::new(item),
            pull_request: None,
            files: Arc::new(Vec::new()),
            tree: Arc::new(Vec::new()),
            stale_head: None,
            offline: true,
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

        let first = app.begin_agent_navigation("older".into(), Some("thread-old".into()));
        let latest = app.begin_agent_navigation("latest".into(), Some("thread-new".into()));
        assert!(latest > first);
        app.begin_review(review, "Reading the discussion");

        app.apply_with_review(review_ready("older"), review);
        assert_eq!(app.open_item.get().as_deref(), Some("latest"));
        assert!(!review.is_item("older"));

        app.apply_with_review(review_ready("latest"), review);
        app.apply_with_review(
            Event::CommentThreads {
                item_id: "latest".into(),
                threads: Vec::new(),
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
        app.apply_with_review(review_ready("older"), review);
        assert!(review.is_item("latest"));
        app.leave_review(review);
        app.apply_with_review(review_ready("latest"), review);
        assert!(!review.is_showing());
    }

    #[test]
    fn viewed_ready_carries_the_reveals_and_the_folded_flag_into_the_open_item() {
        let app = AppState::new();
        let review = review_of("wt");
        // A file with no blob has reveals the store never sees.
        review.reveal("untracked.rs", "", 0, 9);

        // Another item's answer, arriving late, lands nowhere.
        app.apply_with_review(viewed_ready("other", vec![(4, 23)], false), review);
        assert!(review.folded.get());
        assert!(review.viewed.get().is_empty());
        assert!(review.reveals_for("src/x.rs", "b1").is_empty());

        app.apply_with_review(viewed_ready("wt", vec![(4, 23), (40, 59)], false), review);
        assert!(!review.folded.get(), "the item's files were left whole");
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
        app.apply_with_review(viewed_ready("wt", vec![(40, 59)], true), review);
        assert!(review.folded.get());
        assert_eq!(
            review
                .reveals_for("src/x.rs", "b1")
                .iter()
                .collect::<Vec<_>>(),
            vec![(40, 59)]
        );
        app.apply_with_review(viewed_ready("wt", Vec::new(), true), review);
        assert!(review.reveals_for("src/x.rs", "b1").is_empty());
        assert!(
            review.reveals_for("untracked.rs", "").overlaps(0, 9),
            "still kept"
        );
    }

    #[test]
    fn a_writes_answer_carries_only_the_viewed_set_and_leaves_the_reveals_and_the_fold_alone() {
        let app = AppState::new();
        let review = review_of("wt");
        // Down, then Show whole, both applied as they were clicked.
        review.reveal("src/x.rs", "b1", 4, 23);
        review.folded.set(false);

        // A viewed write's answer, read from the store before either
        // reached it, changes neither.
        app.apply_with_review(
            Event::ViewedReady {
                item_id: "wt".into(),
                viewed: Arc::new(
                    [("src/x.rs".to_string(), "b1".to_string())]
                        .into_iter()
                        .collect(),
                ),
                reveals: None,
                folded: None,
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
        assert!(
            !review.folded.get(),
            "Show whole is not undone by a write that came before it"
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
}
